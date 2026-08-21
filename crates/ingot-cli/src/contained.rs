//! `ingot run --contained`, and the `ingot exec` that answers it.
//!
//! Both halves live here because they are one feature and their agreement is the
//! whole correctness question: what the host puts in the boundary has to be what
//! the guest expects to find, and reading them side by side is the only way to
//! keep that true.
//!
//! See [RFC-0005](../../../rfcs/0005-the-contained-run.md).

use std::collections::{BTreeMap, BTreeSet};
use std::process::Command;

use anyhow::{anyhow, bail, Context, Result};
use ingot_compiler::Compilation;
use ingot_ir::{AgentIr, NodeKind};
use ingot_mcp::{McpConfig, McpToolHost};
use ingot_runtime::{
    run as run_agent, AgentRegistry, DenyAllTools, HumanChannel, ModelConfig, ModelProvider,
    RunOptions, RunReport, ToolHost,
};
use ingot_sandbox::{Network, SandboxPlan, RUN_SUBJECT};
use ingot_supervisor::host::{supervise, Deadlines, Outcome, Supervisor};
use ingot_supervisor::protocol::RunConfig as WireConfig;
use ingot_supervisor::{Guest, PROTOCOL_VERSION};
use serde_json::Value;

use crate::run::RunConfig;

/// The command the guest half is invoked as, inside the image.
const GUEST_COMMAND: &[&str] = &["ingot", "exec"];

/// Whether the supervised run gets a boundary.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Containment {
    /// `--contained`: a container derived from the agent's own policy.
    Bounded,
    /// `--supervised`: the same protocol, an ordinary child process, and no
    /// boundary whatsoever. For proving the channel works where there is no
    /// container runtime, and for leaving one variable when a contained run
    /// misbehaves.
    Unbounded,
}

// --- the host half ----------------------------------------------------------

/// Decide the boundary and how the guest will be started.
///
/// Separate from [`execute`], and called before it, because everything here is
/// decided from the **artifact**: whether these agents can share a box, whether
/// their policy is enforceable, what gets mounted. A program that cannot be
/// contained has to say so whether or not an API key happens to be exported,
/// which means this must not wait for a provider to be chosen.
pub fn prepare(
    compilation: &Compilation,
    config: &RunConfig,
    mode: Containment,
    entry: &AgentIr,
) -> Result<Command> {
    // A persistent store is a file outside the box, and nothing that crosses the
    // boundary is a file this side may write. Running anyway would silently start
    // from the declared values and throw away everything written, which is
    // `--no-memory` without anyone asking for it.
    if !entry.persistent.is_empty() && config.memory_mode != crate::memory::MemoryMode::Disabled {
        anyhow::bail!(
            "`{}` declares persistent memory, which a contained run cannot reach\n  \
             the store is a file outside the boundary, and what crosses it is the artifact, \
             the inputs, the tool configuration and the prices\n  \
             help: `--no-memory` runs from the declared values and discards what is written",
            entry.agent
        );
    }

    // What is in force is stated before anything starts, never inferred from
    // which flags the operator remembered.
    match mode {
        Containment::Bounded => {
            let plan = plan_for_run(compilation, config, &entry.agent)?;
            let command = contained_command(config, &plan)?;
            eprintln!("{}", ingot_sandbox::render(&plan));
            eprintln!(
                "the agent runs inside that boundary; the model call and the approval gate cross \
                 out through the supervisor"
            );
            Ok(command)
        }
        // No plan, and deliberately none. There is no boundary here, so
        // refusing over a rule a boundary could not honour would be theatre:
        // nothing is enforced either way, and the warning says so.
        Containment::Unbounded => {
            eprintln!(
                "warning: --supervised runs the agent as an ordinary child process\n         \
                 the policy is checked, and nothing is enforced. This is not a boundary."
            );
            unbounded_command(config)
        }
    }
}

