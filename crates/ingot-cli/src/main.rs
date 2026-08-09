//! `ingot` — the command-line compiler.
//!
//! Exit codes are part of the contract, because CI depends on them:
//!
//! | Code | Meaning                                           |
//! |------|---------------------------------------------------|
//! | 0    | success                                           |
//! | 1    | the program has diagnostics that block the build  |
//! | 2    | the command itself failed (bad path, I/O, ...)    |

use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand, ValueEnum};
use ingot_compiler::{compile_path, compile_source, format_source, Compilation};
use ingot_diagnostics::{codes, ColorChoice as RenderColor};

mod authoring;
mod contained;
mod dev;
mod diff;
mod doctor;
mod image;
mod manifest;
mod run;
mod sandbox;
mod tools;
mod trace;

use manifest::{resolve_target, Manifest, Target, MANIFEST_NAME};
use run::{EventFormat, ProviderChoice, RunConfig, TestConfig};
use sandbox::SandboxConfig;

pub(crate) const EXIT_OK: u8 = 0;
pub(crate) const EXIT_DIAGNOSTICS: u8 = 1;
pub(crate) const EXIT_FAILURE: u8 = 2;

#[derive(Parser, Debug)]
#[command(
    name = "ingot",
    version,
    about = "Compile Ingot agent sources to portable Agent IR",
    long_about = "Ingot compiles a statically typed agent language to a target-neutral \
                  Agent IR. Types, effects, policy and budgets are checked before an \
                  agent ever runs."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,

    /// When to colour diagnostics.
    #[arg(long, value_enum, default_value_t = ColorMode::Auto, global = true)]
    color: ColorMode,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
enum ColorMode {
    Auto,
    Always,
    Never,
}

impl ColorMode {
    fn resolve(self) -> RenderColor {
        match self {
            ColorMode::Always => RenderColor::Always,
            ColorMode::Never => RenderColor::Never,
            ColorMode::Auto => {
                if std::io::stderr().is_terminal() && std::env::var_os("NO_COLOR").is_none() {
                    RenderColor::Always
                } else {
                    RenderColor::Never
                }
            }
        }
    }
}

/// `Run` carries far more flags than the others, so the enum is as large as its
/// largest variant. Boxing it to save a few hundred bytes on one value that is
/// constructed once per process, and doing so through clap's derive, costs more
/// clarity than it buys.
#[allow(clippy::large_enum_variant)]
#[derive(Subcommand, Debug)]
enum Command {
    /// Create a new agent project.
    Init(InitArgs),
    /// Create or review a model-assisted authoring proposal.
    New(NewArgs),
    /// Parse, type-check and validate policy without producing output.
    Check(PathArgs),
    /// Rewrite sources in canonical form.
    Fmt(FmtArgs),
    /// Compile to Agent IR and write it to the output directory.
    Build(BuildArgs),
    /// Print the Agent IR to standard output.
    Ir(IrArgs),
    /// Compile and execute the agent.
    Run(RunArgs),
    /// Replay recorded cassettes and check every one still runs.
    Test(TestArgs),
    /// Check everything a live or contained run needs without starting it.
    Doctor(DoctorArgs),
    /// Prepare the version-matched local image used by contained runs.
    Image(ImageArgs),
    /// Watch, check and build each source revision; optionally run good ones.
    Dev(DevArgs),
    /// Discover MCP schemas and preflight each tool the program declares.
    Tools(ToolsArgs),
    /// Show the boundary each tool server would run inside, derived from the
    /// agent's own policy.
    Sandbox(SandboxArgs),
    /// Explain a diagnostic code in full.
    Explain(ExplainArgs),
    /// The inside half of a supervised run. Not a way to run an agent.
    ///
    /// Hidden because invoking it directly does nothing useful: it reads its
    /// whole configuration from a supervisor on its standard streams, and
    /// without one it refuses. `ingot run --contained` is the command.
    #[command(hide = true)]
    Exec,
}

#[derive(Args, Debug)]
struct InitArgs {
    /// Directory to create. Use `.` to initialise the current directory.
    name: PathBuf,

    /// Maintained starting point for the new project.
    #[arg(long, value_enum, default_value_t = StarterTemplate::Brief)]
    template: StarterTemplate,
}

#[derive(Args, Debug)]
struct NewArgs {
    /// Workflow to author, e.g. "review pull requests for security issues".
    workflow: Vec<String>,

    /// Directory to create. Defaults to a name derived from the workflow.
    #[arg(long, value_name = "DIR", conflicts_with_all = ["previous", "project"])]
    out_dir: Option<PathBuf>,

    /// Maintained offline pattern to start from.
    ///
    /// Without `--provider` this is what `ingot new` writes, and no model call
    /// is made at all.
    #[arg(long, value_enum, conflicts_with_all = ["previous", "project", "provider"])]
    template: Option<StarterTemplate>,

    /// Propose a change to an existing project instead of creating one.
    ///
    /// Nothing is written: the proposal is printed as a diff of the entry
    /// source, and `--apply` is what writes it.
    #[arg(long, value_name = "DIR", conflicts_with = "previous")]
    project: Option<PathBuf>,

    /// Write the proposed source over the project's entry file.
    #[arg(long, requires = "project")]
    apply: bool,

    /// Existing `.ing` source to compare against when reviewing a candidate.
    #[arg(long, value_name = "PATH", requires = "candidate")]
    previous: Option<PathBuf>,

    /// Model-proposed `.ing` source to review before any repair loop applies it.
    #[arg(long, value_name = "PATH", requires = "previous")]
    candidate: Option<PathBuf>,

    /// Follow-up `.ing` source proposals to try after compiler diagnostics.
    #[arg(long = "repair-candidate", value_name = "PATH", requires = "candidate")]
    repair_candidates: Vec<PathBuf>,

    /// Maximum number of repair proposals the authoring loop may consume.
    #[arg(long, value_name = "N", default_value_t = 2)]
    max_repairs: usize,

    /// Where authored source comes from.
    ///
    /// Omitted, nothing reaches a model: `ingot new` writes a maintained
    /// offline template, and reviewing a candidate reads it from disk.
    #[arg(long, value_enum, conflicts_with = "previous")]
    provider: Option<ProviderChoice>,

    /// Authoring exchanges to replay, for `--provider replay`.
    #[arg(long, value_name = "FILE", requires = "provider")]
    cassette: Option<PathBuf>,

    /// Record the authoring exchanges, so the session can be reviewed or replayed.
    #[arg(long, value_name = "FILE", requires = "provider")]
    record: Option<PathBuf>,

    /// Override the model the provider would otherwise choose.
    #[arg(long, value_name = "MODEL", requires = "provider")]
    model: Option<String>,

    /// Reasoning effort: low, medium, high, xhigh or max.
    #[arg(long, value_name = "LEVEL", requires = "provider")]
    effort: Option<String>,

    /// Accept the policy grants the proposal asks for.
    ///
    /// Run once without it to see them. An acceptance given before the list was
    /// printed is not one, so this never applies to a proposal you have not read.
    #[arg(long)]
    accept_policy: bool,
}

#[derive(Args, Debug)]
struct ImageArgs {
    #[command(subcommand)]
    command: ImageCommand,
}

#[derive(Subcommand, Debug)]
enum ImageCommand {
    /// Build the reference image without downloading an unverified image.
    Build(ImageBuildArgs),
}

