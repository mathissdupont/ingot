//! `ingot run` and `ingot test`.
//!
//! Both compile the source, then hand the resulting IR to the reference
//! interpreter. `run` executes against a provider the operator chose; `test`
//! replays recorded cassettes so the suite works with no API key and no network.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use ingot_compiler::Compilation;
use ingot_mcp::{AgentTools, McpConfig, McpToolHost};
use ingot_runtime::{
    run as run_agent, AgentRegistry, ApprovalHandler, ApprovalMode, ApprovalRequest, Artifact,
    Cassette, DenyAllTools, EventSink, ModelConfig, ModelProvider, RecordingProvider,
    RecordingTools, ReplayProvider, ReplayToolHost as ReplayTools, RoutingProvider, RunError,
    RunEvent, RunOptions, RunReport, ToolHost,
};
use serde_json::Value;

use crate::contained::Containment;

/// The transport an artifact declares for an MCP tool. The only one in
/// language 0.1, and the only one `ingot-mcp` serves.
const MCP_TRANSPORT: &str = "mcp";

/// Which model provider to run against.
#[derive(Copy, Clone, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum ProviderChoice {
    /// Route by the vendor the artifact pins, using whichever keys are set.
    Auto,
    /// Call the Anthropic Messages API. Needs `ANTHROPIC_API_KEY`.
    Anthropic,
    /// Call an OpenAI-compatible Chat Completions endpoint. Needs
    /// `OPENAI_API_KEY`; `INGOT_OPENAI_BASE_URL` points it elsewhere.
    Openai,
    /// Call Google's Generative Language API. Needs `GEMINI_API_KEY` (or
    /// `GOOGLE_API_KEY`); `INGOT_GOOGLE_BASE_URL` points it elsewhere.
    Google,
    /// Replay a recorded cassette. No network, no key, same answers every time.
    Replay,
}

/// How agent output is printed.
#[derive(Copy, Clone, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum EventFormat {
    /// Human-readable lines.
    Text,
    /// One JSON object per line, for piping. Every event carries an `event`
    /// key; a live run also writes lines without one, which are the model's
    /// text as it arrives and are not part of the event stream.
    Json,
    /// Nothing but the final artifacts.
    Quiet,
}

pub struct RunConfig {
    pub inputs: Vec<String>,
    pub provider: ProviderChoice,
    pub cassette: Option<PathBuf>,
    pub record: Option<PathBuf>,
    /// Read only by the HTTP providers, so unused in a build without them.
    #[cfg_attr(not(feature = "providers"), allow(dead_code))]
    pub model: Option<String>,
    #[cfg_attr(not(feature = "providers"), allow(dead_code))]
    pub effort: Option<String>,
    pub agent: Option<String>,
    pub out_dir: Option<PathBuf>,
    pub events: EventFormat,
    pub yes: bool,
    pub max_steps: u32,
    /// The project directory; tool servers start here.
    pub root: PathBuf,
    /// Tool servers declared in the manifest.
    pub mcp: McpConfig,
    /// Start no tool server, whatever the manifest says. For checking that an
    /// agent fails the way it is supposed to when a tool is missing.
    pub no_tools: bool,
    /// Run each tool server inside a boundary derived from the policy.
    pub sandbox: bool,
    /// Proceed even where the boundary cannot honour a policy rule.
    pub sandbox_allow_unenforced: bool,
    /// The root policy paths are relative to.
    pub workspace: PathBuf,
    /// Model services declared in the manifest, beyond the built-in ones.
    pub models: ModelConfig,
    /// Run the agent itself inside a boundary derived from its policy, with the
    /// model call and the approval gate crossing out through a supervisor.
    pub contained: bool,
    /// Run the agent over the same supervisor channel with no boundary at all.
    /// Hidden, and it says what it is not.
    pub supervised: bool,
    /// The image a contained run happens inside.
    pub image: Option<String>,
    /// How long a contained run may go without a word from inside, when the
    /// operator or the manifest states a ceiling. Absent, it is derived from the
    /// tool timeout the guest already honours.
    pub timeout_seconds: Option<u64>,
}

impl RunConfig {
    /// Whether this run happens over the supervisor channel, and with what.
    pub fn containment(&self) -> Option<Containment> {
        match (self.contained, self.supervised) {
            (true, _) => Some(Containment::Bounded),
            (false, true) => Some(Containment::Unbounded),
            (false, false) => None,
        }
    }

    fn selection(&self) -> ProviderSelection {
        ProviderSelection {
            choice: self.provider,
            cassette: self.cassette.clone(),
            model: self.model.clone(),
            effort: self.effort.clone(),
            models: self.models.clone(),
            strict_replay: true,
        }
    }
}