/// Start the guest and answer it until it reports an outcome.
pub fn execute(
    mut command: Command,
    compilation: &Compilation,
    config: &RunConfig,
    entry: &AgentIr,
    inputs: BTreeMap<String, Value>,
    provider: &mut dyn ModelProvider,
    approval: &mut HumanChannel,
) -> Result<u8> {
    let wire = WireConfig {
        protocol: PROTOCOL_VERSION,
        agent: entry.agent.clone(),
        agents: compilation.agents.clone(),
        inputs,
        max_steps: config.max_steps,
        mcp: config.mcp.clone(),
        provider: provider.name().to_string(),
        pricing: pricing_for(&compilation.agents, &config.models),
    };

    let mut printer = crate::run::printer_for(config, compilation, true);
    let mut supervisor = Supervisor {
        config: wire,
        provider,
        approval,
    };
    let outcome = supervise(
        &mut command,
        &mut supervisor,
        &mut |event| printer.print(event),
        deadlines(config),
    )
    .map_err(|error| anyhow!("{error}"))?;

    match outcome {
        Outcome::Finished(finished) => {
            let report = RunReport {
                agent: finished.agent,
                outputs: finished.outputs,
                // A contained run opens no store, so there is nothing to hand
                // back. `prepare` refuses an artifact that declares one.
                memory: Default::default(),
                // The supervisor channel carries a finished run or a failed
                // one. Stopping is not one of the outcomes it can report, so a
                // contained run never stops.
                stopped: None,
                usage: finished.usage,
                steps: finished.steps,
                // The guest charged the budget, because the guest holds the
                // interpreter; the ledger crosses back so the run says what it
                // cost either way round. Nothing is recomputed out here — there
                // is nothing to recompute it from, and a second arithmetic would
                // be a second answer.
                spend: finished.spend,
            };
            printer.finish_record(crate::runs::Outcome::Finished {
                steps: report.steps,
                usage: report.usage,
                cost: report.spend.rendered(),
            });
            crate::run::report_cost(&report);
            crate::run::write_outputs(&report, config)?;
            Ok(super::EXIT_OK)
        }
        Outcome::Failed(failed) => {
            printer.finish_record(crate::runs::Outcome::Failed {
                reason: &failed.reason,
            });
            eprintln!("error: {}", failed.reason);
            if failed.operator_error {
                eprintln!(
                    "hint: this is a problem with how the run was invoked, not with the agent itself"
                );
            }
            Ok(super::EXIT_DIAGNOSTICS)
        }
    }
}

/// How long the host waits for a guest that owes it a message.
///
/// An explicit ceiling — `--timeout` or `[run] timeout-seconds` — is taken as
/// stated. Otherwise it is derived from `[mcp] timeout-seconds`: the longest a
/// guest can legitimately be silent is one tool call inside the box, and that is
/// the bound the guest already honours, so deriving it means nobody has to keep
/// two numbers in step by hand.
fn deadlines(config: &RunConfig) -> Deadlines {
    match config.timeout_seconds {
        Some(seconds) => Deadlines::explicit(seconds),
        None => Deadlines::derived(config.mcp.timeout_seconds),
    }
}

/// The prices the run inside is charged against.
///
/// Handed over only when some agent that crossed states a `cost` budget. The
/// condition is the interpreter's own — [`ingot_runtime`] charges nothing where
/// `budget.cost` is absent — and it is asked over exactly the agents that were
/// sent, so it cannot be right out here and wrong in there. A program that
/// bounds no cost therefore puts nothing extra in the box, and one that bounds a
/// cost gets whatever the manifest configured, **including nothing**: a run given
/// no prices charges none, records every model it could not price, and says so.
/// That last part is the half of [GAP-048] that was not about arithmetic at all.
///
/// [GAP-048]: ../../../docs/gaps.md#gap-048
fn pricing_for(agents: &[AgentIr], models: &ModelConfig) -> ingot_runtime::price::Pricing {
    if agents.iter().any(|agent| agent.budget.cost.is_some()) {
        models.pricing()
    } else {
        Default::default()
    }
}

