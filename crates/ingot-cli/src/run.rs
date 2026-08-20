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
use ingot_runtime::cassette::{Consultation, RecordingInterlocutor, ReplayInterlocutor};
use ingot_runtime::{
    run as run_agent, AgentRegistry, ApprovalRequest, Artifact, Cassette, ConsultError,
    ConsultRequest, DenyAllTools, EventSink, FanOut, HumanChannel, Interlocutor, ModelConfig,
    ModelProvider, ProviderError, RecordingProvider, RecordingTools, ReplayProvider,
    ReplayToolHost as ReplayTools, RoutingProvider, RunError, RunEvent, RunOptions, RunReport,
    ToolHost,
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

/// Where an approval gate is answered.
///
/// See [RFC-0020](../../../rfcs/0020-a-person-in-the-loop.md). The channel this
/// adds is deliberately half a channel: the gate already leaves the run on the
/// event stream, so only the answer needed a way in.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum ApprovalChannel {
    /// A terminal if standard input is one, and a refusal if it is not.
    #[default]
    Auto,
    /// One JSON line per gate, read from standard input.
    Stdin,
}

/// Where snapshots live inside a project's build directory.
pub const SNAPSHOTS_DIR: &str = "snapshots";

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
    /// Where this run writes itself down, or `None` to keep no record.
    ///
    /// Separate from `out_dir`, which is where the *agent's* artifacts go and
    /// which an operator points at a pipeline. A run record is the toolchain's
    /// own output and belongs with the build output whatever the agent's
    /// results are doing.
    pub history: Option<PathBuf>,
    pub events: EventFormat,
    /// The project's build directory, where a default memory store lives.
    ///
    /// Deliberately not `history`, which is `None` under `--no-history`. Where
    /// an agent keeps what it remembers and whether this run is written down
    /// are different questions, and tying them together made `--no-history`
    /// silently mean `--no-memory` too.
    pub build_dir: Option<PathBuf>,
    /// Stop at the checkpoint with this label and write a snapshot.
    pub stop_at: Option<String>,
    /// Continue the run this snapshot describes.
    pub resume: Option<PathBuf>,
    /// Where the snapshot goes, when the operator named a place.
    pub snapshot: Option<PathBuf>,
    /// Where the agent's persistent memory store lives, when the operator named
    /// a place. Absent means the default under `build_dir`.
    pub memory: Option<PathBuf>,
    pub memory_mode: crate::memory::MemoryMode,
    pub yes: bool,
    /// Where an approval gate is answered.
    pub approvals: ApprovalChannel,
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
    /// Proceed even where nothing will keep a reach the artifact declared.
    ///
    /// Separate from `sandbox_allow_unenforced`, which is about a boundary
    /// falling short of a policy. This one is about the artifact's own stronger
    /// statement, and it applies with or without a boundary.
    pub allow_unenforced_scopes: bool,
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
            replay_from: 0,
            strict_replay: true,
            announce_routing: true,
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
    /// How many recorded interactions the run this continues already played.
    ///
    /// A cassette is matched by position, so the second half of an interrupted
    /// run has to start where the first stopped. Zero for a run that is not a
    /// resumption, and ignored by every provider that is not a replay.
    pub replay_from: usize,
    /// Whether to print the one-line routing notice for this build.
    ///
    /// The notice says which service a **run's** calls go to, so it belongs to
    /// the run and not to a provider instance. A fan-out builds one provider per
    /// overlapping worker
    /// ([RFC-0021](../../../rfcs/0021-a-fan-out-that-overlaps.md)), and those
    /// builds are silent: printing the same line five times told an operator
    /// nothing except how many threads there were, which is the one thing about a
    /// run that is deliberately not observable.
    pub announce_routing: bool,
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

/// Where a project keeps the recordings `ingot test` replays.
///
/// The same directory `ingot test` defaults to, so a cassette written for one
/// is found by the other. A convention rather than a setting: two names for the
/// same directory would only mean one of them is the wrong one.
pub const CASSETTE_DIR: &str = "tests/cassettes";

/// The cassette `--provider replay` should use when nobody named one.
///
/// A project usually has exactly one, and making somebody type its path to
/// replay their own fixture is friction with nothing behind it. Two or more is
/// a real question — which recording did you mean? — so it is asked rather than
/// guessed, and the answer names them.
pub fn project_cassette(root: &Path) -> Result<PathBuf> {
    let directory = root.join(CASSETTE_DIR);
    let mut found: Vec<PathBuf> = std::fs::read_dir(&directory)
        .map_err(|_| {
            anyhow::anyhow!(
                "`--provider replay` needs a cassette, and {} does not exist\n  \
                 record one with: ingot run --record {CASSETTE_DIR}/<name>.json --input ...",
                directory.display()
            )
        })?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().map(|ext| ext == "json").unwrap_or(false))
        .collect();
    found.sort();

    match found.len() {
        0 => bail!(
            "`--provider replay` needs a cassette, and {} holds none\n  \
             record one with: ingot run --record {CASSETTE_DIR}/<name>.json --input ...",
            directory.display()
        ),
        1 => Ok(found.remove(0)),
        _ => bail!(
            "{} holds {} cassettes, so `--cassette <FILE>` has to say which:\n{}",
            directory.display(),
            found.len(),
            found
                .iter()
                .map(|path| format!("  {}", path.display()))
                .collect::<Vec<_>>()
                .join("\n")
        ),
    }
}

