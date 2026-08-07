//! The [`ToolHost`] the interpreter calls.
//!
//! Routing is decided once, when the host connects, and then it is fixed. Every
//! tool the artifact declares resolves to exactly one server, or to none — and
//! "none" stops the run at the call rather than skipping it.
//!
//! Two servers publishing the same tool is a configuration error, not a race:
//! which one answers must not depend on which one started first.
//!
//! A server is started **once per agent that holds one of its tools**, not once
//! overall. Two agents in a program deliberately differ — in
//! `examples/code-review-team` the sub-agent may read and the coordinator may
//! write — and when a [`Launcher`] bounds what a server can reach, one shared
//! instance would have to be wide enough for both.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use ingot_runtime::{ToolError, ToolHost, ToolInvocation};
use serde_json::{Map, Value};

use crate::client::{McpClient, McpError, ServerInfo, ToolDescriptor};
use crate::config::{McpConfig, ServerConfig};
use crate::convert::to_ingot_value;
use crate::transport::{ChildTransport, Transport};

/// The only transport an artifact may declare in language 0.1.
const TRANSPORT: &str = "mcp";

/// One agent and the MCP tools it holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTools {
    pub agent: String,
    pub tools: BTreeSet<String>,
}

impl AgentTools {
    pub fn new(agent: impl Into<String>, tools: BTreeSet<String>) -> AgentTools {
        AgentTools {
            agent: agent.into(),
            tools,
        }
    }
}

/// How a server process is started.
///
/// The default starts it directly. A sandboxing launcher starts it inside a
/// boundary derived from the calling agent's policy — which is why the agent's
/// name is a parameter rather than something the launcher could look up.
pub trait Launcher {
    fn launch(
        &self,
        server: &ServerConfig,
        agent: &str,
        cwd: &Path,
    ) -> Result<Box<dyn Transport>, String>;

    /// One line for the run log, so it is never a mystery whether a boundary
    /// was in effect.
    fn describe(&self) -> String;
}

/// Starts a server as a child process of this one, with the operator's
/// filesystem and the operator's network. What Ingot has always done.
pub struct DirectLauncher;

impl Launcher for DirectLauncher {
    fn launch(
        &self,
        server: &ServerConfig,
        _agent: &str,
        cwd: &Path,
    ) -> Result<Box<dyn Transport>, String> {
        ChildTransport::spawn(&server.command, &server.args, Some(cwd), &server.pass_env)
            .map(|transport| Box::new(transport) as Box<dyn Transport>)
            .map_err(|error| error.to_string())
    }

    fn describe(&self) -> String {
        "tool servers run as child processes; the policy is checked, not enforced".to_string()
    }
}

/// Where one Ingot tool name is served from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedTool {
    /// The name the artifact declares.
    pub tool: String,
    /// The configured server serving it.
    pub server: String,
    /// The name that server publishes it under.
    pub remote: String,
    /// Whether the manifest mapped the name explicitly.
    pub aliased: bool,
}

struct Instance {
    agent: String,
    client: McpClient,
    published: Vec<ToolDescriptor>,
}

struct Slot {
    name: String,
    instances: Vec<Instance>,
}

struct Route {
    slot: usize,
    remote: String,
}

pub struct McpToolHost {
    slots: Vec<Slot>,
    routes: BTreeMap<String, Route>,
    aliased: BTreeSet<String>,
    launcher: String,
}

impl McpToolHost {
    /// Connect for a single unnamed agent, starting servers directly.
    ///
    /// The shape most callers want, and the one every test that does not care
    /// about boundaries uses.
    pub fn connect(
        config: &McpConfig,
        root: &Path,
        required: &BTreeSet<String>,
    ) -> Result<McpToolHost, McpError> {
        let agents = [AgentTools::new(String::new(), required.clone())];
        McpToolHost::connect_agents(config, root, &agents, &DirectLauncher)
    }

    /// Connect to every configured server, whatever the artifact needs. Used by
    /// `ingot tools`, whose whole job is to show what is out there.
    pub fn connect_all(config: &McpConfig, root: &Path) -> Result<McpToolHost, McpError> {
        McpToolHost::build(config, root, None, &DirectLauncher)
    }