/// Everything provider construction needs, with no run attached.
///
/// `ingot run` executes an artifact and `ingot new` writes one; both need a
/// model and neither should have to learn the other's configuration to get one.
pub struct ProviderSelection {
    pub choice: ProviderChoice,
    pub cassette: Option<PathBuf>,
    /// Read only by the HTTP providers, so unused in a build without them.
    #[cfg_attr(not(feature = "providers"), allow(dead_code))]
    pub model: Option<String>,
    #[cfg_attr(not(feature = "providers"), allow(dead_code))]
    pub effort: Option<String>,
    pub models: ModelConfig,
    /// Whether a replayed interaction must match the request that recorded it.
    ///
    /// A run replays strictly: an edited prompt must fail loudly rather than be
    /// answered from a stale recording. Authoring replays leniently, because
    /// what an authoring cassette pins is the source a model proposed — which
    /// the compiler then verifies from scratch — and not the toolchain-derived
    /// prompt that asked for it. A cassette that stopped replaying every time
    /// the authoring prompt gained a sentence would test the prompt, not the
    /// guardrails.
    pub strict_replay: bool,
}

/// Build the provider a command asked for.
pub fn build_model_provider(selection: &ProviderSelection) -> Result<Box<dyn ModelProvider>> {
    match selection.choice {
        ProviderChoice::Replay => {
            let Some(path) = &selection.cassette else {
                bail!(
                    "`--provider replay` needs `--cassette <FILE>`\n\
                     record one first with: ingot run --record <FILE>"
                );
            };
            let cassette = Cassette::load(path).map_err(anyhow::Error::msg)?;
            let provider = ReplayProvider::new(cassette);
            Ok(Box::new(if selection.strict_replay {
                provider
            } else {
                provider.lenient()
            }))
        }
        ProviderChoice::Anthropic => anthropic(selection),
        ProviderChoice::Openai => openai(selection),
        ProviderChoice::Google => google(selection),
        ProviderChoice::Auto => auto(selection),
    }
}

/// Compile, execute, and write the artifacts.
pub fn execute(compilation: &Compilation, config: &RunConfig) -> Result<u8> {
    let (ir, registry) = select_agent(compilation, config.agent.as_deref())?;

    let inputs = parse_inputs(&config.inputs)?;
    let mut approval = approval_mode(config);

    // A supervised run is a different arrangement, not a different flag on this
    // one: there is no local tool host, the interpreter is somewhere else, and
    // this process's job is to answer it.
    if let Some(mode) = config.containment() {
        if config.record.is_some() {
            bail!(
                "`--record` cannot be combined with a supervised run\n  \
                 the cassette would record the model exchanges, which happen out here, and omit \
                 the tool results, which happen in there — a recording that claims to be of a \
                 contained run and is not\n  \
                 record without --contained, or replay into one with --provider replay"
            );
        }
        // The boundary is settled from the artifact, before the environment gets
        // a say: a program that cannot be contained must say so whether or not a
        // key happens to be exported.
        let command = crate::contained::prepare(compilation, config, mode, &ir)?;
        let mut provider = build_provider(config, &ir.agent, &inputs)?;
        return crate::contained::execute(
            command,
            compilation,
            config,
            &ir,
            inputs,
            provider.as_mut(),
            &mut approval,
        );
    }

    // Recording wraps the host rather than replacing it, so a recorded run uses
    // exactly the tools an unrecorded one would.
    let mut tools = Tools::new(tool_host(compilation, config)?, config.record.is_some());

    let mut provider = build_provider(config, &ir.agent, &inputs)?;
    let mut sink = RunSink {
        printer: EventPrinter::new(config.events, compilation),
    };

    let result = run_agent(
        &ir,
        &registry,
        provider.as_mut(),
        tools.as_mut(),
        &mut sink,
        RunOptions {
            inputs,
            approval,
            max_steps: config.max_steps,
            pricing: config.models.pricing(),
        },
    );

    // Record whatever happened before propagating a failure: a partial cassette
    // is more useful than none when debugging why a run broke.
    if let Some(path) = &config.record {
        if let Some(mut cassette) = provider.finish_recording() {
            cassette.tool_calls = tools.finish_recording();
            cassette.save(path).map_err(anyhow::Error::msg)?;
            eprintln!(
                "recorded {} interaction(s) and {} tool call(s) to {}",
                cassette.interactions.len(),
                cassette.tool_calls.len(),
                path.display()
            );
        }
    }

    let report = match result {
        Ok(report) => report,
        Err(error) => {
            report_failure(&error);
            return Ok(super::EXIT_DIAGNOSTICS);
        }
    };

    report_cost(&report);
    write_outputs(&report, config)?;
    Ok(super::EXIT_OK)
}