/// Build the provider a command asked for.
pub fn build_model_provider(
    selection: &ProviderSelection,
) -> Result<Box<dyn ModelProvider + Send>> {
    match selection.choice {
        ProviderChoice::Replay => {
            let Some(path) = &selection.cassette else {
                bail!(
                    "`--provider replay` needs `--cassette <FILE>`\n\
                     record one first with: ingot run --record <FILE>"
                );
            };
            let cassette = Cassette::load(path).map_err(anyhow::Error::msg)?;
            let provider = ReplayProvider::new(cassette).skipping(selection.replay_from);
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

/// Effects this arrangement can bound to named values **in kind**.
///
/// A boundary derives its mounts from the policy, and the compiler has already
/// checked that a tool's declared paths are inside it — so under a boundary, a
/// declared path reach is kept. **The same is true of a host reach, and has been
/// since 0.4.0:** [GAP-001](../../../docs/gaps.md#gap-001) closed, and a contained
/// server joins an `--internal` network whose only way out is the egress proxy,
/// which refuses a host the policy does not grant.
///
/// This answers *can this arrangement bound this kind of effect at all*, and
/// deliberately not *will it, on this machine, right now*. The second question is
/// settled later and better: the run detects whether the proxy image is present,
/// passes that into the plans, prints exactly what is missing when it is not, and
/// refuses on [`ingot_sandbox::SandboxPlan::is_fully_enforced`]. Answering it in
/// two places is how `network` came to be refused here on the grounds that no
/// arrangement existed, two releases after one did.
fn boundable(config: &RunConfig) -> &'static [&'static str] {
    if config.sandbox || config.containment().is_some() {
        &["filesystem_read", "filesystem_write", "network"]
    } else {
        &[]
    }
}

/// Refuse before starting when the artifact declares a reach nothing will keep.
///
/// A policy's value list has always been advisory where nothing enforces it,
/// and Ingot says so. `!network("arxiv.org")` is not advisory: it states that
/// this tool must be bounded to that host. Running it under something that
/// cannot do that, without being asked, would make the strongest statement in
/// the language the least reliable one.
///
/// Opt-in, so nothing written before [RFC-0014](../../../rfcs/0014-a-capabilitys-reach.md)
/// changes: an artifact that declares no reach reaches this function and leaves
/// immediately.
fn check_declared_reach(
    ir: &ingot_ir::AgentIr,
    registry: &AgentRegistry,
    config: &RunConfig,
) -> Result<()> {
    let boundable = boundable(config);
    let mut unkept: Vec<String> = Vec::new();

    for agent in std::iter::once(ir).chain(registry.values()) {
        for tool in &agent.tools {
            for (effect, values) in &tool.scopes {
                if boundable.contains(&effect.as_str()) {
                    continue;
                }
                unkept.push(format!(
                    "  `{}` declares {effect}({})\n    in agent `{}`",
                    tool.name,
                    values
                        .iter()
                        .map(|value| format!("{value:?}"))
                        .collect::<Vec<_>>()
                        .join(", "),
                    agent.agent
                ));
            }
        }
    }
    unkept.sort();
    unkept.dedup();
    if unkept.is_empty() {
        return Ok(());
    }

    if !config.allow_unenforced_scopes {
        // One remedy now, because there is one. A path reach and a host reach are
        // both kept by a boundary: the mounts come from the policy, and the egress
        // proxy refuses a host the policy does not grant. This used to point a
        // host reach at nothing, on the grounds that no arrangement existed --
        // true when it was written and false since 0.4.0.
        let advice = "this run has no boundary, so nothing bounds a tool to anything; \
             run with --sandbox";
        bail!(
            "this program states where its tools may reach, and this run cannot keep it:\n{}\n\n  \
             {advice}\n  \
             pass --allow-unenforced-scopes to proceed knowing the declaration is advisory here",
            unkept.join("\n")
        );
    }
    for note in &unkept {
        eprintln!("warning: proceeding with a reach nothing enforces\n{note}");
    }
    Ok(())
}

/// Compile, execute, and write the artifacts.
pub fn execute(compilation: &Compilation, config: &RunConfig) -> Result<u8> {
    let (ir, registry) = select_agent(compilation, config.agent.as_deref())?;
    check_declared_reach(&ir, &registry, config)?;
    refuse_an_unanswerable_channel(config)?;

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
        // No resumption inside a box: the supervisor channel reports a finished
        // run or a failed one, so a contained run never stops and never has a
        // snapshot to continue from.
        let mut provider = build_provider(config, &ir.agent, &inputs, 0)?;
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

    // Loaded before the provider, because a replay has to start where the first
    // half stopped: a cassette is matched by position.
    //
    // Checked against the artifact here as well as in the interpreter, so every
    // problem with a `--resume` file is reported the same way -- before the run,
    // as an operational failure -- rather than one before and one during
    // depending on which check happens to fire.
    let resume = match &config.resume {
        Some(path) => {
            let snapshot = ingot_runtime::Resumption::load(path).map_err(anyhow::Error::msg)?;
            snapshot.check(&ir).map_err(anyhow::Error::msg)?;
            Some(snapshot)
        }
        None => None,
    };
    let replay_from = resume
        .as_ref()
        .map(|snapshot| snapshot.model_calls as usize)
        .unwrap_or(0);

    // A person is the third source of answers, so the channel is arranged the
    // way the provider and the tool host already are: replayed from the
    // recording when there is one, wrapped in a recorder when one is being
    // made. See [RFC-0020](../../../rfcs/0020-a-person-in-the-loop.md).
    let mut recorded_answers: Option<std::rc::Rc<std::cell::RefCell<Vec<Consultation>>>> = None;
    if config.provider == ProviderChoice::Replay {
        // Read again rather than threaded down from the provider, which loads
        // it for its own half. One small file, and it keeps each half asking
        // the cassette for what it needs instead of one half carrying the
        // other's.
        if let Some(path) = &config.cassette {
            let cassette = Cassette::load(path).map_err(anyhow::Error::msg)?;
            let played = resume
                .as_ref()
                .map(|snapshot| snapshot.consultations as usize)
                .unwrap_or(0);
            // Strict, like the provider a run gets: a changed question must
            // fail loudly rather than reuse an answer somebody gave to a
            // different one. Leniency belongs to tooling that deliberately
            // replays against edited sources, and a run is not that.
            approval = HumanChannel::Ask(Box::new(
                ReplayInterlocutor::new(cassette.consultations).skipping(played),
            ));
        }
    } else if config.record.is_some() {
        let (recorder, answers) = RecordingInterlocutor::new(approval);
        approval = HumanChannel::Ask(Box::new(recorder));
        recorded_answers = Some(answers);
    }

    let mut provider = build_provider(config, &ir.agent, &inputs, replay_from)?;
    let mut sink = RunSink {
        printer: printer_for(config, compilation, false),
    };

    let store = crate::memory::open(
        &ir,
        config.memory.as_deref(),
        config.build_dir.as_deref(),
        config.memory_mode.clone(),
    )?;
    if !store.note.is_empty() && config.events != EventFormat::Quiet {
        eprintln!("{}", store.note);
    }
    if let Some(dropped) = &store.dropped {
        eprintln!("{dropped}");
    }

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
            memory: store.fields,
            stop_at: config.stop_at.clone(),
            resume,
            pricing: config.models.pricing(),
            fan_out: fan_out(config),
        },
    );

    // Record whatever happened before propagating a failure: a partial cassette
    // is more useful than none when debugging why a run broke.
    if let Some(path) = &config.record {
        if let Some(mut cassette) = provider.finish_recording() {
            cassette.tool_calls = tools.finish_recording();
            if let Some(answers) = &recorded_answers {
                cassette.consultations = answers.borrow().clone();
            }
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
            sink.printer.finish_record(crate::runs::Outcome::Failed {
                reason: &error.to_string(),
            });
            report_failure(&error);
            return Ok(super::EXIT_DIAGNOSTICS);
        }
    };

    // Written before the artifacts, so a failure to write the store cannot be
    // mistaken for a run that did not reach the end.
    if let Some(path) = &store.path {
        crate::memory::save(path, &ir, &report.memory)?;
    }

    if let Some(snapshot) = &report.stopped {
        let path = snapshot_path(config, &ir.agent, &snapshot.label);
        snapshot.save(&path).map_err(anyhow::Error::msg)?;
        sink.printer.finish_record(crate::runs::Outcome::Finished {
            steps: report.steps,
            usage: report.usage,
            cost: report.spend.rendered(),
        });
        // Not on stdout: a stopped run produced no artifact, and the path is
        // the one thing the operator needs next.
        eprintln!(
            "stopped at \"{}\"\n  resume with: ingot run --resume {}",
            snapshot.label,
            path.display()
        );
        return Ok(super::EXIT_OK);
    }

    sink.printer.finish_record(crate::runs::Outcome::Finished {
        steps: report.steps,
        usage: report.usage,
        cost: report.spend.rendered(),
    });
    report_cost(&report);
    write_outputs(&report, config)?;
    Ok(super::EXIT_OK)
}