#[derive(Args, Debug)]
struct ImageBuildArgs {
    /// Ingot source checkout. Defaults to the nearest checkout.
    #[arg(value_name = "SOURCE")]
    source: Option<PathBuf>,
}

/// A small, maintained example of a language pattern rather than a vertical
/// product. Every template checks, builds and replays without a model key.
#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
enum StarterTemplate {
    /// One typed input, one model call, one markdown artifact.
    Brief,
    /// Two inputs and a checked-in document transformed for an audience.
    DocumentWorkflow,
}

#[derive(Args, Debug)]
struct PathArgs {
    /// A `.ing` file or a project directory. Defaults to the nearest project.
    path: Option<PathBuf>,
}

#[derive(Args, Debug)]
struct ToolsArgs {
    #[command(flatten)]
    target: PathArgs,

    /// Print one stable discovery and preflight report for editors and CI.
    #[arg(long)]
    json: bool,

    /// Show editable source and manifest proposals; never write them.
    #[arg(long)]
    propose: bool,
}

#[derive(Args, Debug)]
struct FmtArgs {
    #[command(flatten)]
    target: PathArgs,
    /// Report files that are not formatted instead of rewriting them.
    #[arg(long)]
    check: bool,
}

#[derive(Args, Debug)]
struct BuildArgs {
    #[command(flatten)]
    target: PathArgs,
    /// Override the output directory.
    #[arg(long, value_name = "DIR")]
    out_dir: Option<PathBuf>,

    /// What to compile to.
    ///
    /// `ir` writes the target-neutral Agent IR, which is what every backend
    /// consumes. `python` writes one self-contained Python 3 program per agent.
    #[arg(long = "target", value_enum, default_value_t = BuildTarget::Ir)]
    backend: BuildTarget,

    /// Build anyway when the target does not implement something the agent uses.
    ///
    /// The report says what, and the resulting program will not do it. Refused by
    /// default, because a silently dropped construct is worse than a failed build.
    #[arg(long)]
    allow_unimplemented: bool,

    /// Print the portability report as JSON instead of prose.
    ///
    /// `ingot build --target python --json | jq -e '.unimplemented == []'` is a
    /// deployment gate.
    #[arg(long)]
    json: bool,
}

/// What `ingot build` compiles to.
#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
enum BuildTarget {
    /// The target-neutral Agent IR. The default, and what every backend reads.
    Ir,
    /// A self-contained Python 3 program per agent.
    Python,
}

#[derive(Args, Debug)]
struct IrArgs {
    #[command(flatten)]
    target: PathArgs,
    /// Which agent to print when the file declares several.
    #[arg(long, value_name = "NAME")]
    agent: Option<String>,
}

#[derive(Args, Debug)]
struct RunArgs {
    #[command(flatten)]
    target: PathArgs,

    /// An agent input, as `name=value`. Repeat for each one.
    ///
    /// The value is parsed as JSON when it is valid JSON, and taken as a plain
    /// string otherwise. Prefix with `@` to read it from a file:
    /// `--input document=@report.txt`.
    #[arg(long = "input", short = 'i', value_name = "NAME=VALUE")]
    inputs: Vec<String>,

    /// Where completions come from.
    ///
    /// `auto` sends each call to the vendor the artifact pinned with
    /// `model exact "<vendor>/<model>"`, using whichever API keys are exported.
    #[arg(long, value_enum, default_value_t = ProviderChoice::Auto)]
    provider: ProviderChoice,

    /// Cassette to replay, for `--provider replay`.
    #[arg(long, value_name = "FILE")]
    cassette: Option<PathBuf>,

    /// Record this run to a cassette so it can be replayed offline.
    #[arg(long, value_name = "FILE")]
    record: Option<PathBuf>,

    /// Override the model the artifact asks for.
    #[arg(long, value_name = "MODEL")]
    model: Option<String>,

    /// Reasoning effort: low, medium, high, xhigh or max.
    #[arg(long, value_name = "LEVEL")]
    effort: Option<String>,

    /// Which agent to run when the file declares several. Defaults to the last.
    #[arg(long, value_name = "NAME")]
    agent: Option<String>,

    /// Write artifacts here instead of to standard output.
    #[arg(long, value_name = "DIR")]
    out_dir: Option<PathBuf>,

    /// How progress is reported on stderr.
    #[arg(long, value_enum, default_value_t = EventFormat::Text)]
    events: EventFormat,

    /// Approve every gate without asking. The artifact asked for a human, so
    /// this is deliberately explicit.
    #[arg(long)]
    yes: bool,

    /// Start no MCP server, whatever the manifest configures. Useful for
    /// checking that an agent fails the way it should when a tool is absent.
    #[arg(long)]
    no_tools: bool,

    /// Run each tool server inside a boundary derived from the agent's policy.
    ///
    /// Needs a container runtime and an `image` on each server. `ingot sandbox`
    /// shows what the boundary would be.
    #[arg(long, conflicts_with_all = ["contained", "supervised"])]
    sandbox: bool,

    /// Run the agent itself inside a boundary derived from its policy.
    ///
    /// Everything is inside: the interpreter, the tool servers, and nothing
    /// else. The model call and the approval gate cross out through a
    /// supervisor, so `network deny` holds and the API key never enters the box.
    /// Needs a container runtime. Uses the version-matched reference image
    /// unless `[run] image` or `--image` deliberately selects another.
    #[arg(long)]
    contained: bool,

    /// The image a contained run happens inside.
    #[arg(long, value_name = "IMAGE")]
    image: Option<String>,

    /// Run over the supervisor channel with no boundary at all.
    ///
    /// For proving the channel works where there is no container runtime. It
    /// enforces nothing and says so on every run.
    #[arg(long, hide = true, conflicts_with = "contained")]
    supervised: bool,

    /// Proceed even where the boundary cannot honour a rule the policy states.
    ///
    /// Applies to `--sandbox` and `--contained`. Refused on its own rather than
    /// ignored: a flag that looks like it loosened something and did nothing is
    /// worse than an error.
    #[arg(long)]
    sandbox_allow_unenforced: bool,

    /// The root the artifact's policy paths are relative to.
    #[arg(long, value_name = "DIR")]
    workspace: Option<PathBuf>,

    /// Stop after this many steps, whatever the artifact's own budget allows.
    #[arg(long, default_value_t = 1000, value_name = "N")]
    max_steps: u32,
}

#[derive(Args, Debug)]
struct TestArgs {
    #[command(flatten)]
    target: PathArgs,

    /// Directory of cassettes to replay.
    #[arg(long, value_name = "DIR", default_value = "tests/cassettes")]
    cassettes: PathBuf,

    /// Only run cassettes whose name contains this substring.
    #[arg(value_name = "FILTER")]
    filter: Option<String>,
}

#[derive(Args, Debug)]
struct DoctorArgs {
    #[command(flatten)]
    target: PathArgs,

    /// Print one stable JSON report for editors and CI.
    #[arg(long)]
    json: bool,
}

#[derive(Args, Debug)]
struct DevArgs {
    #[command(flatten)]
    target: PathArgs,

    /// Run every successfully built revision. Off by default: saving a prompt
    /// must not silently make a model call.
    #[arg(long)]
    run: bool,

