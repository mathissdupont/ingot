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
use ingot_compiler::{compile_path, format_source, Compilation};
use ingot_diagnostics::{codes, ColorChoice as RenderColor};

mod contained;
mod dev;
mod doctor;
mod manifest;
mod run;
mod sandbox;

use manifest::{resolve_target, Manifest, Target, MANIFEST_NAME};
use run::{EventFormat, ProviderChoice, RunConfig, TestConfig, ToolsConfig};
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
    /// Watch, check and build each source revision; optionally run good ones.
    Dev(DevArgs),
    /// Show which MCP server provides each tool the program declares.
    Tools(PathArgs),
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
    /// Needs a container runtime and `[run] image`.
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
        Command::Check(args) => run_check(args, color),
        Command::Fmt(args) => run_fmt(args, color),
        Command::Build(args) => run_build(args, color),
        Command::Ir(args) => run_ir(args, color),
        Command::Run(args) => run_run(args, color),
        Command::Test(args) => run_test(args, color),
        Command::Doctor(args) => run_doctor(args, color),
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
    let name = dir
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .filter(|name| name != ".")
        .or_else(|| {
            std::env::current_dir()
                .ok()
                .and_then(|dir| dir.file_name().map(|n| n.to_string_lossy().to_string()))
        })
        .unwrap_or_else(|| "agent".to_string());

    if dir.join(MANIFEST_NAME).exists() {
        bail!("{} already contains an {MANIFEST_NAME}", dir.display());
    }
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;

    let mut manifest = Manifest::new(&name);
    manifest.project.description = Some(args.template.description().to_string());
    write_new(&dir.join(MANIFEST_NAME), &manifest.to_toml())?;
    write_new(&dir.join("main.ing"), &starter_source(&name, args.template))?;
    write_new(&dir.join(".gitignore"), "/target\n")?;
    write_new(
        &dir.join("README.md"),
        &starter_readme(&name, args.template),
    )?;
    write_new(
        &dir.join("tests/cassettes/example.json"),
        &starter_cassette(&name, args.template).to_canonical_json(),
    )?;
    if let Some((path, contents)) = args.template.example_file() {
        write_new(&dir.join(path), contents)?;
    }

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
            image: args.image.clone().or_else(|| target.image()),
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

fn run_tools(args: &PathArgs, color: RenderColor) -> Result<u8> {
    let target = resolve_target(args.path.as_deref())?;
    let compilation = compile(&target)?;
    report(&compilation, color);
    if compilation.has_errors() {
        return Ok(EXIT_DIAGNOSTICS);
    }

    run::tools(
        &compilation,
        &ToolsConfig {
            mcp: target.mcp(),
            root: target.root.clone(),
        },
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