/// Where a stopped run's snapshot goes.
///
/// Beside the run records and the memory stores, under the build directory,
/// because it is output: disposable, already ignored by version control, and
/// expected to be lost with the build directory.
fn snapshot_path(config: &RunConfig, agent: &str, label: &str) -> PathBuf {
    if let Some(path) = &config.snapshot {
        return path.clone();
    }
    let safe = |text: &str| -> String {
        text.chars()
            .map(|ch| {
                if ch.is_ascii_alphanumeric() || ch == '.' || ch == '-' || ch == '_' {
                    ch
                } else {
                    '_'
                }
            })
            .collect()
    };
    let base = config
        .build_dir
        .clone()
        .unwrap_or_else(|| PathBuf::from("."));
    base.join(SNAPSHOTS_DIR)
        .join(format!("{}-{}.json", safe(agent), safe(label)))
}

/// A printer for this run, keeping a record when the command asked for one.
pub(crate) fn printer_for(
    config: &RunConfig,
    compilation: &Compilation,
    contained: bool,
) -> EventPrinter {
    let printer = EventPrinter::new(config.events, compilation);
    match &config.history {
        Some(out_dir) => printer.recording_to(out_dir, contained),
        None => printer,
    }
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
    /// Where this run is being written down, when it is.
    history: Option<History>,
}