/// One event, in whichever form the operator asked for.
///
/// Shared with a supervised run, whose events arrive over a channel rather than
/// from a sink — the operator should not be able to tell from the output which
/// arrangement produced it.
pub(crate) struct EventPrinter {
    format: EventFormat,
    trace: crate::trace::HumanTrace,
    /// Whether a model is mid-sentence on the current line.
    streaming: bool,
}

impl EventPrinter {
    pub(crate) fn new(format: EventFormat, compilation: &Compilation) -> Self {
        Self {
            format,
            trace: crate::trace::HumanTrace::with_sources(
                &compilation.agents,
                &compilation.sources,
                compilation.file,
            ),
            streaming: false,
        }
    }

    pub(crate) fn print(&mut self, event: &RunEvent) {
        // A model that was mid-sentence has stopped being mid-sentence, and the
        // next line should not land in the middle of it.
        self.close_stream();
        match self.format {
            EventFormat::Text => eprintln!("{}", self.trace.render(event)),
            EventFormat::Json => eprintln!("{}", event.to_json_line()),
            EventFormat::Quiet => {}
        }
    }

    /// A fragment of an answer, as the model produces it.
    ///
    /// Written to the same stream as the trace and never to stdout, which
    /// carries the artifacts: a pipeline reading a run's output must not have
    /// half-finished text spliced into it.
    ///
    /// In JSON these lines carry no `event` key, and that is the contract — the
    /// event stream is the set of lines that have one. A consumer selecting on
    /// `event` sees exactly what a replay would reproduce.
    pub(crate) fn delta(&mut self, node: &str, text: &str) {
        match self.format {
            EventFormat::Text => {
                if !self.streaming {
                    eprint!("        ");
                    self.streaming = true;
                }
                // Indent continuation lines to the same column, so a multi-line
                // answer stays inside the trace rather than breaking out of it.
                eprint!("{}", text.replace('\n', "\n        "));
                let _ = std::io::stderr().flush();
            }
            EventFormat::Json => {
                self.streaming = true;
                eprintln!(
                    "{}",
                    serde_json::json!({ "delta": { "node": node, "text": text } })
                );
            }
            EventFormat::Quiet => {}
        }
    }

    /// The text is finished, and whether it became the answer.
    pub(crate) fn settled(&mut self, node: &str, kept: bool) {
        match self.format {
            EventFormat::Text => {
                self.close_stream();
                if !kept {
                    // Said plainly, because the text above is the beginning of
                    // a real answer and looks like a result.
                    eprintln!("        (discarded: that text is not the answer)");
                }
            }
            EventFormat::Json => {
                self.streaming = false;
                eprintln!(
                    "{}",
                    serde_json::json!({ "settled": { "node": node, "kept": kept } })
                );
            }
            EventFormat::Quiet => {}
        }
    }

    fn close_stream(&mut self) {
        if self.streaming && self.format == EventFormat::Text {
            eprintln!();
        }
        self.streaming = false;
    }
}

/// Where a live run's output goes: the event stream, and the text a model is
/// producing right now.
///
/// Both reach the same printer, and the printer keeps them from colliding on
/// screen. They are still two streams — see [`ingot_runtime::EventSink`].
pub(crate) struct RunSink {
    printer: EventPrinter,
}

impl EventSink for RunSink {
    fn emit(&mut self, event: RunEvent) {
        self.printer.print(&event);
    }

    fn delta(&mut self, node: &str, text: &str) {
        self.printer.delta(node, text);
    }

    fn settled(&mut self, node: &str, kept: bool) {
        self.printer.settled(node, kept);
    }
}

fn report_failure(error: &RunError) {
    eprintln!("error: {error}");
    if error.is_operator_error() {
        eprintln!(
            "hint: this is a problem with how the run was invoked, not with the agent itself"
        );
    }
    if let RunError::CapabilityDenied {
        effect, explicit, ..
    } = error
    {
        if *explicit {
            eprintln!("hint: the artifact's policy denies `{effect}`; rebuild it with the capability granted");
        } else {
            eprintln!(
                "hint: add `{} allow [...]` to the agent's policy block and rebuild",
                policy_subject(effect)
            );
        }
    }
}

fn policy_subject(effect: &str) -> &str {
    match effect {
        "secret_access" => "secrets",
        other => other,
    }
}

/// Pick the agent to run and build the registry of everything it may call.
fn select_agent(
    compilation: &Compilation,
    requested: Option<&str>,
) -> Result<(ingot_ir::AgentIr, AgentRegistry)> {
    if compilation.agents.is_empty() {
        bail!("the program declares no agent");
    }
    let registry: AgentRegistry = compilation
        .agents
        .iter()
        .map(|agent| (agent.agent.clone(), agent.clone()))
        .collect();

    let ir = match requested {
        Some(name) => compilation.agent(name).cloned().with_context(|| {
            let available: Vec<&str> = compilation
                .agents
                .iter()
                .map(|agent| agent.agent.as_str())
                .collect();
            format!(
                "no agent named `{name}`; this file declares: {}",
                available.join(", ")
            )
        })?,
        None => {
            // The last declared agent is the entry point by convention: a
            // coordinator is written after the sub-agents it calls.
            compilation
                .agents
                .last()
                .cloned()
                .expect("checked non-empty above")
        }
    };
    Ok((ir, registry))
}

