//! `ingot sandbox` — what boundary each tool server would run inside.
//!
//! An inspection command, in the same family as `ingot tools`. It answers "what
//! can this agent actually reach?" from the artifact and the manifest alone,
//! before a container exists and without starting anything.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::Result;
use ingot_compiler::Compilation;
use ingot_ir::AgentIr;
use ingot_mcp::{ChildTransport, Launcher, McpConfig, ServerConfig, Transport};
use ingot_sandbox::{invocation, render, ExecutorError, Runtime, SandboxPlan};

pub struct SandboxConfig {
    /// The root the artifact's policy paths are relative to.
    pub workspace: PathBuf,
    pub mcp: McpConfig,
    /// Print the plans as JSON instead of prose, for piping.
    pub json: bool,
}

/// Report the boundary for every (server, agent) pair this program needs.
pub fn inspect(compilation: &Compilation, config: &SandboxConfig) -> Result<u8> {
    if pairs(compilation, &config.mcp).is_empty() {
        eprintln!("no tool server is configured, so nothing would be contained");
        eprintln!("hint: `ingot tools` shows what this program declares and what serves it");
        return Ok(super::EXIT_OK);
    }

    let plans = match plan_all(compilation, &config.mcp, &config.workspace) {
        Ok(plans) => plans.into_values().collect::<Vec<_>>(),
        Err(problems) => {
            for problem in problems {
                eprintln!("error: {problem}");
            }
            eprintln!();
            eprintln!(
                "hint: a policy path is relative to the workspace ({})",
                config.workspace.display()
            );
            eprintln!("      pass --workspace <DIR> if that is not where the files are");
            return Ok(super::EXIT_DIAGNOSTICS);
        }
    };

    match ingot_sandbox::detect() {
        Ok(runtime) => eprintln!("runtime   {} {}", runtime.program, runtime.version),
        Err(error) => eprintln!("runtime   unavailable — {error}"),
    }

    if config.json {
        let text = serde_json::to_string_pretty(&plans)?;
        println!("{text}");
    } else {
        print_plans(&plans, &config.workspace);
    }

    Ok(super::EXIT_OK)
}

fn print_plans(plans: &[SandboxPlan], workspace: &Path) {
    println!("workspace  {}", workspace.display());
    println!();
    for plan in plans {
        println!("{}", render(plan));
        println!();
    }

    let unenforced: usize = plans
        .iter()
        .filter(|plan| !plan.is_fully_enforced())
        .count();
    if unenforced == 0 {
        println!("every policy rule above is enforced by the boundary");
    } else {
        println!(
            "{unenforced} of {} boundaries leave something unenforced; \
             `ingot run --sandbox` refuses these unless you pass --sandbox-allow-unenforced",
            plans.len()
        );
    }
}

/// Every boundary this program needs, keyed by the server and the agent.
pub type Plans = BTreeMap<(String, String), SandboxPlan>;

/// Plan a boundary for each (server, agent) pair.
///
/// `filter_egress` says whether the caller will start an
/// [`ingot_sandbox::EgressBoundary`] and
/// route these servers through it. It decides only whether a host allowlist is
/// reported as unenforceable, so passing `true` without starting the proxy
/// would replace a true statement with a false one.
pub fn plan_all_with(
    compilation: &Compilation,
    mcp: &McpConfig,
    workspace: &Path,
    filter_egress: bool,
) -> Result<Plans, Vec<String>> {
    let mut plans = Plans::new();
    let mut problems = Vec::new();

    for (server, agent) in pairs(compilation, mcp) {
        match ingot_sandbox::plan(
            agent,
            &server.name,
            workspace,
            &server.pass_env,
            filter_egress,
        ) {
            Ok(plan) => {
                plans.insert((server.name.clone(), agent.agent.clone()), plan);
            }
            Err(error) => problems.push(format!(
                "agent {} cannot be contained: {error}",
                agent.agent
            )),
        }
    }

    if problems.is_empty() {
        Ok(plans)
    } else {
        Err(problems)
    }
}

/// Plan with no egress filtering, for callers that only inspect.
pub fn plan_all(
    compilation: &Compilation,
    mcp: &McpConfig,
    workspace: &Path,
) -> Result<Plans, Vec<String>> {
    plan_all_with(compilation, mcp, workspace, false)
}

/// The hosts every plan in a set names, sorted and deduplicated.
///
/// One proxy serves every server in a run, so its list is the union. That is
/// wider than any single agent's grant, and it is why the compile-time check in
/// [RFC-0014] matters: the boundary bounds the run, and the compiler bounds each
/// agent within it.
///
/// [RFC-0014]: ../../../rfcs/0014-a-capabilitys-reach.md
pub fn allowed_hosts(plans: &Plans) -> Vec<String> {
    let mut hosts: Vec<String> = plans
        .values()
        .filter_map(|plan| match &plan.network {
            ingot_sandbox::Network::Hosts { hosts } => Some(hosts.clone()),
            _ => None,
        })
        .flatten()
        .collect();
    hosts.sort();
    hosts.dedup();
    hosts
}

/// Starts each tool server inside the boundary planned for the calling agent.
pub struct ContainerLauncher {
    runtime: Runtime,
    /// Held for the launcher's lifetime, so the network and the proxy outlive
    /// every server that routes through them and are removed when it drops.
    egress: Option<ingot_sandbox::EgressBoundary>,
    workspace: PathBuf,
    plans: Plans,
}