/// The state a printer needs to keep a run record beside the terminal output.
///
/// The recorder is opened on the first event rather than up front, because
/// `runStarted` is what names the agent and the provider — and because a run
/// that never got as far as starting has nothing worth a file.
struct History {
    out_dir: PathBuf,
    contained: bool,
    recorder: Option<crate::runs::RunRecorder>,
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
            history: None,
        }
    }

    /// Also write this run down, under `out_dir`.
    ///
    /// Deltas are deliberately not recorded. They are not events — see
    /// [`ingot_runtime::EventSink`] — and a record holding them would be a
    /// record a replay could not reproduce.
    pub(crate) fn recording_to(mut self, out_dir: &Path, contained: bool) -> Self {
        self.history = Some(History {
            out_dir: out_dir.to_path_buf(),
            contained,
            recorder: None,
        });
        self
    }

    pub(crate) fn print(&mut self, event: &RunEvent) {
        // A model that was mid-sentence has stopped being mid-sentence, and the
        // next line should not land in the middle of it.
        self.close_stream();
        self.record(event);
        match self.format {
            EventFormat::Text => eprintln!("{}", self.trace.render(event)),
            EventFormat::Json => eprintln!("{}", event.to_json_line()),
            EventFormat::Quiet => {}
        }
    }

    fn record(&mut self, event: &RunEvent) {
        let Some(history) = &mut self.history else {
            return;
        };
        if history.recorder.is_none() {
            let RunEvent::RunStarted { agent, provider } = event else {
                // Nothing before `runStarted` names the run, and nothing after
                // it arrives without one.
                return;
            };
            history.recorder = crate::runs::RunRecorder::begin(
                &history.out_dir,
                agent,
                provider,
                history.contained,
            );
        }
        if let Some(recorder) = &mut history.recorder {
            recorder.event(event);
        }
    }

    /// Close the run record with what the command saw, and say where it went.
    ///
    /// Called by whichever path ran the agent. A record left unclosed is a run
    /// that reported no result, which is a state the studio shows under that
    /// name rather than one it guesses about.
    pub(crate) fn finish_record(&mut self, outcome: crate::runs::Outcome<'_>) {
        let Some(history) = &mut self.history else {
            return;
        };
        let Some(recorder) = &mut history.recorder else {
            return;
        };
        recorder.finish(outcome);
        if self.format != EventFormat::Quiet {
            eprintln!("history   {}", recorder.path().display());
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

fn approval_mode(config: &RunConfig) -> HumanChannel {
    if config.yes {
        return HumanChannel::AssumeYes;
    }
    // Before the terminal check, and deliberately: an operator who named a
    // channel gets it whether or not this process happens to have a terminal.
    // The parent asked to answer, so the parent answers.
    if config.approvals == ApprovalChannel::Stdin {
        return HumanChannel::Ask(Box::new(StdinChannel));
    }
    if std::io::stdin().is_terminal() {
        HumanChannel::Ask(Box::new(TerminalPerson))
    } else {
        // Unattended runs deny by default. An artifact that asked for a human
        // does not get one silently.
        HumanChannel::Deny
    }
}

/// One gate, answered by whoever started this process.
///
/// **This is half a channel, and that is the design.** A gate already leaves the
/// run: [`RunEvent::ApprovalRequested`] is emitted before the handler is asked,
/// so under `--events json` a parent watching stderr sees the node, the effects
/// and the reason without anything new being invented. Only the answer had
/// nowhere to go, so only the answer gets a path.
///
/// It is standard input rather than standard output because **stdout carries
/// the run's artifacts** so the command composes with a pipe, and a protocol
/// sharing that stream would corrupt them. See
/// [RFC-0020](../../../rfcs/0020-a-person-in-the-loop.md).
struct StdinChannel;

/// One line in: which gate, and whether it may proceed.
///
/// `deny_unknown_fields` for the reason the studio's request struct has it —
/// inventing a field must be a refusal rather than something quietly ignored.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ApprovalAnswer {
    node: String,
    allowed: bool,
}

/// One answer line from the parent, skipping blanks.
///
/// Shared by both halves of the channel because both want the same thing: the
/// next line, or a reason there will not be one.
fn read_answer_line(node: &str) -> Result<String, String> {
    let mut line = String::new();
    loop {
        line.clear();
        match std::io::stdin().read_line(&mut line) {
            Ok(0) => {
                return Err(format!(
                    "standard input closed before node `{node}` was answered"
                ))
            }
            Ok(_) if line.trim().is_empty() => continue,
            Ok(_) => return Ok(line.trim().to_string()),
            Err(error) => return Err(format!("cannot read an answer: {error}")),
        }
    }
}

impl Interlocutor for StdinChannel {
    fn approve(&mut self, request: &ApprovalRequest) -> bool {
        // The parent went away without answering. Refusing is the only safe
        // reading: a closed pipe must never become consent, which is the one
        // failure an approval gate exists to prevent.
        let line = match read_answer_line(&request.node) {
            Ok(line) => line,
            Err(reason) => {
                eprintln!("approval channel: {reason}");
                return false;
            }
        };

        let answer: ApprovalAnswer = match serde_json::from_str(line.trim()) {
            Ok(answer) => answer,
            Err(error) => {
                eprintln!("approval channel: unreadable answer for node `{}`: {error}\n  expected {{\"node\":\"…\",\"allowed\":true|false}}", request.node);
                return false;
            }
        };

        // An answer naming another gate means the channel is one message out of
        // step, and the run blocks on one gate at a time — so this is a parent
        // answering a question that was already settled. Applying it here would
        // decide *this* gate with *that* intent, which is worse than refusing.
        if answer.node != request.node {
            eprintln!(
                "approval channel: node `{}` was asked and the answer named `{}`",
                request.node, answer.node
            );
            return false;
        }
        answer.allowed
    }

    fn consult(&mut self, request: &ConsultRequest) -> Result<String, ConsultError> {
        let line = read_answer_line(&request.node).map_err(ConsultError::Failed)?;
        let answer: ConsultAnswer = serde_json::from_str(line.trim()).map_err(|error| {
            ConsultError::Failed(format!(
                "unreadable answer for node `{}`: {error}; expected                  {{\"node\":\"…\",\"answer\":\"…\"}}",
                request.node
            ))
        })?;
        if answer.node != request.node {
            return Err(ConsultError::Failed(format!(
                "node `{}` was asked and the answer named `{}`",
                request.node, answer.node
            )));
        }
        Ok(answer.answer)
    }
}

struct TerminalPerson;

impl Interlocutor for TerminalPerson {
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

    fn consult(&mut self, request: &ConsultRequest) -> Result<String, ConsultError> {
        eprintln!();
        eprintln!("  A QUESTION FOR YOU at node {}", request.node);
        for (name, value) in &request.context {
            eprintln!("  {name}: {}", render_context(value));
        }
        eprintln!("  {}", request.question);

        if request.choices.is_empty() {
            eprint!("  your answer: ");
            let _ = std::io::stderr().flush();
            let mut answer = String::new();
            if std::io::stdin().read_line(&mut answer).is_err() {
                return Err(ConsultError::Failed("standard input closed".to_string()));
            }
            return Ok(answer.trim().to_string());
        }

        for (index, choice) in request.choices.iter().enumerate() {
            eprintln!("    {}) {choice}", index + 1);
        }
        // Re-asked rather than failed on a typo. A question has no safe default,
        // so the only alternative to asking again is ending the run over a
        // mistyped digit — which loses everything before it.
        loop {
            eprint!("  choose 1-{}: ", request.choices.len());
            let _ = std::io::stderr().flush();
            let mut answer = String::new();
            if std::io::stdin().read_line(&mut answer).is_err() {
                return Err(ConsultError::Failed("standard input closed".to_string()));
            }
            let answer = answer.trim();
            if answer.is_empty() {
                return Err(ConsultError::Failed(
                    "no answer given at the terminal".to_string(),
                ));
            }
            if let Some(choice) = answer
                .parse::<usize>()
                .ok()
                .filter(|number| *number >= 1 && *number <= request.choices.len())
                .map(|number| request.choices[number - 1].clone())
            {
                return Ok(choice);
            }
            if let Some(choice) = request.choices.iter().find(|choice| *choice == answer) {
                return Ok(choice.clone());
            }
            eprintln!("  `{answer}` is not one of the choices");
        }
    }
}

/// One line in: which question, and what the person said.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ConsultAnswer {
    node: String,
    answer: String,
}