fn parse_inputs(raw: &[String]) -> Result<BTreeMap<String, Value>> {
    let mut inputs = BTreeMap::new();
    for entry in raw {
        let Some((name, value)) = entry.split_once('=') else {
            bail!("`--input {entry}` is not `name=value`");
        };
        let name = name.trim().to_string();
        let value = value.trim();

        // `@path` reads a file, so a document does not have to fit on a command
        // line. Everything else is JSON if it parses as JSON, and a string
        // otherwise — so `--input topic=compilers` does the obvious thing.
        let parsed = if let Some(path) = value.strip_prefix('@') {
            let text = std::fs::read_to_string(path)
                .with_context(|| format!("reading input file {path}"))?;
            Value::String(text)
        } else {
            serde_json::from_str(value).unwrap_or_else(|_| Value::String(value.to_string()))
        };
        inputs.insert(name, parsed);
    }
    Ok(inputs)
}

fn approval_mode(config: &RunConfig) -> ApprovalMode {
    if config.yes {
        return ApprovalMode::AssumeYes;
    }
    if std::io::stdin().is_terminal() {
        ApprovalMode::Ask(Box::new(TerminalApprovals))
    } else {
        // Unattended runs deny by default. An artifact that asked for a human
        // does not get one silently.
        ApprovalMode::Deny
    }
}

struct TerminalApprovals;

impl ApprovalHandler for TerminalApprovals {
    fn approve(&mut self, request: &ApprovalRequest) -> bool {
        eprintln!();
        eprintln!("  APPROVAL REQUIRED at node {}", request.node);
        eprintln!("  {}", request.reason);
        eprintln!("  effects: {}", request.effects.join(", "));
        eprint!("  allow? [y/N] ");
        let _ = std::io::stderr().flush();

        let mut answer = String::new();
        if std::io::stdin().read_line(&mut answer).is_err() {
            return false;
        }
        matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes")
    }
}

/// Every MCP tool the program declares, across all its agents.
///
/// A superset of what one run needs: which sub-agents a flow reaches is decided
/// at run time, so narrowing this would mean starting a server halfway through
/// a run. Starting one server too many is the cheaper mistake.
pub(crate) fn required_tools(compilation: &Compilation) -> BTreeSet<String> {
    compilation
        .agents
        .iter()
        .flat_map(|agent| agent.tools.iter())
        .filter(|tool| tool.transport == MCP_TRANSPORT)
        .map(|tool| tool.name.clone())
        .collect()
}

/// The same, split by agent, for a host that gives each its own boundary.
fn tools_per_agent(compilation: &Compilation) -> Vec<AgentTools> {
    compilation
        .agents
        .iter()
        .map(|agent| {
            AgentTools::new(
                agent.agent.clone(),
                agent
                    .tools
                    .iter()
                    .filter(|tool| tool.transport == MCP_TRANSPORT)
                    .map(|tool| tool.name.clone())
                    .collect(),
            )
        })
        .collect()
}

/// Build the host that contains each tool server.
///
/// Refuses before starting anything when a boundary cannot honour a rule the
/// artifact states. An operator who switched a sandbox on and believes
/// `network allow ["arxiv.org"]` is in force is worse off than one who knows it
/// is not.
fn contained_host(compilation: &Compilation, config: &RunConfig) -> Result<McpToolHost> {
    let plans = crate::sandbox::plan_all(compilation, &config.mcp, &config.workspace)
        .map_err(|problems| anyhow::anyhow!("{}", problems.join("\n")))?;

    let unenforced: Vec<String> = plans
        .values()
        .filter(|plan| !plan.is_fully_enforced())
        .flat_map(|plan| {
            plan.unenforceable
                .iter()
                .map(move |note| format!("  {} ({})\n    {}", note.policy, plan.agent, note.reason))
        })
        .collect();

    if !unenforced.is_empty() && !config.sandbox_allow_unenforced {
        bail!(
            "the boundary cannot honour every rule this program states:\n{}\n\n\
             run `ingot sandbox` to see the whole picture, tighten the policy, or pass \
             --sandbox-allow-unenforced to proceed knowing which limits are advisory",
            unenforced.join("\n")
        );
    }
    for note in &unenforced {
        eprintln!("warning: proceeding with an unenforced rule\n{note}");
    }

    let runtime = ingot_sandbox::detect().map_err(|error| anyhow::anyhow!("{error}"))?;
    let launcher = crate::sandbox::ContainerLauncher::new(runtime, config.workspace.clone(), plans);

    McpToolHost::connect_agents(
        &config.mcp,
        &config.root,
        &tools_per_agent(compilation),
        &launcher,
    )
    .map_err(|error| anyhow::anyhow!("{error}"))
}