    /// An example input as `name=value`; repeat for each input.
    #[arg(
        long = "input",
        short = 'i',
        value_name = "NAME=VALUE",
        requires = "run"
    )]
    inputs: Vec<String>,

    /// Where opt-in runs get completions.
    #[arg(long, value_enum, default_value_t = ProviderChoice::Auto)]
    provider: ProviderChoice,

    /// Cassette used when `--provider replay` is selected.
    #[arg(long, value_name = "FILE", requires = "run")]
    cassette: Option<PathBuf>,

    /// Agent to run when the source declares several.
    #[arg(long, value_name = "NAME", requires = "run")]
    agent: Option<String>,

    /// Progress detail for opt-in runs.
    #[arg(long, value_enum, default_value_t = EventFormat::Quiet)]
    events: EventFormat,

    /// Approve every gate during opt-in runs without prompting.
    #[arg(long, requires = "run")]
    yes: bool,

    /// Stop an opt-in run after this many steps.
    #[arg(long, default_value_t = 1000, value_name = "N")]
    max_steps: u32,
}

#[derive(Args, Debug)]
struct SandboxArgs {
    #[command(flatten)]
    target: PathArgs,

    /// The root the artifact's policy paths are relative to.
    ///
    /// An artifact says `src`; this says where `src` lives. Defaults to the
    /// project directory.
    #[arg(long, value_name = "DIR")]
    workspace: Option<PathBuf>,

    /// Print the plans as JSON, for piping.
    #[arg(long)]
    json: bool,
}

#[derive(Args, Debug)]
struct ExplainArgs {
    /// A diagnostic code such as `ING4001`.
    code: String,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let color = cli.color.resolve();

    let result = match &cli.command {
        Command::Init(args) => run_init(args),
        Command::New(args) => run_new(args, color),
        Command::Check(args) => run_check(args, color),
        Command::Fmt(args) => run_fmt(args, color),
        Command::Build(args) => run_build(args, color),
        Command::Ir(args) => run_ir(args, color),
        Command::Run(args) => run_run(args, color),
        Command::Test(args) => run_test(args, color),
        Command::Doctor(args) => run_doctor(args, color),
        Command::Image(args) => run_image(args),
        Command::Dev(args) => run_dev(args, color),
        Command::Tools(args) => run_tools(args, color),
        Command::Sandbox(args) => run_sandbox(args, color),
        Command::Explain(args) => run_explain(args),
        Command::Exec => contained::exec(),
    };

    match result {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::from(EXIT_FAILURE)
        }
    }
}

// --- init ------------------------------------------------------------------

fn run_init(args: &InitArgs) -> Result<u8> {
    let dir = &args.name;
    let name = project_name_for_dir(dir);
    create_starter_project(dir, &name, args.template, args.template.description())?;

    println!(
        "Created agent project `{name}` from template `{}` in {}",
        args.template.as_str(),
        dir.display()
    );
    println!();
    println!("Next steps:");
    if dir != Path::new(".") {
        println!("  cd {}", dir.display());
    }
    println!("  ingot check");
    println!("  ingot build");
    println!("  ingot test");
    Ok(EXIT_OK)
}

/// The cassette agent name for a recorded authoring session.
///
/// Not an agent in the language — authoring happens before there is one — but a
/// recorded session sits next to run cassettes and has to be identifiable.
const AUTHORING_AGENT: &str = "ingot.authoring";

fn run_new(args: &NewArgs, color: RenderColor) -> Result<u8> {
    if args.previous.is_some() {
        return review_candidate_files(args, color);
    }
    if let Some(project) = &args.project {
        return propose_into_project(args, project, color);
    }
    if !args.repair_candidates.is_empty() {
        bail!("--repair-candidate requires --previous and --candidate");
    }
    create_from_workflow(args, color)
}

/// The workflow as one string, refused when it is empty or carries a secret.
///
/// The scan happens before the words reach a prompt, a manifest or a log: a key
/// pasted into a workflow description is the likeliest way one would enter this
/// command, and the only useful moment to stop it is the first one.
fn workflow_words(args: &NewArgs) -> Result<String> {
    let workflow = args.workflow.join(" ");
    if workflow.trim().is_empty() {
        bail!(
            "describe the workflow to author, or pass --previous and --candidate to review a \
             proposal"
        );
    }
    if let Some(finding) = authoring::scan_for_credentials(&workflow) {
        bail!(
            "the workflow description contains {} and was not sent anywhere\n  \
             remove it and describe the credential by name instead; a value in a workflow \
             would reach the prompt, the manifest and this terminal's history",
            finding.shape
        );
    }
    Ok(workflow)
}

// --- new: reviewing candidate files ----------------------------------------

fn review_candidate_files(args: &NewArgs, color: RenderColor) -> Result<u8> {
    let previous = args.previous.as_ref().expect("checked by the caller");
    let candidate = args
        .candidate
        .as_ref()
        .expect("clap requires --candidate with --previous");

    let previous_source = std::fs::read_to_string(previous)
        .with_context(|| format!("reading {}", previous.display()))?;
    let candidate_source = std::fs::read_to_string(candidate)
        .with_context(|| format!("reading {}", candidate.display()))?;
    let repair_sources = args
        .repair_candidates
        .iter()
        .map(|path| {
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))
        })
        .collect::<Result<Vec<_>>>()?;

    let mut candidates = authoring::FixedCandidates::new(&candidate_source, &repair_sources);
    // Two loose files are not a project, so there is no routing table to hold a
    // tool declaration against and none is invented.
    let repair = authoring::author(
        &previous_source,
        &mut candidates,
        &authoring::ToolContext::Unchecked,
        limits(args),
    )?;

    if let Some(code) = report_authoring(&repair, 0, color) {
        return Ok(code);
    }

    let source = repair
        .accepted_source()
        .expect("a compiled loop has source");
    println!("candidate source contains no new policy proposal");
    println!(
        "compiler-verified authoring completed after {} attempt(s)",
        repair.attempts().len()
    );
    print_authoring_attempts(&repair);
    print_proposed_diff(&previous.display().to_string(), &previous_source, source);
    Ok(EXIT_OK)
}

// --- new: proposing into an existing project -------------------------------

fn propose_into_project(args: &NewArgs, project: &Path, color: RenderColor) -> Result<u8> {
    if args.provider.is_none() {
        bail!(
            "--project needs --provider: a maintained template can start a project, but only a \
             model can propose a change to one that already exists\n  \
             use `--provider auto` with a key exported, or `--provider replay --cassette <FILE>`"
        );
    }
    let workflow = workflow_words(args)?;
    let target = resolve_target(Some(project))?;
    let previous_source = std::fs::read_to_string(&target.entry)
        .with_context(|| format!("reading {}", target.entry.display()))?;

    // Real schemas or none: a proposal written against invented tools compiles
    // and cannot run, and the failure would arrive at run time in front of
    // whoever trusted the generator.
    let mcp = target.mcp();
    let tools = if mcp.is_empty() {
        authoring::ToolContext::NoServers
    } else {
        authoring::ToolContext::Routed(tools::routed(&tools::ToolsConfig {
            root: target.root.clone(),
            mcp,
        })?)
    };

    let package = target
        .manifest
        .as_ref()
        .and_then(|manifest| package_name(&manifest.project.name));
    let session = author_with_model(args, &workflow, &previous_source, package, &tools)?;

    if let Some(code) = report_authoring(&session.repair, session.calls, color) {
        return Ok(code);
    }
    let source = session
        .repair
        .accepted_source()
        .expect("a compiled loop has source");

    let entry = target.entry.display().to_string();
    if !print_proposed_diff(&entry, &previous_source, source) {
        println!("the proposal makes no change to {entry}");
        return Ok(EXIT_OK);
    }

    if !args.apply {
        println!();
        println!("nothing was written; re-run with --apply to write this to {entry}");
        return Ok(EXIT_OK);
    }

    std::fs::write(&target.entry, source).with_context(|| format!("writing {entry}"))?;
    println!();
    println!("wrote {entry}");
    println!("check the result and re-record any cassette the change invalidates:");
    println!("  ingot check");
    println!("  ingot test");
    Ok(EXIT_OK)
}