/// A context value, as a person should see it.
///
/// A bare string shows as itself rather than as a quoted JSON string: the person
/// is reading prose, not a document.
fn render_context(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// Refuse a channel whose other half nobody can see.
///
/// `--approvals stdin` splits an exchange across two streams: the gate leaves on
/// the event stream and the answer comes back on standard input. Under
/// `--events text` the gate leaves as prose meant for a person, and under
/// `--events quiet` it does not leave at all — so a parent would wait for a line
/// it has no way to know is wanted, and the run would wait for an answer nobody
/// knew to send. **Two processes waiting for each other is the failure this
/// whole channel exists to remove**, so it is refused here rather than reached.
fn refuse_an_unanswerable_channel(config: &RunConfig) -> Result<()> {
    if config.approvals == ApprovalChannel::Stdin && config.events != EventFormat::Json {
        bail!(
            "`--approvals stdin` needs `--events json`\n  \
             a gate is answered on standard input and asked on the event stream, so a parent \
             that cannot read the stream cannot know a gate is waiting\n  \
             help: add `--events json`"
        );
    }
    Ok(())
}

/// Refuse a remote server under a boundary that cannot cover it.
///
/// `--sandbox` starts a server inside a boundary derived from the agent's
/// policy, and there is no process to put in one. `--contained` puts the
/// interpreter inside a box whose network is denied, and the supervisor channel
/// carries a model call and an approval gate -- not a tool call.
///
/// Connecting anyway and reporting a boundary that covers nothing would be
/// worse than not offering the flag. See
/// [RFC-0019](../../../rfcs/0019-a-tool-server-that-is-not-a-child-process.md).
fn refuse_remote_under_a_boundary(config: &RunConfig) -> Result<()> {
    let named = |flag: &str| -> Result<()> {
        let server = config
            .mcp
            .servers
            .iter()
            .find(|server| server.is_remote())
            .map(|server| server.name.clone())
            .unwrap_or_default();
        bail!(
            "MCP server `{server}` is reached over a network, which `{flag}` cannot cover\n  \
             {}\n  \
             help: run it locally with a `command`, or drop `{flag}`",
            if flag == "--sandbox" {
                "a boundary bounds a process this machine starts, and there is none here"
            } else {
                "the supervisor channel carries a model call and an approval gate; \
                 there is no channel for a tool call out of the box"
            }
        )
    };
    if config.sandbox {
        return named("--sandbox");
    }
    if config.contained {
        return named("--contained");
    }
    Ok(())
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

/// The same, split by agent, with each agent's `network` grant attached.
///
/// Needed by two callers for two reasons: a boundary is derived per agent, and
/// a remote server is authorised per agent. Both want the same list.
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
            .with_network(network_grant(agent))
        })
        .collect()
}