/// Connect to the tool servers this program needs.
///
/// The host is chosen here and nowhere else. When nothing is configured the
/// answer is [`DenyAllTools`], so an artifact that needs a tool stops at the
/// call with a message naming it — never by quietly skipping the step.
fn tool_host(compilation: &Compilation, config: &RunConfig) -> Result<Box<dyn ToolHost>> {
    let required = required_tools(compilation);
    if config.no_tools || config.mcp.is_empty() {
        if !required.is_empty() && !config.no_tools {
            eprintln!(
                "warning: this program declares {} tool(s) and the manifest configures no MCP \
                 server, so any call will stop the run",
                required.len()
            );
            eprintln!("hint: run `ingot tools` to see what is missing");
        }
        return Ok(Box::new(DenyAllTools));
    }

    let host = if config.sandbox {
        contained_host(compilation, config)?
    } else {
        McpToolHost::connect(&config.mcp, &config.root, &required)
            .map_err(|error| anyhow::anyhow!("{error}"))?
    };

    // Say what got wired to what, and whether a boundary is in force. Whether
    // the policy is enforced or merely checked must never be something an
    // operator has to infer from which flags they remembered.
    eprintln!("{}", host.launcher());
    for tool in host.resolved() {
        eprintln!(
            "tool {} <- {}:{}{}",
            tool.tool,
            tool.server,
            tool.remote,
            if tool.aliased { " (aliased)" } else { "" }
        );
    }
    for missing in host.unresolved(&required) {
        eprintln!("warning: no configured server provides `{missing}`");
    }

    Ok(Box::new(host))
}

/// A tool host plus, optionally, the recorder wrapped around it.
///
/// The same shape as [`Provider`], for the same reason: what serves the tools is
/// a deployment decision, so the host stays a trait object, and the recording
/// has to come back out afterwards without a downcast.
enum Tools {
    Plain(Box<dyn ToolHost>),
    Recording(RecordingTools<Box<dyn ToolHost>>),
}

impl Tools {
    fn new(inner: Box<dyn ToolHost>, record: bool) -> Tools {
        if record {
            Tools::Recording(RecordingTools::new(inner))
        } else {
            Tools::Plain(inner)
        }
    }

    fn as_mut(&mut self) -> &mut dyn ToolHost {
        match self {
            Tools::Plain(inner) => inner.as_mut(),
            Tools::Recording(inner) => inner,
        }
    }

    fn finish_recording(self) -> Vec<ingot_runtime::ToolExchange> {
        match self {
            Tools::Plain(_) => Vec::new(),
            Tools::Recording(inner) => inner.finish(),
        }
    }
}

/// A provider plus, optionally, the recorder wrapped around it.
pub(crate) enum Provider {
    Plain(Box<dyn ModelProvider>),
    Recording(RecordingProvider<Box<dyn ModelProvider>>),
}

impl Provider {
    /// Wrap a provider, recording its exchanges under `agent` when asked.
    pub(crate) fn new(inner: Box<dyn ModelProvider>, record: bool, agent: &str) -> Provider {
        if record {
            Provider::Recording(RecordingProvider::new(inner, agent))
        } else {
            Provider::Plain(inner)
        }
    }

    pub(crate) fn as_mut(&mut self) -> &mut dyn ModelProvider {
        match self {
            Provider::Plain(inner) => inner.as_mut(),
            Provider::Recording(inner) => inner,
        }
    }

    pub(crate) fn finish_recording(self) -> Option<Cassette> {
        match self {
            Provider::Plain(_) => None,
            Provider::Recording(inner) => Some(inner.finish()),
        }
    }
}

fn build_provider(
    config: &RunConfig,
    agent: &str,
    inputs: &BTreeMap<String, Value>,
) -> Result<Provider> {
    let inner = build_model_provider(&config.selection())?;

    Ok(if config.record.is_some() {
        Provider::Recording(RecordingProvider::new(inner, agent).with_inputs(inputs.clone()))
    } else {
        Provider::Plain(inner)
    })
}

/// Vendors that need no declaring, when a key for them is exported.
pub const BUILT_IN_PROVIDERS: &[&str] = &["anthropic", "google", "openai"];

/// Whether a Gemini key is exported, under either name it goes by.
///
/// Both are in wide use and neither is wrong, so recognising only one would
/// leave a configured machine reporting that it has no Google provider.
#[cfg(feature = "google")]
pub fn google_key_is_set() -> bool {
    ["GEMINI_API_KEY", "GOOGLE_API_KEY"]
        .iter()
        .any(|name| std::env::var_os(name).is_some())
}