impl ContainerLauncher {
    pub fn new(runtime: Runtime, workspace: PathBuf, plans: Plans) -> ContainerLauncher {
        ContainerLauncher {
            runtime,
            egress: None,
            workspace,
            plans,
        }
    }

    /// Route every server this launches through `boundary`.
    ///
    /// Taken by value so the launcher owns it: the network and the proxy have to
    /// outlive every container that joined them, and a boundary dropped early
    /// would take the network out from under a running server.
    pub fn through(mut self, boundary: ingot_sandbox::EgressBoundary) -> ContainerLauncher {
        self.egress = Some(boundary);
        self
    }
}

impl Launcher for ContainerLauncher {
    fn launch(
        &self,
        server: &ServerConfig,
        agent: &str,
        _cwd: &Path,
    ) -> Result<Box<dyn Transport + Send>, String> {
        let plan = self
            .plans
            .get(&(server.name.clone(), agent.to_string()))
            .ok_or_else(|| {
                format!(
                    "no boundary was planned for `{}` and agent `{agent}`",
                    server.name
                )
            })?;

        let image = server.image.as_deref().ok_or_else(|| {
            ExecutorError::NoImage {
                server: server.name.clone(),
            }
            .to_string()
        })?;

        // A write grant is how an artifact says "put the output here", so the
        // directory is created rather than the run failing on its absence.
        for directory in plan.directories_to_create() {
            std::fs::create_dir_all(directory)
                .map_err(|error| format!("creating {}: {error}", directory.display()))?;
        }

        let mut command = vec![server.command.clone()];
        command.extend(server.args.iter().cloned());
        let args = invocation(
            plan,
            image,
            &command,
            &self.workspace,
            self.egress.as_ref().map(|boundary| boundary.route()),
        );

        // The named variables have to reach the runtime's own environment for
        // `--env NAME` to forward them, and nowhere else.
        ChildTransport::spawn(
            &self.runtime.program,
            &args,
            Some(&self.workspace),
            &plan.env,
        )
        .map(|transport| Box::new(transport) as Box<dyn Transport + Send>)
        .map_err(|error| error.to_string())
    }

    fn describe(&self) -> String {
        format!(
            "tool servers run contained, by {} {}; the policy is enforced",
            self.runtime.program, self.runtime.version
        )
    }
}

/// Which servers would serve which agents.
///
/// One plan per pair, not per server: a program's agents deliberately differ,
/// and a box wide enough for all of them would hand each the others' grants.
fn pairs<'a>(
    compilation: &'a Compilation,
    mcp: &'a McpConfig,
) -> Vec<(&'a ServerConfig, &'a AgentIr)> {
    let mut pairs = Vec::new();
    for server in &mcp.servers {
        for agent in &compilation.agents {
            if agent_needs(agent, server) {
                pairs.push((server, agent));
            }
        }
    }
    pairs
}

/// Whether this agent holds a tool this server could serve.
///
/// With an explicit alias map the answer is exact. Without one, the server's
/// tool list is the only way to know, and reading it means starting the server
/// — which an inspection command should not do. So assume it could, and say so
/// by planning for it.
fn agent_needs(agent: &AgentIr, server: &ServerConfig) -> bool {
    if agent.tools.is_empty() {
        return false;
    }
    if server.tools.is_empty() {
        return true;
    }
    agent
        .tools
        .iter()
        .any(|tool| server.tools.contains_key(&tool.name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ingot_ir::{ToolBinding, ToolSignature};

    fn server(name: &str, tools: &[(&str, &str)]) -> ServerConfig {
        ServerConfig {
            name: name.to_string(),
            command: "unused".to_string(),
            url: None,
            auth_env: None,
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

    fn agent_holding(tools: &[&str]) -> AgentIr {
        let mut ir = ingot_ir::AgentIr::from_json(
            r#"{"irVersion":"0.1","language":"0.1","agent":"test.A","inputs":{},"outputs":{},
                "types":{},"requirements":{"model":{"mode":"unspecified"}},"tools":[],
                "state":{},"budget":{},"policy":{},"effects":[],"nodes":[]}"#,
        )
        .expect("the fixture must parse");
        ir.tools = tools
            .iter()
            .map(|name| ToolBinding {
                reference: format!("mcp:{name}"),
                name: (*name).to_string(),
                transport: "mcp".to_string(),
                effects: Vec::new(),
                scopes: std::collections::BTreeMap::new(),
                signature: ToolSignature {
                    params: Vec::new(),
                    result: "text".to_string(),
                },
            })
            .collect();
        ir
    }

    #[test]
    fn an_agent_with_no_tools_needs_no_boundary() {
        assert!(!agent_needs(&agent_holding(&[]), &server("files", &[])));
    }

    #[test]
    fn an_explicit_alias_map_decides_exactly() {
        let mapped = server("files", &[("repo.read_file", "fs.read_file")]);
        assert!(agent_needs(&agent_holding(&["repo.read_file"]), &mapped));
        assert!(!agent_needs(&agent_holding(&["web.search"]), &mapped));
    }

    #[test]
    fn without_a_map_a_server_is_assumed_to_serve_rather_than_started_to_find_out() {
        let unmapped = server("files", &[]);
        assert!(agent_needs(&agent_holding(&["anything"]), &unmapped));
    }
}