// --- new: creating a project ------------------------------------------------

fn create_from_workflow(args: &NewArgs, color: RenderColor) -> Result<u8> {
    let workflow = workflow_words(args)?;
    let dir = args
        .out_dir
        .clone()
        .unwrap_or_else(|| PathBuf::from(project_slug(&workflow)));
    let name = project_name_for_dir(&dir);
    let description = format!("Authored from workflow: {workflow}");

    let Some(_) = args.provider else {
        let template = args
            .template
            .unwrap_or_else(|| StarterTemplate::for_workflow(&workflow));
        create_starter_project(&dir, &name, template, &description)?;

        println!(
            "Created compiler-verified agent project `{name}` from workflow in {}",
            dir.display()
        );
        println!("Workflow: {workflow}");
        println!("Template: {}", template.as_str());
        println!();
        println!("Next steps:");
        if dir != Path::new(".") {
            println!("  cd {}", dir.display());
        }
        println!("  ingot check");
        println!("  ingot build");
        println!("  ingot test");
        return Ok(EXIT_OK);
    };

    // Settled before the model is asked for anything: a run that would refuse to
    // write its result should not spend a call finding that out.
    if dir.join(MANIFEST_NAME).exists() {
        bail!("{} already contains an {MANIFEST_NAME}", dir.display());
    }

    // A project that does not exist yet configures no tool server, so an
    // authored `tool` declaration cannot be routed by anything.
    let tools = authoring::ToolContext::NoServers;
    let session = author_with_model(args, &workflow, "", package_name(&name), &tools)?;

    if let Some(code) = report_authoring(&session.repair, session.calls, color) {
        return Ok(code);
    }
    let source = session
        .repair
        .accepted_source()
        .expect("a compiled loop has source");

    let compilation = compile_source("main.ing", source);
    let inputs = compilation
        .agents
        .first()
        .map(|agent| agent.inputs.clone())
        .unwrap_or_default();

    let mut manifest = Manifest::new(&name);
    manifest.project.description = Some(description);
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    write_new(&dir.join(MANIFEST_NAME), &manifest.to_toml())?;
    write_new(&dir.join("main.ing"), source)?;
    write_new(&dir.join(".gitignore"), "/target\n")?;
    for (input, ty) in &inputs {
        if let Some(path) = example_input_path(input, ty) {
            write_new(&dir.join(&path), &example_input(input, &workflow))?;
        }
    }
    write_new(
        &dir.join("README.md"),
        &authored_readme(&name, &workflow, &inputs),
    )?;

    println!(
        "Created compiler-verified agent project `{name}` from workflow in {}",
        dir.display()
    );
    println!("Workflow: {workflow}");
    println!("Authored by: {}", provider_label(args));
    println!();
    println!("Next steps:");
    if dir != Path::new(".") {
        println!("  cd {}", dir.display());
    }
    println!("  ingot check");
    println!("  ingot build");
    println!("  ingot test");
    println!();
    println!(
        "`ingot test` has no cassette to replay yet, and one is not invented: a recorded \
         answer nothing produced would be a test that proves nothing."
    );
    println!("Record the offline test once, against a configured provider:");
    println!("  {}", record_command(&inputs));
    Ok(EXIT_OK)
}

fn provider_label(args: &NewArgs) -> &'static str {
    match args.provider {
        Some(ProviderChoice::Replay) => "a replayed authoring cassette",
        Some(_) => "a model, verified by the compiler",
        None => "a maintained template",
    }
}

// --- new: the shared authoring loop ----------------------------------------

struct AuthoringSession {
    repair: authoring::RepairLoop,
    calls: usize,
}

fn limits(args: &NewArgs) -> authoring::Limits {
    authoring::Limits {
        max_repairs: args.max_repairs,
        accept_policy: args.accept_policy,
    }
}

/// Ask a provider for source and run it through the same bounded loop the
/// file-backed review uses.
fn author_with_model(
    args: &NewArgs,
    workflow: &str,
    previous_source: &str,
    package: Option<String>,
    tools: &authoring::ToolContext,
) -> Result<AuthoringSession> {
    let selection = run::ProviderSelection {
        choice: args.provider.expect("checked by the caller"),
        cassette: args.cassette.clone(),
        model: args.model.clone(),
        effort: args.effort.clone(),
        // Authoring reads no manifest-declared provider: it may be creating the
        // manifest, and a proposal into an existing project must not depend on
        // one having been declared.
        models: ingot_runtime::ModelConfig::default(),
        strict_replay: false,
    };
    let mut provider = run::Provider::new(
        run::build_model_provider(&selection)?,
        args.record.is_some(),
        AUTHORING_AGENT,
    );

    let (repair, calls) = {
        let request = authoring::AuthoringRequest {
            workflow: workflow.to_string(),
            previous_source: previous_source.to_string(),
            package,
        };
        let mut model = authoring::ModelAuthor::new(provider.as_mut(), request, tools);
        let repair = authoring::author(previous_source, &mut model, tools, limits(args));
        (repair, model.calls())
    };

    // The recording is saved whatever the loop decided. A session that ended in
    // a refusal is the one most worth being able to read again.
    if let Some(path) = &args.record {
        if let Some(cassette) = provider.finish_recording() {
            cassette.save(path).map_err(anyhow::Error::msg)?;
            eprintln!("recorded the authoring session to {}", path.display());
        }
    }

    Ok(AuthoringSession {
        repair: repair?,
        calls,
    })
}