/// Every vendor this run can reach: the built-in ones, plus whatever the
/// manifest declared.
///
/// A vendor whose key is absent is not an error here — it is only an error if
/// the artifact asks for it, and then the router says so by name. A declared
/// provider replaces a built-in of the same name, so `[[model.provider]] name =
/// "openai"` points the familiar name somewhere else.
fn available(selection: &ProviderSelection) -> Result<Vec<(String, Box<dyn ModelProvider>)>> {
    selection
        .models
        .validate(BUILT_IN_PROVIDERS)
        .map_err(|reason| anyhow::anyhow!("{reason}"))?;

    let declared: BTreeSet<&str> = selection
        .models
        .providers
        .iter()
        .map(|provider| provider.name.as_str())
        .collect();
    // `mut` is unused in a build with no HTTP provider, which is what
    // `tools/ingot.Dockerfile` produces. Worth keeping that configuration
    // warning-free: a Docker build log full of noise is a Docker build log
    // nobody reads.
    #[allow(unused_mut)]
    let mut providers: Vec<(String, Box<dyn ModelProvider>)> = Vec::new();

    #[cfg(feature = "anthropic")]
    if !declared.contains("anthropic") && std::env::var_os("ANTHROPIC_API_KEY").is_some() {
        if let Ok(provider) = ingot_runtime::anthropic::AnthropicProvider::from_env() {
            providers.push((
                ingot_runtime::anthropic::PROVIDER.to_string(),
                Box::new(
                    provider
                        .with_model(selection.model.clone())
                        .with_effort(selection.effort.clone()),
                ),
            ));
        }
    }

    #[cfg(feature = "openai")]
    if !declared.contains("openai") && std::env::var_os("OPENAI_API_KEY").is_some() {
        if let Ok(provider) = ingot_runtime::openai::OpenAiProvider::from_env() {
            providers.push((
                ingot_runtime::openai::PROVIDER.to_string(),
                Box::new(
                    provider
                        .with_model(selection.model.clone())
                        .with_effort(selection.effort.clone()),
                ),
            ));
        }
    }

    #[cfg(feature = "google")]
    if !declared.contains("google") && google_key_is_set() {
        if let Ok(provider) = ingot_runtime::google::GoogleProvider::from_env() {
            providers.push((
                ingot_runtime::google::PROVIDER.to_string(),
                Box::new(
                    provider
                        .with_model(selection.model.clone())
                        .with_effort(selection.effort.clone()),
                ),
            ));
        }
    }

    // A declared provider is a stated intention, so a missing key for one is an
    // error rather than a quiet absence.
    #[cfg(feature = "providers")]
    for declaration in &selection.models.providers {
        let provider = ingot_runtime::catalogue::build(
            declaration,
            selection.model.clone(),
            selection.effort.clone(),
        )
        .map_err(|error| anyhow::anyhow!("{error}"))?;
        providers.push((declaration.name.clone(), provider));
    }

    // A build with no HTTP provider cannot serve a declaration, and saying so is
    // better than ignoring the manifest. This is the shape the image built by
    // `tools/ingot.Dockerfile` has: the contained half asks its supervisor for
    // completions, so it carries no provider and no TLS stack at all.
    #[cfg(not(feature = "providers"))]
    if let Some(declaration) = selection.models.providers.first() {
        bail!(
            "the manifest declares the model provider `{}`, and this build has no HTTP provider \
             to reach it with\n  \
             rebuild with `--features openai` (or `anthropic`), or use \
             `--provider replay --cassette <FILE>`",
            declaration.name
        );
    }

    let _ = declared;
    Ok(providers)
}

/// Route by the vendor the artifact pinned, falling back to the only key
/// present.
///
/// The point of the default: an artifact that says `model exact "openai/…"`
/// should just run, without the operator having to repeat that on the command
/// line — and must never be answered by a vendor it did not name.
fn auto(selection: &ProviderSelection) -> Result<Box<dyn ModelProvider>> {
    let mut providers = available(selection)?;
    if providers.is_empty() {
        bail!(
            "no model provider is available\n  \
             export ANTHROPIC_API_KEY, OPENAI_API_KEY or GEMINI_API_KEY, declare one with \
             `[[model.provider]]` in {}, or use `--provider replay --cassette <FILE>`",
            super::MANIFEST_NAME
        );
    }

    // `default = "…"` in the manifest, else the only one there is. With several
    // and no default, an artifact that names no vendor is asked to pick, rather
    // than being answered by whichever happened to sort first.
    let chosen_default = selection
        .models
        .default
        .clone()
        .or_else(|| (providers.len() == 1).then(|| providers[0].0.clone()));

    let mut router = RoutingProvider::new();
    if let Some(name) = &chosen_default {
        if let Some(index) = providers.iter().position(|(vendor, _)| vendor == name) {
            let (vendor, provider) = providers.remove(index);
            router = router.or_else(vendor, provider);
        }
    }
    for (vendor, provider) in providers {
        router = router.with(vendor, provider);
    }

    eprintln!("{}", router.describe());
    Ok(Box::new(router))
}