    /// Connect for each agent that needs a server, through `launcher`.
    pub fn connect_agents(
        config: &McpConfig,
        root: &Path,
        agents: &[AgentTools],
        launcher: &dyn Launcher,
    ) -> Result<McpToolHost, McpError> {
        McpToolHost::build(config, root, Some(agents), launcher)
    }

    fn build(
        config: &McpConfig,
        root: &Path,
        agents: Option<&[AgentTools]>,
        launcher: &dyn Launcher,
    ) -> Result<McpToolHost, McpError> {
        config.validate().map_err(McpError::Configuration)?;

        let mut host = McpToolHost {
            slots: Vec::new(),
            routes: BTreeMap::new(),
            aliased: BTreeSet::new(),
            launcher: launcher.describe(),
        };

        // `None` means "show me everything", which `ingot tools` wants. One
        // unnamed agent that needs nothing means "start nothing".
        let wanted: Vec<AgentTools> = match agents {
            Some(agents) => agents
                .iter()
                .filter(|agent| !agent.tools.is_empty())
                .cloned()
                .collect(),
            None => vec![AgentTools::new(String::new(), BTreeSet::new())],
        };
        if agents.is_some() && wanted.is_empty() {
            return Ok(host);
        }

        for server in &config.servers {
            let needed: Vec<&AgentTools> = wanted
                .iter()
                .filter(|agent| agents.is_none() || !skippable(server, &agent.tools))
                .collect();
            if needed.is_empty() {
                continue;
            }

            let cwd = working_directory(root, server);
            let mut instances = Vec::new();
            for agent in needed {
                let transport = launcher
                    .launch(server, &agent.agent, &cwd)
                    .map_err(|reason| McpError::Transport {
                        server: server.name.clone(),
                        reason,
                        stderr: String::new(),
                    })?;

                let mut client = McpClient::new(server.name.clone(), transport, config.timeout());
                let info = client.initialize()?.clone();
                if !info.serves_tools {
                    return Err(McpError::Configuration(format!(
                        "MCP server `{}` ({} {}) declares no `tools` capability, so it has no \
                         tools to serve",
                        server.name, info.name, info.version
                    )));
                }
                let published = client.list_tools()?;
                instances.push(Instance {
                    agent: agent.agent.clone(),
                    client,
                    published,
                });
            }

            let index = host.slots.len();
            host.slots.push(Slot {
                name: server.name.clone(),
                instances,
            });
            // Routing comes from the first instance. A server publishes the
            // same tools whichever agent it was started for; only what those
            // tools can reach differs.
            let filter = agents.map(|_| {
                wanted
                    .iter()
                    .flat_map(|agent| agent.tools.iter().cloned())
                    .collect::<BTreeSet<String>>()
            });
            host.add_routes(index, server, filter.as_ref())?;
        }

        Ok(host)
    }

    fn add_routes(
        &mut self,
        index: usize,
        config: &ServerConfig,
        required: Option<&BTreeSet<String>>,
    ) -> Result<(), McpError> {
        let published: BTreeSet<&str> = self.slots[index]
            .instances
            .first()
            .map(|instance| {
                instance
                    .published
                    .iter()
                    .map(|tool| tool.name.as_str())
                    .collect()
            })
            .unwrap_or_default();

        // Explicit aliases first: they are the operator's stated intent, and
        // they win over an accidental name match on the same server.
        let mut local: BTreeMap<String, String> = BTreeMap::new();
        for (tool, remote) in &config.tools {
            if required.is_some_and(|required| !required.contains(tool)) {
                continue;
            }
            if !published.contains(remote.as_str()) {
                return Err(McpError::Configuration(format!(
                    "MCP server `{}` maps `{tool}` to `{remote}`, which it does not publish\n  \
                     it publishes: {}",
                    config.name,
                    list(&published)
                )));
            }
            local.insert(tool.clone(), remote.clone());
            self.aliased.insert(tool.clone());
        }

        for tool in &published {
            if local.contains_key(*tool) {
                continue;
            }
            if required.is_some_and(|required| !required.contains(*tool)) {
                continue;
            }
            local.insert((*tool).to_string(), (*tool).to_string());
        }

        for (tool, remote) in local {
            if let Some(existing) = self.routes.get(&tool) {
                let first = &self.slots[existing.slot].name;
                return Err(McpError::Configuration(format!(
                    "both `{first}` and `{}` serve the tool `{tool}`\n  \
                     rename one with an entry under `[mcp.server.tools]`, or drop a server",
                    config.name
                )));
            }
            self.routes.insert(
                tool,
                Route {
                    slot: index,
                    remote,
                },
            );
        }
        Ok(())
    }