/// Print what the loop did. `Some(code)` when the outcome stops the command.
fn report_authoring(
    repair: &authoring::RepairLoop,
    calls: usize,
    color: RenderColor,
) -> Option<u8> {
    if calls > 0 {
        let usage = repair.usage();
        eprintln!(
            "authoring made {calls} model call(s), using {} input and {} output token(s)",
            usage.input_tokens, usage.output_tokens
        );
    }
    for proposal in repair.accepted_proposals() {
        println!(
            "accepted policy grant: agent {}: {} {}",
            proposal.agent, proposal.subject, proposal.action
        );
    }

    match repair.outcome() {
        authoring::RepairOutcome::Compiled { .. } => None,
        authoring::RepairOutcome::PolicyProposals { proposals } => {
            println!("candidate source requests policy changes");
            println!("these are not part of automatic compiler repair:");
            for proposal in proposals {
                println!(
                    "  agent {}: {} {}",
                    proposal.agent, proposal.subject, proposal.action
                );
            }
            println!();
            println!("review and accept policy changes explicitly before continuing");
            println!("re-run with --accept-policy to accept exactly these grants");
            Some(EXIT_DIAGNOSTICS)
        }
        authoring::RepairOutcome::RetryCeilingReached => {
            print_authoring_attempts(repair);
            println!(
                "compiler repair reached retry ceiling after {} attempt(s)",
                repair.attempts().len()
            );
            if let Some(last) = repair.attempts().last() {
                println!("last source:");
                println!("{}", last.source);
                let compilation = compile_source("candidate.ing", &last.source);
                eprint!("{}", compilation.render_diagnostics(color));
            }
            Some(EXIT_DIAGNOSTICS)
        }
        authoring::RepairOutcome::CredentialRefused { finding } => {
            println!(
                "the proposed source contains {} on line {}",
                finding.shape, finding.line
            );
            println!("nothing was written, and the source was not sent back to the model");
            println!(
                "a credential belongs in the environment, named by `pass-env` in {MANIFEST_NAME}, \
                 never in source"
            );
            Some(EXIT_DIAGNOSTICS)
        }
    }
}

fn print_authoring_attempts(repair: &authoring::RepairLoop) {
    for attempt in repair.attempts() {
        if attempt.has_errors() {
            println!("attempt {} failed compiler verification", attempt.number);
            for diagnostic in &attempt.diagnostics {
                println!("  {}: {}", diagnostic.code, diagnostic.message);
            }
        } else {
            println!("attempt {} passed compiler verification", attempt.number);
        }
    }
}

/// Show the change rather than the result. Returns whether there was one.
fn print_proposed_diff(label: &str, previous: &str, proposed: &str) -> bool {
    match diff::unified(label, previous, "proposed", proposed, diff::CONTEXT) {
        Some(rendered) => {
            print!("{rendered}");
            true
        }
        None => false,
    }
}

fn run_image(args: &ImageArgs) -> Result<u8> {
    match &args.command {
        ImageCommand::Build(args) => image::build(args.source.as_deref()),
    }
}

fn create_starter_project(
    dir: &Path,
    name: &str,
    template: StarterTemplate,
    description: &str,
) -> Result<()> {
    if dir.join(MANIFEST_NAME).exists() {
        bail!("{} already contains an {MANIFEST_NAME}", dir.display());
    }
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;

    let mut manifest = Manifest::new(name);
    manifest.project.description = Some(description.to_string());
    write_new(&dir.join(MANIFEST_NAME), &manifest.to_toml())?;
    write_new(&dir.join("main.ing"), &starter_source(name, template))?;
    write_new(&dir.join(".gitignore"), "/target\n")?;
    write_new(&dir.join("README.md"), &starter_readme(name, template))?;
    write_new(
        &dir.join("tests/cassettes/example.json"),
        &starter_cassette(name, template).to_canonical_json(),
    )?;
    if let Some((path, contents)) = template.example_file() {
        write_new(&dir.join(path), contents)?;
    }
    Ok(())
}

fn write_new(path: &Path, contents: &str) -> Result<()> {
    if path.exists() {
        bail!("{} already exists", path.display());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    std::fs::write(path, contents).with_context(|| format!("writing {}", path.display()))
}

const EXAMPLE_DOCUMENT: &str = "Ingot compiles a typed agent language to portable Agent IR. \
The same checked artifact can run through independent backends. Policies and \
budgets travel with the artifact so each backend can enforce them.\n";

impl StarterTemplate {
    fn as_str(self) -> &'static str {
        match self {
            StarterTemplate::Brief => "brief",
            StarterTemplate::DocumentWorkflow => "document-workflow",
        }
    }

    fn description(self) -> &'static str {
        match self {
            StarterTemplate::Brief => "A small typed agent that turns a topic into a brief.",
            StarterTemplate::DocumentWorkflow => {
                "A document transformation workflow with two typed inputs."
            }
        }
    }

    fn agent(self) -> &'static str {
        match self {
            StarterTemplate::Brief => "Brief",
            StarterTemplate::DocumentWorkflow => "DocumentWorkflow",
        }
    }

    fn example_file(self) -> Option<(&'static str, &'static str)> {
        match self {
            StarterTemplate::Brief => None,
            StarterTemplate::DocumentWorkflow => Some(("examples/document.txt", EXAMPLE_DOCUMENT)),
        }
    }

    fn for_workflow(workflow: &str) -> StarterTemplate {
        let lowered = workflow.to_ascii_lowercase();
        if [
            "document",
            "documents",
            "doc",
            "docs",
            "file",
            "files",
            "audience",
            "summarise",
            "summarize",
        ]
        .iter()
        .any(|needle| lowered.contains(needle))
        {
            StarterTemplate::DocumentWorkflow
        } else {
            StarterTemplate::Brief
        }
    }
}

fn project_name_for_dir(dir: &Path) -> String {
    dir.file_name()
        .map(|name| name.to_string_lossy().to_string())
        .filter(|name| name != ".")
        .or_else(|| {
            std::env::current_dir()
                .ok()
                .and_then(|dir| dir.file_name().map(|n| n.to_string_lossy().to_string()))
        })
        .unwrap_or_else(|| "agent".to_string())
}

fn project_slug(workflow: &str) -> String {
    let words: Vec<String> = workflow
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
        .take(5)
        .map(|word| word.to_ascii_lowercase())
        .collect();
    if words.is_empty() {
        "authored-agent".to_string()
    } else {
        words.join("-")
    }
}

/// A package identifier derived from a project name, if one can be.
///
/// Returns `None` when the name cannot become a valid identifier — it is empty,
/// starts with a digit, or collides with a reserved word. `package` is optional
/// in the language, so omitting it beats generating source that will not
/// compile. `ingot init agent` used to produce `package agent`, which is a
/// syntax error.
fn package_name(name: &str) -> Option<String> {
    let sanitised: String = name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    let sanitised = sanitised.trim_matches('_').to_string();

    if sanitised.is_empty() {
        return None;
    }
    if sanitised.starts_with(|ch: char| ch.is_ascii_digit()) {
        return None;
    }
    if ingot_lexer::KEYWORDS.contains(&sanitised.as_str()) {
        return None;
    }
    Some(sanitised)
}

fn starter_source(name: &str, template: StarterTemplate) -> String {
    let package = match package_name(name) {
        Some(package) => format!(
            "package {package}
"
        ),
        None => String::new(),
    };
    match template {
        StarterTemplate::Brief => format!(
            r#"language 0.1
{package}
/// Summarises a topic into a short markdown brief.
agent Brief(topic: string) -> brief<markdown> {{
  model requires {{
    structured_output
  }}

  budget {{
    steps <= 4
    tokens <= 20000
  }}

  policy {{
    network deny
  }}

  flow {{
    emit brief = ask<markdown>(
      "Write a short, factual brief about ${{topic}}. Use headings and bullet points."
    )
  }}
}}
"#
        ),
        StarterTemplate::DocumentWorkflow => format!(
            r#"language 0.1
{package}
/// Rewrites a document for a named audience without changing its facts.
agent DocumentWorkflow(document: text, audience: string) -> summary<markdown> {{
  model requires {{
    structured_output
  }}

  budget {{
    steps <= 4
    tokens <= 20000
  }}

  policy {{
    network deny
  }}

  flow {{
    emit summary = ask<markdown>(
      "Summarise the following document for ${{audience}}. Preserve the important facts.\n\n${{document}}"
    )
  }}
}}
"#
        ),
    }
}