fn anthropic(selection: &ProviderSelection) -> Result<Box<dyn ModelProvider>> {
    #[cfg(feature = "anthropic")]
    {
        Ok(Box::new(
            ingot_runtime::anthropic::AnthropicProvider::from_env()
                .map_err(anyhow::Error::msg)?
                .with_model(selection.model.clone())
                .with_effort(selection.effort.clone()),
        ))
    }
    #[cfg(not(feature = "anthropic"))]
    {
        let _ = selection;
        bail!(
            "this build has no Anthropic provider\n\
             rebuild with `cargo build --features anthropic`, or use \
             `--provider replay --cassette <FILE>`"
        );
    }
}

fn openai(selection: &ProviderSelection) -> Result<Box<dyn ModelProvider>> {
    #[cfg(feature = "openai")]
    {
        Ok(Box::new(
            ingot_runtime::openai::OpenAiProvider::from_env()
                .map_err(anyhow::Error::msg)?
                .with_model(selection.model.clone())
                .with_effort(selection.effort.clone()),
        ))
    }
    #[cfg(not(feature = "openai"))]
    {
        let _ = selection;
        bail!(
            "this build has no OpenAI provider\n\
             rebuild with `cargo build --features openai`, or use \
             `--provider replay --cassette <FILE>`"
        );
    }
}

fn google(selection: &ProviderSelection) -> Result<Box<dyn ModelProvider>> {
    #[cfg(feature = "google")]
    {
        Ok(Box::new(
            ingot_runtime::google::GoogleProvider::from_env()
                .map_err(anyhow::Error::msg)?
                .with_model(selection.model.clone())
                .with_effort(selection.effort.clone()),
        ))
    }
    #[cfg(not(feature = "google"))]
    {
        let _ = selection;
        bail!(
            "this build has no Google provider\n\
             rebuild with `cargo build --features google`, or use \
             `--provider replay --cassette <FILE>`"
        );
    }
}

/// Say what the run cost, and say plainly when it could not be worked out.
///
/// Silence would be the failure mode: a `cost` budget that is never mentioned
/// looks enforced. [Runtime 0.1 §8](../../../specs/runtime/v0.1.md) says a
/// backend that cannot price a request must not pretend to, and not mentioning
/// it is a way of pretending.
fn report_cost(report: &RunReport) {
    let spend = &report.spend;
    if let Some(rendered) = spend.rendered() {
        eprintln!("cost      {rendered}");
    }
    for (model, reason) in spend.unpriced() {
        eprintln!("cost      not charged for `{model}`: {reason}");
    }
    if !spend.is_complete() {
        eprintln!("          the budget was not enforced; add `[[model.price]]` to charge it");
    }
}

pub(crate) fn write_outputs(report: &RunReport, config: &RunConfig) -> Result<()> {
    let Some(dir) = &config.out_dir else {
        // No directory given: the artifacts go to stdout, so the command
        // composes with a pipe.
        let mut stdout = std::io::stdout().lock();
        for artifact in report.outputs.values() {
            stdout
                .write_all(&artifact.to_bytes())
                .context("writing to standard output")?;
            if !artifact.to_bytes().ends_with(b"\n") {
                stdout.write_all(b"\n").ok();
            }
        }
        return Ok(());
    };

    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    for artifact in report.outputs.values() {
        let path = artifact_path(dir, artifact);
        std::fs::write(&path, artifact.to_bytes())
            .with_context(|| format!("writing {}", path.display()))?;
        println!("{} -> {}", artifact.name, path.display());
    }
    Ok(())
}

fn artifact_path(dir: &Path, artifact: &Artifact) -> PathBuf {
    dir.join(format!("{}.{}", artifact.name, artifact.extension()))
}

// --- ingot test -----------------------------------------------------------

pub struct TestConfig {
    pub cassette_dir: PathBuf,
    pub filter: Option<String>,
    pub pricing: ingot_runtime::price::Pricing,
}