/// The boundary a contained run gets.
///
/// One agent's policy, not the union of several. A program whose agents would get
/// different boundaries is refused rather than run in the widest of them: a
/// sub-agent holding a grant its own policy denies is exactly the failure this
/// feature exists to prevent, and it would be invisible.
pub fn plan_for_run(
    compilation: &Compilation,
    config: &RunConfig,
    entry: &str,
) -> Result<SandboxPlan> {
    // The tool servers now run inside, so whatever they were promised has to
    // cross the boundary with them.
    let pass_env = crossing_env(&config.mcp);

    let mut plans: Vec<SandboxPlan> = Vec::new();
    for agent in &compilation.agents {
        let plan = ingot_sandbox::plan(agent, RUN_SUBJECT, &config.workspace, &pass_env, false)
            .map_err(|error| {
                anyhow!(
                    "agent {} cannot be contained: {error}\n\
                     hint: a policy path is relative to the workspace ({})",
                    agent.agent,
                    config.workspace.display()
                )
            })?;
        plans.push(plan);
    }

    let entry_plan = plans
        .iter()
        .find(|plan| plan.agent == entry)
        .cloned()
        .ok_or_else(|| anyhow!("the program does not declare `{entry}`"))?;

    // Only agents this one can actually reach. A program with a second agent
    // nobody calls has no widening to worry about, and refusing it would be a
    // false alarm.
    let reachable = reachable_from(compilation, entry);
    let divergent: Vec<&SandboxPlan> = plans
        .iter()
        .filter(|plan| reachable.contains(&plan.agent) && !same_boundary(plan, &entry_plan))
        .collect();

    if !divergent.is_empty() {
        let mut message = String::from(
            "this program's agents do not share one boundary, so containing the run would widen \
             a policy\n",
        );
        for plan in std::iter::once(&entry_plan).chain(divergent.iter().copied()) {
            message.push_str(&format!("  {:<28}{}\n", plan.agent, summarise(plan)));
        }
        message.push_str(
            "\n  one box cannot hold both without giving an agent a grant its own policy denies\n  \
             run with --sandbox instead, which gives each agent's tool servers their own boundary",
        );
        bail!(message);
    }

    let unenforced: Vec<String> = entry_plan
        .unenforceable
        .iter()
        .map(|note| format!("  {}\n    {}", note.policy, note.reason))
        .collect();
    if !unenforced.is_empty() && !config.sandbox_allow_unenforced {
        bail!(
            "the boundary cannot honour every rule this agent states:\n{}\n\n\
             tighten the policy, or pass --sandbox-allow-unenforced to proceed knowing which \
             limits are advisory",
            unenforced.join("\n")
        );
    }
    for note in &unenforced {
        eprintln!("warning: proceeding with an unenforced rule\n{note}");
    }

    Ok(entry_plan)
}

/// Every environment variable name any configured server was promised.
fn crossing_env(mcp: &McpConfig) -> Vec<String> {
    let mut names: Vec<String> = mcp
        .servers
        .iter()
        .flat_map(|server| server.pass_env.iter().cloned())
        .collect();
    names.sort();
    names.dedup();
    names
}

/// Whether two plans grant the same reach. `unenforceable` is deliberately not
/// compared: it is a note about the policy, not part of the boundary.
fn same_boundary(left: &SandboxPlan, right: &SandboxPlan) -> bool {
    let mounts = |plan: &SandboxPlan| -> BTreeSet<(String, bool)> {
        plan.mounts
            .iter()
            .map(|mount| (mount.guest.clone(), mount.writable))
            .collect()
    };
    mounts(left) == mounts(right) && left.network == right.network
}

fn summarise(plan: &SandboxPlan) -> String {
    let mut parts: Vec<String> = plan
        .mounts
        .iter()
        .map(|mount| {
            format!(
                "{} {}",
                mount.guest,
                if mount.writable { "rw" } else { "ro" }
            )
        })
        .collect();
    parts.push(match &plan.network {
        Network::None => "no network".to_string(),
        Network::Unrestricted => "network".to_string(),
        Network::Hosts { hosts } => format!("network ({})", hosts.join(", ")),
    });
    parts.join(", ")
}

/// Agents `entry` can reach, including itself.
fn reachable_from(compilation: &Compilation, entry: &str) -> BTreeSet<String> {
    let mut reached: BTreeSet<String> = BTreeSet::new();
    let mut pending = vec![entry.to_string()];

    while let Some(name) = pending.pop() {
        if !reached.insert(name.clone()) {
            continue;
        }
        let Some(agent) = compilation.agents.iter().find(|agent| agent.agent == name) else {
            continue;
        };
        for node in &agent.nodes {
            if node.kind == NodeKind::AgentCall {
                if let Some(callee) = &node.agent {
                    pending.push(callee.clone());
                }
            }
        }
    }
    reached
}