fn starter_readme(name: &str, template: StarterTemplate) -> String {
    let replay = match template {
        StarterTemplate::Brief => {
            "ingot run --provider replay --cassette tests/cassettes/example.json --input topic=\"compiler design\""
        }
        StarterTemplate::DocumentWorkflow => {
            "ingot run --provider replay --cassette tests/cassettes/example.json --input document=@examples/document.txt --input audience=\"project leads\""
        }
    };
    let dev_replay = replay.replacen("ingot run", "ingot dev --run", 1);
    format!(
        r#"# {name}

An agent written in Ingot from the `{}` template. `main.ing` is the source of
truth: the template, compiler, test and runtime do not hide another workflow
representation behind it.

## First run

These commands work without a model API key:

```bash
ingot check
ingot build
ingot test
{replay}
```

`ingot test` replays the reviewed fixture in `tests/cassettes/`. The final
command runs that same fixture and prints the artifact. Change `main.ing`, then
record a new cassette against a configured provider before accepting its diff.

## Develop

Keep `check` and the canonical IR build current while editing:

```bash
ingot dev
```

Running is opt-in, so a save does not silently call a model. This command runs
each successful revision against the checked-in cassette and example inputs:

```bash
{dev_replay}
```

`ingot build` writes `target/ingot/{}.ir.json`. Agent IR is the canonical,
target-neutral artifact consumed by every backend.
"#,
        template.as_str(),
        template.agent()
    )
}

// --- files for a model-authored project ------------------------------------

/// Where an example value for this input belongs, when a file is the natural
/// way to pass one.
///
/// Prose inputs get a file because `--input document=@examples/document.txt` is
/// how anyone would really pass a document. A scalar goes on the command line,
/// where it is easier to change than in a file nobody remembers exists.
fn example_input_path(name: &str, ty: &str) -> Option<PathBuf> {
    matches!(ty, "text" | "markdown").then(|| PathBuf::from(format!("examples/{name}.txt")))
}

fn example_input(name: &str, workflow: &str) -> String {
    format!(
        "Example `{name}` for: {workflow}\n\n\
         Replace this with real content. It exists so the first run has something \
         to read, and so the recorded cassette is made against a value you chose.\n"
    )
}

/// The `--input` flag for one declared input, with a value of the right shape.
fn input_flag(name: &str, ty: &str) -> String {
    let value = match ty {
        "text" | "markdown" => format!("@examples/{name}.txt"),
        "string" => "\"...\"".to_string(),
        "int" => "0".to_string(),
        "float" => "0.0".to_string(),
        "bool" => "true".to_string(),
        ty if ty.ends_with("[]") => "[]".to_string(),
        _ => "{}".to_string(),
    };
    format!("--input {name}={value}")
}

fn input_flags(inputs: &std::collections::BTreeMap<String, String>) -> String {
    inputs
        .iter()
        .map(|(name, ty)| input_flag(name, ty))
        .collect::<Vec<_>>()
        .join(" ")
}

/// The one command that turns an authored project into a project with an
/// offline test.
fn record_command(inputs: &std::collections::BTreeMap<String, String>) -> String {
    let flags = input_flags(inputs);
    let separator = if flags.is_empty() { "" } else { " " };
    format!("ingot run --record tests/cassettes/example.json{separator}{flags}")
}

fn authored_readme(
    name: &str,
    workflow: &str,
    inputs: &std::collections::BTreeMap<String, String>,
) -> String {
    let record = record_command(inputs);
    let replay = {
        let flags = input_flags(inputs);
        let separator = if flags.is_empty() { "" } else { " " };
        format!(
            "ingot run --provider replay --cassette tests/cassettes/example.json{separator}{flags}"
        )
    };
    format!(
        r#"# {name}

An agent written in Ingot, authored from this workflow:

> {workflow}

`main.ing` is the source of truth. The authoring model wrote it once and has no
further part in this project: the compiler, tests and runtime never call it, and
every command below works without one.

## First run

These commands need no model API key:

```bash
ingot check
ingot build
```

`ingot build` writes the canonical, target-neutral Agent IR under
`target/ingot/`.

## The offline test

There is no cassette yet, and one was not invented for you: a recorded answer
that no model produced would be a test that proves nothing. Record one against a
configured provider, review the answer it captured, and commit it:

```bash
{record}
```

After that, the project replays with no key and no network:

```bash
ingot test
{replay}
```

## Develop

```bash
ingot dev
```

Keeps `check` and the IR build current while you edit. Running is opt-in, so
saving a prompt never silently calls a model. When you change a prompt, the
recorded cassette stops matching on purpose — re-record it and review the diff.

## Changing it with the model again

```bash
ingot new --project . --provider auto "what you want changed"
```

That prints a diff and writes nothing until you pass `--apply`.
"#
    )
}

fn starter_cassette(name: &str, template: StarterTemplate) -> ingot_runtime::Cassette {
    use std::collections::BTreeMap;

    use ingot_runtime::{
        schema::ResponseShape, CompletionRequest, Interaction, ModelSelection, Usage,
    };
    use serde_json::json;

    let (inputs, prompt, value) = match template {
        StarterTemplate::Brief => {
            let inputs: BTreeMap<String, serde_json::Value> =
                [("topic".to_string(), json!("compiler design"))].into();
            (
                inputs,
                "Write a short, factual brief about compiler design. Use headings and bullet points."
                    .to_string(),
                json!("# Compiler design\n\n- A front end understands source.\n- An intermediate representation connects analysis to execution.\n- Backends let one checked program reach more than one target."),
            )
        }
        StarterTemplate::DocumentWorkflow => {
            let inputs: BTreeMap<String, serde_json::Value> = [
                ("audience".to_string(), json!("project leads")),
                ("document".to_string(), json!(EXAMPLE_DOCUMENT)),
            ]
            .into();
            (
                inputs,
                format!(
                    "Summarise the following document for project leads. Preserve the important facts.\n\n{EXAMPLE_DOCUMENT}"
                ),
                json!("# Project brief\n\nIngot turns typed agent source into portable Agent IR. Independent backends consume the same checked artifact, including its policy and budget declarations."),
            )
        }
    };

    let request = CompletionRequest {
        node: "n0".to_string(),
        model: ModelSelection::Capabilities {
            capabilities: vec!["structured_output".to_string()],
            min_context_tokens: None,
        },
        system: None,
        prompt,
        context: Vec::new(),
        response_type: "markdown".to_string(),
        shape: ResponseShape::Prose,
        max_tokens: 20_000,
    };
    let qualified_agent = match package_name(name) {
        Some(package) => format!("{package}.{}", template.agent()),
        None => template.agent().to_string(),
    };
    let mut cassette = ingot_runtime::Cassette::new(qualified_agent);
    cassette.inputs = inputs;
    cassette.interactions.push(Interaction {
        index: 0,
        node: request.node.clone(),
        request_digest: request.digest(),
        response_type: request.response_type,
        value,
        usage: Usage {
            input_tokens: 120,
            output_tokens: 60,
            cache_read_tokens: 0,
        },
        model: Some("template/replay".to_string()),
    });
    cassette
}