/// Replay every cassette in a directory and report pass/fail.
pub fn test(compilation: &Compilation, config: &TestConfig) -> Result<u8> {
    if !config.cassette_dir.is_dir() {
        eprintln!(
            "no cassettes in {} — nothing to test",
            config.cassette_dir.display()
        );
        eprintln!(
            "record one with: ingot run --provider anthropic --record {}/<name>.json --input ...",
            config.cassette_dir.display()
        );
        return Ok(super::EXIT_OK);
    }

    let cassettes =
        ingot_runtime::load_directory(&config.cassette_dir).map_err(anyhow::Error::msg)?;
    if cassettes.is_empty() {
        eprintln!("no cassettes in {}", config.cassette_dir.display());
        return Ok(super::EXIT_OK);
    }

    let registry: AgentRegistry = compilation
        .agents
        .iter()
        .map(|agent| (agent.agent.clone(), agent.clone()))
        .collect();

    let mut passed = 0usize;
    let mut failed = 0usize;

    for (name, cassette) in cassettes {
        if let Some(filter) = &config.filter {
            if !name.contains(filter.as_str()) {
                continue;
            }
        }

        let Some(ir) = registry.get(&cassette.agent) else {
            eprintln!(
                "FAIL {name}: the cassette targets `{}`, which this program does not declare",
                cassette.agent
            );
            failed += 1;
            continue;
        };

        let inputs = cassette.inputs.clone();
        // Tools are served from the recording, or refused. Never reached: a
        // test that touches the filesystem or the network is not the offline,
        // repeatable thing `ingot test` promises, and a cassette with no tool
        // calls in it is not evidence that a tool-using agent works.
        let recorded_tools = cassette.tool_calls.clone();
        let mut provider = ReplayProvider::new(cassette);
        let mut tools = ReplayTools::new(recorded_tools);
        let mut sink = ingot_runtime::CollectingSink::default();

        let result = run_agent(
            ir,
            &registry,
            &mut provider,
            &mut tools,
            &mut sink,
            RunOptions {
                inputs,
                approval: ApprovalMode::Deny,
                max_steps: 1_000,
                // The same prices a live run uses, so `cost <= 5 usd` is a
                // property `ingot test` can hold the agent to rather than a
                // line nothing checks.
                pricing: config.pricing.clone(),
            },
        );

        match result {
            Ok(report) => {
                let unused = provider.remaining();
                let unused_tools = tools.remaining();
                if unused > 0 || unused_tools > 0 {
                    // A recording that was not played out is a test that stopped
                    // early without saying so.
                    if unused > 0 {
                        eprintln!(
                            "FAIL {name}: {unused} recorded interaction(s) were never played"
                        );
                    }
                    if unused_tools > 0 {
                        eprintln!(
                            "FAIL {name}: {unused_tools} recorded tool call(s) were never played"
                        );
                    }
                    failed += 1;
                } else {
                    let cost = match report.spend.rendered() {
                        Some(rendered) => format!(", {rendered}"),
                        None => String::new(),
                    };
                    // A budget nothing could charge is named here too. A test
                    // that quietly did not enforce one is a test that says a
                    // limit holds when nobody checked.
                    for (model, reason) in report.spend.unpriced() {
                        eprintln!("     {name}: cost not charged for `{model}`: {reason}");
                    }
                    println!(
                        "ok   {name}  ({} step(s), {} token(s){cost})",
                        report.steps,
                        report.usage.total()
                    );
                    passed += 1;
                }
            }
            Err(error) => {
                eprintln!("FAIL {name}: {error}");
                failed += 1;
            }
        }
    }

    if failed == 0 {
        println!("{passed} passed");
        Ok(super::EXIT_OK)
    } else {
        eprintln!("{passed} passed, {failed} failed");
        Ok(super::EXIT_DIAGNOSTICS)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inputs_accept_bare_strings() {
        let inputs = parse_inputs(&["topic=compilers".to_string()]).unwrap();
        assert_eq!(inputs["topic"], Value::String("compilers".into()));
    }

    #[test]
    fn inputs_accept_json() {
        let inputs = parse_inputs(&["items=[\"a\",\"b\"]".to_string(), "n=3".to_string()]).unwrap();
        assert_eq!(inputs["items"], serde_json::json!(["a", "b"]));
        assert_eq!(inputs["n"], serde_json::json!(3));
    }

    #[test]
    fn a_value_containing_equals_is_not_split_twice() {
        let inputs = parse_inputs(&["q=a=b".to_string()]).unwrap();
        assert_eq!(inputs["q"], Value::String("a=b".into()));
    }

    #[test]
    fn a_malformed_input_is_reported() {
        let error = parse_inputs(&["nonsense".to_string()]).unwrap_err();
        assert!(error.to_string().contains("name=value"), "{error}");
    }

    #[test]
    fn artifact_paths_use_the_content_type_extension() {
        let artifact = Artifact {
            name: "report".into(),
            content_type: "markdown".into(),
            value: Value::String("x".into()),
        };
        let path = artifact_path(Path::new("out"), &artifact);
        assert!(path.ends_with("report.md"), "{}", path.display());
    }
}