/// `docker run …  <image> ingot exec`.
fn contained_command(config: &RunConfig, plan: &SandboxPlan) -> Result<Command> {
    let image = config
        .image
        .clone()
        .unwrap_or_else(crate::image::reference_image);

    let runtime = ingot_sandbox::detect().map_err(|error| anyhow!("{error}"))?;
    eprintln!("runtime   {} {}", runtime.program, runtime.version);

    match ingot_sandbox::image_exists(&runtime, &image) {
        Ok(true) => {}
        Ok(false) => bail!(crate::image::missing_image(&image)),
        Err(error) => return Err(anyhow!("{error}")),
    }

    // A pinned reference names bytes rather than a label, so it is checked
    // before the boundary is built. Acquisition stays manual either way: a pull
    // becomes automatic only once there is a signature and a trust root to check
    // it against.
    if crate::image::pinned_digest(&image).is_some() {
        let present =
            ingot_sandbox::image_digests(&runtime, &image).map_err(|error| anyhow!("{error}"))?;
        crate::image::verify_pin(&image, &present)?;
        eprintln!("image     {image} (digest verified)");
    }

    // A write grant is how an artifact says "put the output here", so the
    // directory is created rather than the run failing on its absence.
    for directory in plan.directories_to_create() {
        std::fs::create_dir_all(directory)
            .with_context(|| format!("creating {}", directory.display()))?;
    }

    let guest: Vec<String> = GUEST_COMMAND.iter().map(|part| part.to_string()).collect();
    let args = ingot_sandbox::invocation(plan, &image, &guest, &config.workspace, None);

    let mut command = Command::new(&runtime.program);
    command.args(&args);
    Ok(command)
}

/// This same binary, as an ordinary child process. No boundary.
fn unbounded_command(config: &RunConfig) -> Result<Command> {
    let program = std::env::current_exe().context("finding this executable")?;
    let mut command = Command::new(program);
    command.arg("exec");
    // The working directory is where a tool server's `--root .` resolves, and
    // inside a boundary that is `/workspace`. Matching it here keeps the two
    // paths comparable, which is the only reason `--supervised` is useful.
    command.current_dir(&config.workspace);
    Ok(command)
}

// --- the guest half ---------------------------------------------------------

/// `ingot exec`: be the inside of a supervised run.
///
/// Takes no arguments and reads no files. Everything comes down the channel,
/// which is what keeps the boundary containing only the paths the policy named.
pub fn exec() -> Result<u8> {
    let guest = Guest::on_stdio();

    let config = guest
        .config()
        .map_err(|error| anyhow!("{error}\nhint: `ingot exec` is the inside half of `ingot run --contained`; it is not a way to run an agent"))?;

    let registry: AgentRegistry = config
        .agents
        .iter()
        .map(|agent| (agent.agent.clone(), agent.clone()))
        .collect();

    let Some(ir) = registry.get(&config.agent).cloned() else {
        let available: Vec<&str> = registry.keys().map(String::as_str).collect();
        guest
            .fail_with(
                &format!(
                    "the supervisor asked for `{}`, which was not among the {} agent(s) it sent: {}",
                    config.agent,
                    registry.len(),
                    available.join(", ")
                ),
                false,
            )
            .map_err(|error| anyhow!("{error}"))?;
        return Ok(super::EXIT_DIAGNOSTICS);
    };

    let mut tools = match guest_tools(&config) {
        Ok(tools) => tools,
        Err(reason) => {
            // A tool server that will not start is the operator's to fix, and
            // saying so through the channel means the message reaches their
            // terminal rather than dying with the container.
            guest
                .fail_with(&reason, true)
                .map_err(|error| anyhow!("{error}"))?;
            return Ok(super::EXIT_DIAGNOSTICS);
        }
    };

    let mut provider = guest.provider(&config.provider);
    let mut events = guest.events();

    let result = run_agent(
        &ir,
        &registry,
        &mut provider,
        tools.as_mut(),
        &mut events,
        RunOptions {
            inputs: config.inputs.clone(),
            // Always `Ask`. The decision is the host's; this side only carries
            // the question, so `--yes` and an unattended deny are both applied
            // out there where the operator is.
            approval: HumanChannel::Ask(Box::new(guest.approvals())),
            max_steps: config.max_steps,
            // The store is a file outside the box, and what crosses the
            // boundary is the artifact, the inputs, the tool configuration and
            // the prices — never a file this side may write. `prepare` refuses
            // before it gets here.
            memory: std::collections::BTreeMap::new(),
            stop_at: None,
            resume: None,
            // The prices the host was configured with. They cross because the
            // boundary bounds effects rather than arithmetic, and a `cost`
            // ceiling that holds outside and not inside is the asymmetry
            // GAP-048 recorded. Empty is still a correct answer: a run with no
            // prices charges nothing and says so, in here as out there.
            pricing: config.pricing.clone(),
            // No factory, so a fan-out inside the box has a ceiling of one. The
            // provider here *is* the supervisor channel -- request and reply over
            // one pair of pipes -- so there is nothing a second instance could
            // be, and the sequential answer falls out rather than being decided.
            fan_out: Default::default(),
        },
    );

    match result {
        Ok(report) => guest
            .finished(&report)
            .map_err(|error| anyhow!("{error}"))?,
        Err(error) => guest.failed(&error).map_err(|error| anyhow!("{error}"))?,
    }
    Ok(super::EXIT_OK)
}