// --- check -----------------------------------------------------------------

fn run_check(args: &PathArgs, color: RenderColor) -> Result<u8> {
    let target = resolve_target(args.path.as_deref())?;
    let compilation = compile(&target)?;
    report(&compilation, color);
    Ok(exit_code(&compilation))
}

// --- fmt -------------------------------------------------------------------

fn run_fmt(args: &FmtArgs, color: RenderColor) -> Result<u8> {
    let target = resolve_target(args.target.path.as_deref())?;
    let original = std::fs::read_to_string(&target.entry)
        .with_context(|| format!("reading {}", target.entry.display()))?;
    let name = target.entry.display().to_string();

    let result = format_source(name.clone(), original.clone());
    if result.diagnostics.has_errors() {
        eprint!(
            "{}",
            ingot_diagnostics::render_all(&result.sources, &result.diagnostics, color)
        );
        eprintln!("cannot format {name}: fix the syntax errors first");
        return Ok(EXIT_DIAGNOSTICS);
    }

    let Some(formatted) = result.formatted else {
        return Ok(EXIT_DIAGNOSTICS);
    };

    if formatted == original {
        if !args.check {
            println!("{name} is already formatted");
        }
        return Ok(EXIT_OK);
    }

    if args.check {
        eprintln!("{name} is not formatted");
        eprintln!("run `ingot fmt` to rewrite it");
        return Ok(EXIT_DIAGNOSTICS);
    }

    std::fs::write(&target.entry, &formatted)
        .with_context(|| format!("writing {}", target.entry.display()))?;
    println!("formatted {name}");
    Ok(EXIT_OK)
}

// --- build -----------------------------------------------------------------

fn run_build(args: &BuildArgs, color: RenderColor) -> Result<u8> {
    if (args.json || args.allow_unimplemented) && args.backend == BuildTarget::Ir {
        bail!(
            "--json and --allow-unimplemented belong to a portability report, and the `ir` \
             target has nothing to report: the IR is what every backend reads, so nothing can \
             fail to express it\n  \
             pass --target python"
        );
    }

    let mut target = resolve_target(args.target.path.as_deref())?;
    if let Some(out_dir) = &args.out_dir {
        target.out_dir = out_dir.clone();
    }

    // Machine-readable output must contain exactly one JSON document. Progress
    // remains useful for the ordinary terminal-oriented build.
    if !args.json {
        if let Some(manifest) = &target.manifest {
            println!(
                "building {} {}",
                manifest.project.name, manifest.project.version
            );
        }
    }

    let compilation = compile(&target)?;
    report(&compilation, color);
    if compilation.has_errors() {
        return Ok(EXIT_DIAGNOSTICS);
    }

    if compilation.agents.is_empty() {
        println!("nothing to build: the program declares no agent");
        return Ok(EXIT_OK);
    }

    std::fs::create_dir_all(&target.out_dir)
        .with_context(|| format!("creating {}", target.out_dir.display()))?;

    match args.backend {
        BuildTarget::Ir => build_ir(&compilation, &target),
        BuildTarget::Python => build_python(&compilation, &target, args),
    }
}

pub(crate) fn build_ir(compilation: &Compilation, target: &Target) -> Result<u8> {
    for agent in &compilation.agents {
        let path = target
            .out_dir
            .join(format!("{}.ir.json", short_name(agent)));
        std::fs::write(&path, agent.to_canonical_json())
            .with_context(|| format!("writing {}", path.display()))?;
        println!("{} -> {}", agent.agent, path.display());
    }
    Ok(EXIT_OK)
}