    /// Every tool this host can serve, in name order.
    pub fn resolved(&self) -> Vec<ResolvedTool> {
        self.routes
            .iter()
            .map(|(tool, route)| ResolvedTool {
                tool: tool.clone(),
                server: self.slots[route.slot].name.clone(),
                remote: route.remote.clone(),
                aliased: self.aliased.contains(tool),
            })
            .collect()
    }

    /// Which of `required` nothing serves.
    pub fn unresolved(&self, required: &BTreeSet<String>) -> Vec<String> {
        required
            .iter()
            .filter(|tool| !self.routes.contains_key(*tool))
            .cloned()
            .collect()
    }

    /// Connected servers and what each publishes, for `ingot tools`.
    pub fn inventory(&self) -> Vec<(String, Option<ServerInfo>, Vec<ToolDescriptor>)> {
        self.slots
            .iter()
            .filter_map(|slot| {
                let instance = slot.instances.first()?;
                Some((
                    slot.name.clone(),
                    instance.client.info().cloned(),
                    instance.published.clone(),
                ))
            })
            .collect()
    }

    /// How servers were started, for the run log.
    pub fn launcher(&self) -> &str {
        &self.launcher
    }

    /// How many server processes are running.
    pub fn instances(&self) -> usize {
        self.slots.iter().map(|slot| slot.instances.len()).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// Stop every server. Also happens on drop; explicit so a caller can do it
    /// before printing a summary.
    pub fn close(&mut self) {
        for slot in &mut self.slots {
            for instance in &mut slot.instances {
                instance.client.close();
            }
        }
    }
}

impl ToolHost for McpToolHost {
    fn name(&self) -> &str {
        "mcp"
    }

    fn provides(&self, tool: &str) -> bool {
        self.routes.contains_key(tool)
    }

    fn call(&mut self, invocation: &ToolInvocation) -> Result<Value, ToolError> {
        if invocation.transport != TRANSPORT {
            return Err(ToolError::NotAvailable(format!(
                "{} (transport `{}`; this host serves `{TRANSPORT}`)",
                invocation.name, invocation.transport
            )));
        }
        let Some(route) = self.routes.get(&invocation.name) else {
            return Err(ToolError::NotAvailable(invocation.name.clone()));
        };
        let remote = route.remote.clone();
        let slot = route.slot;

        let instance = match instance_for(&mut self.slots[slot], &invocation.agent) {
            Some(instance) => instance,
            None => {
                return Err(ToolError::Failed(format!(
                    "no instance of `{}` was started for agent `{}`; \
                     a server is started per agent so that each gets its own policy's bound",
                    self.slots[slot].name, invocation.agent
                )))
            }
        };

        let mut arguments = Map::new();
        for (name, value) in &invocation.arguments {
            arguments.insert(name.clone(), value.clone());
        }

        let outcome = instance
            .client
            .call_tool(&remote, Value::Object(arguments))
            .map_err(|error| ToolError::Failed(error.to_string()))?;

        to_ingot_value(&remote, &invocation.result_type, &outcome)
    }
}

/// The instance started for this agent.
///
/// Falls back to the only instance when there is exactly one and it was started
/// for no particular agent — the shape [`McpToolHost::connect`] produces.
fn instance_for<'a>(slot: &'a mut Slot, agent: &str) -> Option<&'a mut Instance> {
    if slot.instances.len() == 1 && slot.instances[0].agent.is_empty() {
        return slot.instances.first_mut();
    }
    slot.instances
        .iter_mut()
        .find(|instance| instance.agent == agent)
}