/// What an agent's policy says under `network`, in the form the tool host needs.
///
/// Default-deny, like every other subject: a policy with no `network` rule
/// grants nothing, which is why an absent entry becomes a denial rather than an
/// unscoped allow.
fn network_grant(agent: &ingot_ir::AgentIr) -> ingot_mcp::NetworkGrant {
    match agent.policy.get("network") {
        Some(rule) => ingot_mcp::NetworkGrant {
            allowed: matches!(
                rule.decision,
                ingot_ir::Decision::Allow | ingot_ir::Decision::RequireApproval
            ),
            hosts: rule.values.iter().cloned().collect(),
        },
        None => ingot_mcp::NetworkGrant::default(),
    }
}

/// A name for this run's network and proxy.
///
/// Derived from the process id rather than a random value, so a leftover object
/// can be traced back to the run that made it — and so two runs on one machine
/// never collide on a name.
fn boundary_name(compilation: &Compilation) -> String {
    let agent = compilation
        .agents
        .first()
        .map(|agent| agent.agent.as_str())
        .unwrap_or("run");
    let slug: String = agent
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect();
    format!("{}-{}", slug.to_ascii_lowercase(), std::process::id())
}

/// Build the host that contains each tool server.
///
/// Refuses before starting anything when a boundary cannot honour a rule the
/// artifact states. An operator who switched a sandbox on and believes
/// `network allow ["arxiv.org"]` is in force is worse off than one who knows it
/// is not.
fn contained_host(compilation: &Compilation, config: &RunConfig) -> Result<McpToolHost> {
    // Looked for, not required — yet. The boundary is settled from the artifact
    // before the environment gets a say, so a policy this program states and no
    // boundary can keep is reported as that, on a machine with a container
    // runtime and on a machine without one. Demanding the runtime here would
    // replace "your policy cannot be enforced" with "install Docker", which
    // answers a question nobody asked and hides the one they did.
    let detected = ingot_sandbox::detect();

    // Whether the allowlist will be kept has to be settled before the plans are
    // made, because it decides what they report. Settled from what is actually
    // available: an image that is not there cannot bound anything, and a plan
    // that assumed it would be would be the lie this whole path avoids.
    let egress_image = std::env::var("INGOT_EGRESS_IMAGE")
        .unwrap_or_else(|_| ingot_sandbox::DEFAULT_EGRESS_IMAGE.to_string());
    let can_filter = detected
        .as_ref()
        .ok()
        .and_then(|runtime| ingot_sandbox::image_exists(runtime, &egress_image).ok())
        .unwrap_or(false);

    let plans =
        crate::sandbox::plan_all_with(compilation, &config.mcp, &config.workspace, can_filter)
            .map_err(|problems| anyhow::anyhow!("{}", problems.join("\n")))?;

    let hosts = crate::sandbox::allowed_hosts(&plans);
    if !hosts.is_empty() && !can_filter {
        eprintln!(
            "note: `{egress_image}` is not present, so the host allowlist cannot be kept\n  \
             build it with: docker build -f tools/egress.Dockerfile -t {egress_image} ."
        );
    }

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

    // Now the machine matters: everything above was about the program, and this
    // is the first line that needs something to actually put a server inside.
    let runtime = detected.map_err(|error| anyhow::anyhow!("{error}"))?;
    let mut launcher =
        crate::sandbox::ContainerLauncher::new(runtime.clone(), config.workspace.clone(), plans);

    if can_filter && !hosts.is_empty() {
        // One name per run, so two runs on one machine do not share a filter
        // built from one of their policies.
        let name = boundary_name(compilation);
        let boundary = ingot_sandbox::EgressBoundary::start(&runtime, &name, &hosts, &egress_image)
            .map_err(|error| anyhow::anyhow!("{error}"))?;
        eprintln!("egress    bounded to {} by a proxy", hosts.join(", "));
        launcher = launcher.through(boundary);
    }

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
fn tool_host(compilation: &Compilation, config: &RunConfig) -> Result<Box<dyn ToolHost + Send>> {
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

    // A remote server is authorised against the calling agent's own `network`
    // grant, which means the host has to know which agent it is starting for.
    // The one-unnamed-agent shortcut has no agent and so cannot check, so a
    // manifest with any remote server takes the per-agent path -- at the cost
    // of one session per agent, which is the correct cost: two agents in a
    // program legitimately differ, and one being allowed to reach a hosted
    // server does not admit the other.
    let remote = config.mcp.servers.iter().any(|server| server.is_remote());
    if remote {
        refuse_remote_under_a_boundary(config)?;
    }

    let host = if config.sandbox {
        contained_host(compilation, config)?
    } else if remote {
        McpToolHost::connect_agents(
            &config.mcp,
            &config.root,
            &tools_per_agent(compilation),
            &ingot_mcp::DirectLauncher,
        )
        .map_err(|error| anyhow::anyhow!("{error}"))?
    } else {
        McpToolHost::connect(&config.mcp, &config.root, &required)
            .map_err(|error| anyhow::anyhow!("{error}"))?
    };

    for server in &config.mcp.servers {
        if server.is_remote()
            && !server.url.as_deref().unwrap_or("").starts_with("https://")
            && !server.is_loopback()
        {
            eprintln!(
                "warning: MCP server `{}` is reached over plain HTTP at {}\n         \
                 tool arguments and results cross the network unencrypted",
                server.name,
                server.url.as_deref().unwrap_or("")
            );
        }
    }

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
    Plain(Box<dyn ToolHost + Send>),
    Recording(RecordingTools<Box<dyn ToolHost + Send>>),
}

impl Tools {
    fn new(inner: Box<dyn ToolHost + Send>, record: bool) -> Tools {
        if record {
            Tools::Recording(RecordingTools::new(inner))
        } else {
            Tools::Plain(inner)
        }
    }

    fn as_mut(&mut self) -> &mut (dyn ToolHost + Send) {
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
    Plain(Box<dyn ModelProvider + Send>),
    Recording(RecordingProvider<Box<dyn ModelProvider + Send>>),
}

impl Provider {
    /// Wrap a provider, recording its exchanges under `agent` when asked.
    pub(crate) fn new(inner: Box<dyn ModelProvider + Send>, record: bool, agent: &str) -> Provider {
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
    replay_from: usize,
) -> Result<Provider> {
    let inner = build_model_provider(&ProviderSelection {
        replay_from,
        ..config.selection()
    })?;

    Ok(if config.record.is_some() {
        Provider::Recording(RecordingProvider::new(inner, agent).with_inputs(inputs.clone()))
    } else {
        Provider::Plain(inner)
    })
}

/// Whether a `parallel map` in this run may overlap, and how many iterations.
///
/// The interesting half is whether there is anything to build a **second**
/// provider from. Two arrangements have exactly one source of answers, and a run
/// in either of them gets a ceiling of one:
///
/// * **a replay** has one tape, with one position in it;
/// * **a recording** has one cassette being written, and a cassette somebody can
///   review is written in index order — which needs per-iteration buffers this
///   change does not have yet.
///
/// Neither is a special case for cassettes. One source of answers is a lock, and
/// a lock is sequential execution with extra steps — the shape
/// [RFC-0021](../../../rfcs/0021-a-fan-out-that-overlaps.md) states once and
/// applies everywhere, including to a contained run, whose provider is a channel
/// and which therefore reaches this the same way by never asking.
fn fan_out(config: &RunConfig) -> FanOut {
    if config.record.is_some() || config.provider == ProviderChoice::Replay {
        return FanOut::default();
    }
    let selection = ProviderSelection {
        // Silent: the run announced its routing when it built its own provider,
        // and a worker's build is the same routing again.
        announce_routing: false,
        ..config.selection()
    };
    FanOut::new(
        Box::new(move || {
            // A provider that will not build is reported at the node that asked
            // for it, like any other provider failure. `anyhow`'s chain is
            // flattened with `:#` because `ProviderError` carries a message
            // rather than a source to walk.
            build_model_provider(&selection)
                .map_err(|error| ProviderError::Configuration(format!("{error:#}")))
        }),
        config.models.fan_out_ceiling(),
    )
}

/// Vendors that need no declaring, when a key for them is exported.
pub const BUILT_IN_PROVIDERS: &[&str] = &["anthropic", "google", "openai"];

/// A vendor this binary can reach without a manifest.
///
/// One table rather than one per reader. `ingot doctor` and `ingot studio` both
/// answer "is there a provider here?", and two copies of this list would let
/// them answer it differently.
pub struct BuiltIn {
    /// What the router and the manifest call it.
    pub name: &'static str,
    /// The wire protocol it speaks, which is what a build includes or omits.
    pub protocol: &'static str,
    /// Every variable it answers to, most canonical first. A vendor may go by
    /// more than one name and recognising only the first would tell a
    /// configured machine it has no provider.
    pub variables: &'static [&'static str],
    /// Whether this build carries the protocol at all.
    pub included: bool,
}

pub const BUILT_IN: &[BuiltIn] = &[
    BuiltIn {
        name: "anthropic",
        protocol: "anthropic",
        variables: &["ANTHROPIC_API_KEY"],
        included: cfg!(feature = "anthropic"),
    },
    BuiltIn {
        name: "google",
        protocol: "google",
        variables: &["GEMINI_API_KEY", "GOOGLE_API_KEY"],
        included: cfg!(feature = "google"),
    },
    BuiltIn {
        name: "openai",
        protocol: "openai",
        variables: &["OPENAI_API_KEY"],
        included: cfg!(feature = "openai"),
    },
];

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
fn available(
    selection: &ProviderSelection,
) -> Result<Vec<(String, Box<dyn ModelProvider + Send>)>> {
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
    let mut providers: Vec<(String, Box<dyn ModelProvider + Send>)> = Vec::new();

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
            &selection.models,
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
fn auto(selection: &ProviderSelection) -> Result<Box<dyn ModelProvider + Send>> {
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

    if selection.announce_routing {
        eprintln!("{}", router.describe());
    }
    Ok(Box::new(router))
}

fn anthropic(selection: &ProviderSelection) -> Result<Box<dyn ModelProvider + Send>> {
    #[cfg(feature = "anthropic")]
    {
        Ok(Box::new(
            ingot_runtime::anthropic::AnthropicProvider::from_env()
                .map_err(anyhow::Error::msg)?
                .with_model(selection.model.clone())
                .with_effort(selection.effort.clone())
                .with_catalogue(selection.models.clone()),
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

fn openai(selection: &ProviderSelection) -> Result<Box<dyn ModelProvider + Send>> {
    #[cfg(feature = "openai")]
    {
        Ok(Box::new(
            ingot_runtime::openai::OpenAiProvider::from_env()
                .map_err(anyhow::Error::msg)?
                .with_model(selection.model.clone())
                .with_effort(selection.effort.clone())
                .with_catalogue(selection.models.clone()),
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

fn google(selection: &ProviderSelection) -> Result<Box<dyn ModelProvider + Send>> {
    #[cfg(feature = "google")]
    {
        Ok(Box::new(
            ingot_runtime::google::GoogleProvider::from_env()
                .map_err(anyhow::Error::msg)?
                .with_model(selection.model.clone())
                .with_effort(selection.effort.clone())
                .with_catalogue(selection.models.clone()),
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
        // A person's recorded answers, served the way the model's and the tools'
        // are. Without this `ingot test` denied every question and an artifact
        // holding a `consult` could not be tested at all — while
        // `ingot run --provider replay` replayed the same cassette happily.
        // [RFC-0020](../../../rfcs/0020-a-person-in-the-loop.md) settled which of
        // those two is right: *how does an artifact containing a `consult` run in
        // CI at all? It replays.* This is the command that is CI.
        let recorded_answers = cassette.consultations.clone();
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
                // Strict, so an edited question fails the test rather than
                // reusing the answer somebody gave to a different one — which is
                // the whole reason a consultation carries a digest. Gates are
                // approved rather than denied, as they are on any replay: the
                // effect happened once already, under somebody who said yes, and
                // what a replayed tool call returns comes from the recording
                // rather than from the tool.
                approval: HumanChannel::Ask(Box::new(ReplayInterlocutor::new(recorded_answers))),
                max_steps: 1_000,
                // No store. A test is offline and repeatable, and a run that
                // started from whatever a previous run happened to leave on
                // disk would be neither. A test runs to the end, so it never
                // stops at a checkpoint either.
                memory: std::collections::BTreeMap::new(),
                stop_at: None,
                resume: None,
                // The same prices a live run uses, so `cost <= 5 usd` is a
                // property `ingot test` can hold the agent to rather than a
                // line nothing checks.
                pricing: config.pricing.clone(),
                // A test replays, so there is one tape and a fan-out has a
                // ceiling of one. Not stated as a rule about tests: it is the
                // absence of anything to build a second provider from.
                fan_out: FanOut::default(),
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