/// Compile for a target that is not the IR.
///
/// The report comes first and always, because the useful moment to learn that a
/// target cannot express something is before the artifact is deployed rather
/// than when the agent reaches the node.
fn build_python(compilation: &Compilation, target: &Target, args: &BuildArgs) -> Result<u8> {
    use ingot_backend_python as python;

    let report = python::analyse(python::TARGET, &compilation.agents);

    if args.json {
        // A deployment gate reads `.unimplemented`; the per-agent detail is
        // there for whoever has to fix it.
        let payload = serde_json::json!({
            "target": report.target,
            "buildable": report.buildable(),
            "unimplemented": report.unimplemented(),
            "agents": report.agents,
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
    } else {
        eprintln!("{}", report.render());
        eprintln!();
    }

    if !report.buildable() && !args.allow_unimplemented {
        let blocked: Vec<&str> = report
            .blocked()
            .iter()
            .map(|agent| agent.agent.as_str())
            .collect();
        bail!(
            "`{}` does not implement everything these agents use: {}\n  \
             fix the agent, or pass --allow-unimplemented to build one that will not do it",
            python::TARGET,
            blocked.join(", ")
        );
    }

    for agent in &compilation.agents {
        let source = match python::emit(agent) {
            Ok(source) => source,
            // Reaching here with --allow-unimplemented is the operator getting
            // what they asked for, and it still refuses rather than emitting a
            // program with a hole in it.
            Err(error) => bail!(
                "{} cannot be built for `{}`: {error}",
                agent.agent,
                python::TARGET
            ),
        };
        let path = target
            .out_dir
            .join(format!("{}.{}", short_name(agent), python::EXTENSION));
        std::fs::write(&path, &source).with_context(|| format!("writing {}", path.display()))?;
        if !args.json {
            println!("{} -> {}", agent.agent, path.display());
        }
    }
    Ok(EXIT_OK)
}

/// The last segment of a dotted agent name, which is what a file is named after.
fn short_name(agent: &ingot_ir::AgentIr) -> &str {
    agent.agent.rsplit('.').next().unwrap_or(&agent.agent)
}

// --- ir --------------------------------------------------------------------

fn run_ir(args: &IrArgs, color: RenderColor) -> Result<u8> {
    let target = resolve_target(args.target.path.as_deref())?;
    let compilation = compile(&target)?;
    report(&compilation, color);
    if compilation.has_errors() {
        return Ok(EXIT_DIAGNOSTICS);
    }

    let agent = match &args.agent {
        Some(name) => compilation.agent(name).with_context(|| {
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
        None => match compilation.agents.len() {
            0 => bail!("the program declares no agent"),
            1 => &compilation.agents[0],
            _ => {
                let available: Vec<&str> = compilation
                    .agents
                    .iter()
                    .map(|agent| agent.agent.as_str())
                    .collect();
                bail!(
                    "this file declares several agents; pass --agent <name>\navailable: {}",
                    available.join(", ")
                )
            }
        },
    };

    let mut stdout = std::io::stdout().lock();
    stdout
        .write_all(agent.to_canonical_json().as_bytes())
        .context("writing IR to standard output")?;
    Ok(EXIT_OK)
}

// --- run / test --------------------------------------------------------------

fn run_run(args: &RunArgs, color: RenderColor) -> Result<u8> {
    if args.sandbox_allow_unenforced && !args.sandbox && !args.contained {
        bail!(
            "--sandbox-allow-unenforced only means something with --sandbox or --contained; \
             without a boundary there is nothing to leave unenforced"
        );
    }
    if args.image.is_some() && !args.contained {
        bail!("--image only applies to --contained; a run on the host has no image");
    }

    let target = resolve_target(args.target.path.as_deref())?;
    let compilation = compile(&target)?;
    report(&compilation, color);
    if compilation.has_errors() {
        return Ok(EXIT_DIAGNOSTICS);
    }

    run::execute(
        &compilation,
        &RunConfig {
            inputs: args.inputs.clone(),
            provider: args.provider,
            cassette: args.cassette.clone(),
            record: args.record.clone(),
            model: args.model.clone(),
            effort: args.effort.clone(),
            agent: args.agent.clone(),
            out_dir: args.out_dir.clone(),
            events: args.events,
            yes: args.yes,
            max_steps: args.max_steps,
            mcp: target.mcp(),
            root: target.root.clone(),
            no_tools: args.no_tools,
            sandbox: args.sandbox,
            sandbox_allow_unenforced: args.sandbox_allow_unenforced,
            workspace: workspace(args.workspace.as_deref(), &target)?,
            models: target.model(),
            contained: args.contained,
            supervised: args.supervised,
            image: args
                .image
                .clone()
                .or_else(|| target.image())
                .or_else(|| args.contained.then(image::reference_image)),
        },
    )
}

/// The root policy paths are relative to: the flag, then the manifest, then the
/// project directory.
pub(crate) fn workspace(flag: Option<&Path>, target: &Target) -> Result<PathBuf> {
    let chosen = flag
        .map(Path::to_path_buf)
        .unwrap_or_else(|| target.workspace());
    chosen
        .canonicalize()
        .with_context(|| format!("resolving the workspace {}", chosen.display()))
}

fn run_tools(args: &ToolsArgs, color: RenderColor) -> Result<u8> {
    let target = resolve_target(args.target.path.as_deref())?;
    let compilation = compile(&target)?;
    report(&compilation, color);
    if compilation.has_errors() {
        return Ok(EXIT_DIAGNOSTICS);
    }

    tools::inspect(
        &compilation,
        &tools::ToolsConfig {
            mcp: target.mcp(),
            root: target.root.clone(),
        },
        args.json,
        args.propose,
    )
}

fn run_test(args: &TestArgs, color: RenderColor) -> Result<u8> {
    let target = resolve_target(args.target.path.as_deref())?;
    let compilation = compile(&target)?;
    report(&compilation, color);
    if compilation.has_errors() {
        return Ok(EXIT_DIAGNOSTICS);
    }

    // Cassette paths are relative to the project, not the working directory,
    // so `ingot test examples/research-agent` works from anywhere.
    let project_root = target
        .entry
        .parent()
        .unwrap_or(Path::new("."))
        .to_path_buf();
    let cassette_dir = if args.cassettes.is_absolute() {
        args.cassettes.clone()
    } else {
        project_root.join(&args.cassettes)
    };

    run::test(
        &compilation,
        &TestConfig {
            cassette_dir,
            filter: args.filter.clone(),
        },
    )
}

// --- doctor ---------------------------------------------------------------

fn run_doctor(args: &DoctorArgs, color: RenderColor) -> Result<u8> {
    let target = resolve_target(args.target.path.as_deref())?;
    let compilation = compile(&target)?;
    if !args.json {
        report(&compilation, color);
    }
    doctor::inspect(&target, &compilation, args.json)
}

// --- dev ------------------------------------------------------------------

fn run_dev(args: &DevArgs, color: RenderColor) -> Result<u8> {
    let initial = resolve_target(args.target.path.as_deref())?;
    dev::watch(
        args.target.path.as_deref(),
        initial,
        &dev::DevConfig {
            run: args.run,
            inputs: args.inputs.clone(),
            provider: args.provider,
            cassette: args.cassette.clone(),
            agent: args.agent.clone(),
            events: args.events,
            yes: args.yes,
            max_steps: args.max_steps,
            color,
        },
    )
}

// --- sandbox ---------------------------------------------------------------

fn run_sandbox(args: &SandboxArgs, color: RenderColor) -> Result<u8> {
    let target = resolve_target(args.target.path.as_deref())?;
    let compilation = compile(&target)?;
    report(&compilation, color);
    if compilation.has_errors() {
        return Ok(EXIT_DIAGNOSTICS);
    }

    sandbox::inspect(
        &compilation,
        &SandboxConfig {
            workspace: workspace(args.workspace.as_deref(), &target)?,
            mcp: target.mcp(),
            json: args.json,
        },
    )
}

// --- explain ---------------------------------------------------------------

fn run_explain(args: &ExplainArgs) -> Result<u8> {
    match codes::explain(&args.code) {
        Some(text) => {
            println!("{}\n", args.code.to_ascii_uppercase());
            println!("{text}");
            Ok(EXIT_OK)
        }
        None => {
            eprintln!("no long-form explanation for `{}`", args.code);
            eprintln!();
            eprintln!("explained codes: {}", codes::EXPLAINED_CODES.join(", "));
            Ok(EXIT_FAILURE)
        }
    }
}

// --- shared ----------------------------------------------------------------

pub(crate) fn compile(target: &Target) -> Result<Compilation> {
    compile_path(&target.entry).with_context(|| format!("compiling {}", target.entry.display()))
}

/// Print diagnostics, then a one-line summary.
///
/// Everything here goes to stderr, including the success line: stdout carries
/// machine-readable output such as `ingot ir`, and a status message must never
/// end up in a pipe someone is parsing.
pub(crate) fn report(compilation: &Compilation, color: RenderColor) {
    if !compilation.diagnostics.is_empty() {
        eprint!("{}", compilation.render_diagnostics(color));
    }

    let errors = compilation.error_count();
    let warnings = compilation.warning_count();
    match (errors, warnings) {
        (0, 0) => eprintln!("ok"),
        (0, warnings) => eprintln!("ok, {warnings} warning(s)"),
        (errors, 0) => eprintln!("failed: {errors} error(s)"),
        (errors, warnings) => eprintln!("failed: {errors} error(s), {warnings} warning(s)"),
    }
}

fn exit_code(compilation: &Compilation) -> u8 {
    if compilation.has_errors() {
        EXIT_DIAGNOSTICS
    } else {
        EXIT_OK
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_name_becomes_a_package() {
        assert_eq!(
            package_name("research-agent").as_deref(),
            Some("research_agent")
        );
        assert_eq!(package_name("My Agent").as_deref(), Some("my_agent"));
    }

    #[test]
    fn a_reserved_word_yields_no_package() {
        // `ingot init agent` used to generate `package agent`, which is a
        // syntax error, so the generated project would not compile.
        for reserved in ["agent", "tool", "flow", "type", "policy"] {
            assert_eq!(package_name(reserved), None, "`{reserved}` is reserved");
        }
    }

    #[test]
    fn a_name_that_cannot_start_an_identifier_yields_no_package() {
        assert_eq!(package_name("2fa"), None);
        assert_eq!(package_name("---"), None);
        assert_eq!(package_name(""), None);
    }

    #[test]
    fn the_generated_source_omits_an_unusable_package_line() {
        let source = starter_source("agent", StarterTemplate::Brief);
        assert!(!source.contains("package"), "{source}");
        assert!(
            source.starts_with(
                "language 0.1
"
            ),
            "{source}"
        );
    }

    #[test]
    fn the_generated_source_keeps_a_usable_package_line() {
        let source = starter_source("research-agent", StarterTemplate::Brief);
        assert!(source.contains("package research_agent"), "{source}");
    }
}