/// Whether a server can be left unstarted for this agent.
///
/// Only decidable when the operator mapped names explicitly. With no map, the
/// server's tool list is the only way to know what it offers, and reading that
/// means starting it.
fn skippable(server: &ServerConfig, required: &BTreeSet<String>) -> bool {
    !server.tools.is_empty() && !server.tools.keys().any(|tool| required.contains(tool))
}

fn working_directory(root: &Path, server: &ServerConfig) -> PathBuf {
    match &server.cwd {
        Some(cwd) => root.join(cwd),
        None => root.to_path_buf(),
    }
}

fn list(names: &BTreeSet<&str>) -> String {
    if names.is_empty() {
        "nothing".to_string()
    } else {
        names.iter().copied().collect::<Vec<_>>().join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(name: &str, tools: &[(&str, &str)]) -> ServerConfig {
        ServerConfig {
            name: name.to_string(),
            command: "unused".to_string(),
            args: Vec::new(),
            image: None,
            cwd: None,
            pass_env: Vec::new(),
            tools: tools
                .iter()
                .map(|(tool, remote)| (tool.to_string(), remote.to_string()))
                .collect(),
        }
    }

    fn required(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    #[test]
    fn a_server_that_maps_nothing_this_agent_needs_is_not_started() {
        let server = config("mail", &[("mailer.send", "send")]);
        assert!(skippable(&server, &required(&["web.search"])));
        assert!(!skippable(&server, &required(&["mailer.send"])));
    }

    #[test]
    fn a_server_with_no_map_must_be_started_to_find_out_what_it_has() {
        let server = config("files", &[]);
        assert!(!skippable(&server, &required(&["anything"])));
    }

    #[test]
    fn the_working_directory_defaults_to_the_project_root() {
        let root = Path::new("/projects/review");
        assert_eq!(
            working_directory(root, &config("files", &[])),
            PathBuf::from("/projects/review")
        );
        let mut elsewhere = config("files", &[]);
        elsewhere.cwd = Some("workspace".to_string());
        assert_eq!(
            working_directory(root, &elsewhere),
            Path::new("/projects/review").join("workspace")
        );
    }

    #[test]
    fn an_empty_requirement_set_starts_nothing() {
        let config = McpConfig {
            servers: vec![config("files", &[])],
            ..McpConfig::default()
        };
        let Ok(host) = McpToolHost::connect(&config, Path::new("."), &BTreeSet::new()) else {
            panic!("connecting to nothing cannot fail");
        };
        assert!(host.is_empty());
        assert!(host.resolved().is_empty());
        assert_eq!(host.instances(), 0);
    }

    #[test]
    fn an_agent_that_holds_no_tools_starts_nothing() {
        let config = McpConfig {
            servers: vec![config("files", &[])],
            ..McpConfig::default()
        };
        let agents = [AgentTools::new("test.Quiet", BTreeSet::new())];
        let Ok(host) =
            McpToolHost::connect_agents(&config, Path::new("."), &agents, &DirectLauncher)
        else {
            panic!("an agent with no tools needs no server");
        };
        assert!(host.is_empty());
    }

    #[test]
    fn a_broken_manifest_is_refused_before_anything_is_spawned() {
        let config = McpConfig {
            servers: vec![config("files", &[]), config("files", &[])],
            ..McpConfig::default()
        };
        let Err(error) = McpToolHost::connect(&config, Path::new("."), &required(&["x"])) else {
            panic!("two servers with the same name must be refused");
        };
        assert!(
            matches!(error, McpError::Configuration(ref reason) if reason.contains("unique")),
            "{error}"
        );
    }

    #[test]
    fn the_direct_launcher_says_the_policy_is_not_enforced() {
        let described = DirectLauncher.describe();
        assert!(described.contains("not enforced"), "{described}");
    }
}