/// Tool servers, started inside the boundary as children of this process.
///
/// They inherit the boundary rather than getting one of their own, which is the
/// same guarantee by a shorter route: they are already inside a box built from
/// the policy they would otherwise have been given.
fn guest_tools(config: &WireConfig) -> Result<Box<dyn ToolHost + Send>, String> {
    if config.mcp.is_empty() {
        return Ok(Box::new(DenyAllTools));
    }

    let mut mcp = config.mcp.clone();
    for server in &mut mcp.servers {
        // Both describe where a server sits on the *host*, and there is no host
        // in here. Warned about rather than dropped in silence: an operator who
        // set `image` expects it to mean something.
        if server.image.take().is_some() {
            eprintln!(
                "warning: server `{}` has an `image`, which a contained run ignores — \
                 it already runs inside one",
                server.name
            );
        }
        if server.cwd.take().is_some() {
            eprintln!(
                "warning: server `{}` has a `cwd`, which a contained run ignores — \
                 the working directory is the workspace",
                server.name
            );
        }
    }

    let required: BTreeSet<String> = config
        .agents
        .iter()
        .flat_map(|agent| agent.tools.iter())
        .filter(|tool| tool.transport == "mcp")
        .map(|tool| tool.name.clone())
        .collect();

    let cwd = std::env::current_dir().map_err(|error| format!("reading the workspace: {error}"))?;
    let host = McpToolHost::connect(&mcp, &cwd, &required).map_err(|error| {
        format!(
            "{error}\nhint: a contained run starts its tool servers inside the image, so the \
             image must contain them"
        )
    })?;

    eprintln!("{}", host.launcher());
    for tool in host.resolved() {
        eprintln!("tool {} <- {}:{}", tool.tool, tool.server, tool.remote);
    }
    for missing in host.unresolved(&required) {
        eprintln!("warning: no configured server provides `{missing}`");
    }
    Ok(Box::new(host))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ingot_ir::{Decision, Node, PolicyRule};

    fn agent(name: &str, policy: &[(&str, Decision, &[&str])], calls: &[&str]) -> AgentIr {
        let mut ir = AgentIr::from_json(
            r#"{"irVersion":"0.1","language":"0.1","agent":"x","inputs":{},"outputs":{},
                "types":{},"requirements":{"model":{"mode":"unspecified"}},"tools":[],
                "state":{},"budget":{},"policy":{},"effects":[],"nodes":[]}"#,
        )
        .expect("the fixture must parse");
        ir.agent = name.to_string();
        for (subject, decision, values) in policy {
            ir.policy.insert(
                (*subject).to_string(),
                PolicyRule {
                    decision: *decision,
                    values: values.iter().map(|v| (*v).to_string()).collect(),
                    qualifier: None,
                },
            );
        }
        for (index, callee) in calls.iter().enumerate() {
            let mut node = Node::new(format!("n{index}"), NodeKind::AgentCall);
            node.agent = Some((*callee).to_string());
            ir.nodes.push(node);
        }
        ir
    }

    fn compilation(agents: Vec<AgentIr>) -> Compilation {
        let mut compilation =
            ingot_compiler::compile_source("test.ing".to_string(), "language 0.1\n".to_string());
        compilation.agents = agents;
        compilation
    }

    #[test]
    fn an_agent_reaches_itself_and_whatever_it_calls_transitively() {
        let program = compilation(vec![
            agent("p.Leaf", &[], &[]),
            agent("p.Middle", &[], &["p.Leaf"]),
            agent("p.Top", &[], &["p.Middle"]),
            agent("p.Unrelated", &[], &[]),
        ]);
        let reached = reachable_from(&program, "p.Top");
        assert!(reached.contains("p.Top"));
        assert!(reached.contains("p.Middle"));
        assert!(reached.contains("p.Leaf"));
        assert!(
            !reached.contains("p.Unrelated"),
            "an agent nobody calls cannot widen anything: {reached:?}"
        );
    }

    #[test]
    fn a_cycle_in_the_call_graph_terminates() {
        let program = compilation(vec![
            agent("p.A", &[], &["p.B"]),
            agent("p.B", &[], &["p.A"]),
        ]);
        assert_eq!(reachable_from(&program, "p.A").len(), 2);
    }

    #[test]
    fn two_plans_granting_the_same_reach_are_the_same_boundary() {
        let mut left = SandboxPlan {
            agent: "p.A".into(),
            server: RUN_SUBJECT.into(),
            mounts: Vec::new(),
            network: Network::None,
            env: Vec::new(),
            workdir: "/workspace".into(),
            unenforceable: Vec::new(),
        };
        let mut right = left.clone();
        right.agent = "p.B".into();
        assert!(same_boundary(&left, &right));

        // A note about the policy is not part of the boundary.
        right.unenforceable = vec![ingot_sandbox::Unenforceable {
            policy: "external_write allow".into(),
            reason: "x".into(),
        }];
        assert!(same_boundary(&left, &right));

        // A network is.
        left.network = Network::Unrestricted;
        assert!(!same_boundary(&left, &right));
    }

    #[test]
    fn the_environment_that_crosses_is_the_union_of_what_the_servers_were_promised() {
        let mut mcp = McpConfig::default();
        for (name, env) in [("a", vec!["Z", "A"]), ("b", vec!["A"])] {
            mcp.servers.push(ingot_mcp::ServerConfig {
                name: name.to_string(),
                command: "x".to_string(),
                url: None,
                auth_env: None,
                args: Vec::new(),
                image: None,
                cwd: None,
                pass_env: env.iter().map(|n| n.to_string()).collect(),
                tools: BTreeMap::new(),
            });
        }
        assert_eq!(
            crossing_env(&mcp),
            vec!["A".to_string(), "Z".to_string()],
            "sorted and deduplicated, so the invocation is the same every run"
        );
    }

    #[test]
    fn the_prices_cross_only_when_an_agent_states_a_cost_ceiling() {
        let priced = ModelConfig {
            prices: vec![ingot_runtime::price::ModelPrice {
                model: "claude-opus-5".into(),
                input: "3".into(),
                output: "15".into(),
                cache_read: None,
                currency: "usd".into(),
            }],
            ..Default::default()
        };

        // Nobody bounded a cost, so nothing needs pricing and nothing goes in.
        let unbounded = vec![agent("p.A", &[], &[]), agent("p.B", &[], &[])];
        assert!(pricing_for(&unbounded, &priced).is_empty());

        // One agent does — and it need not be the entry, because any of them
        // may be the one that runs or the one that is called.
        let mut bounded = unbounded.clone();
        bounded[1].budget.cost = Some(ingot_ir::Cost {
            amount: "5".into(),
            currency: "usd".into(),
        });
        assert_eq!(
            pricing_for(&bounded, &priced).models().collect::<Vec<_>>(),
            vec!["claude-opus-5"]
        );

        // A ceiling with no prices behind it still crosses as nothing, and the
        // run inside says so rather than charging against an empty table.
        assert!(pricing_for(&bounded, &ModelConfig::default()).is_empty());
    }

    #[test]
    fn a_summary_names_the_mounts_and_the_network() {
        let plan = SandboxPlan {
            agent: "p.A".into(),
            server: RUN_SUBJECT.into(),
            mounts: vec![ingot_sandbox::Mount {
                path: "src".into(),
                host: std::path::PathBuf::from("/srv/src"),
                guest: "/workspace/src".into(),
                writable: false,
                from: "filesystem_read allow [\"src\"]".into(),
            }],
            network: Network::None,
            env: Vec::new(),
            workdir: "/workspace".into(),
            unenforceable: Vec::new(),
        };
        let text = summarise(&plan);
        assert!(text.contains("/workspace/src ro"), "{text}");
        assert!(text.contains("no network"), "{text}");
    }
}
