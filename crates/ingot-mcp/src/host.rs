//! The [`ToolHost`] the interpreter calls.
//!
//! Routing is decided once, when the host connects, and then it is fixed. Every
//! tool the artifact declares resolves to exactly one server, or to none — and
//! "none" stops the run at the call rather than skipping it.
//!
//! Two servers publishing the same tool is a configuration error, not a race:
//! which one answers must not depend on which one started first.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use ingot_runtime::{ToolError, ToolHost, ToolInvocation};
use serde_json::{Map, Value};

use crate::client::{McpClient, McpError, ServerInfo, ToolDescriptor};
use crate::config::{McpConfig, ServerConfig};
use crate::convert::to_ingot_value;
use crate::transport::ChildTransport;

/// The only transport an artifact may declare in language 0.1.
const TRANSPORT: &str = "mcp";

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

struct Connected {
    name: String,
    client: McpClient,
    published: Vec<ToolDescriptor>,
}

struct Route {
    server: usize,
    remote: String,
}

pub struct McpToolHost {
    servers: Vec<Connected>,
    routes: BTreeMap<String, Route>,
    aliased: BTreeSet<String>,
}

impl McpToolHost {
    /// Connect to the servers needed to serve `required`.
    ///
    /// A server whose manifest entry maps none of the required tools is not
    /// started at all: configuring a tool server should not mean paying for it
    /// on every run of every agent.
    pub fn connect(
        config: &McpConfig,
        root: &Path,
        required: &BTreeSet<String>,
    ) -> Result<McpToolHost, McpError> {
        McpToolHost::connect_inner(config, root, Some(required))
    }

    /// Connect to every configured server, whatever the artifact needs. Used by
    /// `ingot tools`, whose whole job is to show what is out there.
    pub fn connect_all(config: &McpConfig, root: &Path) -> Result<McpToolHost, McpError> {
        McpToolHost::connect_inner(config, root, None)
    }

    fn connect_inner(
        config: &McpConfig,
        root: &Path,
        required: Option<&BTreeSet<String>>,
    ) -> Result<McpToolHost, McpError> {
        config.validate().map_err(McpError::Configuration)?;

        let mut host = McpToolHost {
            servers: Vec::new(),
            routes: BTreeMap::new(),
            aliased: BTreeSet::new(),
        };

        if required.is_some_and(BTreeSet::is_empty) {
            return Ok(host);
        }

        for server in &config.servers {
            if let Some(required) = required {
                if skippable(server, required) {
                    continue;
                }
            }

            let transport = ChildTransport::spawn(
                &server.command,
                &server.args,
                Some(&working_directory(root, server)),
                &server.pass_env,
            )
            .map_err(|error| McpError::Transport {
                server: server.name.clone(),
                reason: error.to_string(),
                stderr: String::new(),
            })?;

            let mut client =
                McpClient::new(server.name.clone(), Box::new(transport), config.timeout());
            let info = client.initialize()?.clone();
            if !info.serves_tools {
                return Err(McpError::Configuration(format!(
                    "MCP server `{}` ({} {}) declares no `tools` capability, so it has no tools \
                     to serve",
                    server.name, info.name, info.version
                )));
            }
            let published = client.list_tools()?;

            let index = host.servers.len();
            host.servers.push(Connected {
                name: server.name.clone(),
                client,
                published,
            });
            host.add_routes(index, server, required)?;
        }

        Ok(host)
    }

    fn add_routes(
        &mut self,
        index: usize,
        config: &ServerConfig,
        required: Option<&BTreeSet<String>>,
    ) -> Result<(), McpError> {
        let published: BTreeSet<&str> = self.servers[index]
            .published
            .iter()
            .map(|tool| tool.name.as_str())
            .collect();

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
                let first = &self.servers[existing.server].name;
                return Err(McpError::Configuration(format!(
                    "both `{first}` and `{}` serve the tool `{tool}`\n  \
                     rename one with an entry under `[mcp.server.tools]`, or drop a server",
                    config.name
                )));
            }
            self.routes.insert(
                tool,
                Route {
                    server: index,
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
                server: self.servers[route.server].name.clone(),
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
        self.servers
            .iter()
            .map(|server| {
                (
                    server.name.clone(),
                    server.client.info().cloned(),
                    server.published.clone(),
                )
            })
            .collect()
    }

    pub fn is_empty(&self) -> bool {
        self.servers.is_empty()
    }

    /// Stop every server. Also happens on drop; explicit so a caller can do it
    /// before printing a summary.
    pub fn close(&mut self) {
        for server in &mut self.servers {
            server.client.close();
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
        let server = route.server;

        let mut arguments = Map::new();
        for (name, value) in &invocation.arguments {
            arguments.insert(name.clone(), value.clone());
        }

        let outcome = self.servers[server]
            .client
            .call_tool(&remote, Value::Object(arguments))
            .map_err(|error| ToolError::Failed(error.to_string()))?;

        to_ingot_value(&remote, &invocation.result_type, &outcome)
    }
}

/// Whether a server can be left unstarted for this run.
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
    fn a_server_that_maps_nothing_this_run_needs_is_not_started() {
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
}
