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
    /// Show which MCP server provides each tool the program declares.
    Tools(PathArgs),
    /// Show the boundary each tool server would run inside, derived from the
    /// agent's own policy.
    Sandbox(SandboxArgs),
    /// Explain a diagnostic code in full.
    Explain(ExplainArgs),
}

#[derive(Args, Debug)]
struct InitArgs {
    /// Directory to create. Use `.` to initialise the current directory.
    name: PathBuf,
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
    #[arg(long, value_enum, default_value_t = ProviderChoice::Anthropic)]
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
    #[arg(long)]
    sandbox: bool,

    /// Proceed even where the boundary cannot honour a rule the policy states.
    #[arg(long, requires = "sandbox")]
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
        Command::Tools(args) => run_tools(args, color),
        Command::Sandbox(args) => run_sandbox(args, color),
        Command::Explain(args) => run_explain(args),
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

    let manifest = Manifest::new(&name);
    write_new(&dir.join(MANIFEST_NAME), &manifest.to_toml())?;
    write_new(&dir.join("main.ing"), &starter_source(&name))?;
    write_new(&dir.join(".gitignore"), "/target\n")?;
    write_new(&dir.join("README.md"), &starter_readme(&name))?;

    println!("Created agent project `{name}` in {}", dir.display());
    println!();
    println!("Next steps:");
    if dir != Path::new(".") {
        println!("  cd {}", dir.display());
    }
    println!("  ingot check");
    println!("  ingot build");
    Ok(EXIT_OK)
}

fn write_new(path: &Path, contents: &str) -> Result<()> {
    if path.exists() {
        bail!("{} already exists", path.display());
    }
    std::fs::write(path, contents).with_context(|| format!("writing {}", path.display()))
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

fn starter_source(name: &str) -> String {
    let package = match package_name(name) {
        Some(package) => format!(
            "package {package}
"
        ),
        None => String::new(),
    };
    format!(
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
    )
}

fn starter_readme(name: &str) -> String {
    format!(
        "# {name}\n\n\
         An agent written in Ingot.\n\n\
         ## Commands\n\n\
         ```\n\
         ingot check     # types, effects, policy and budgets\n\
         ingot fmt       # canonical formatting\n\
         ingot build     # compile to Agent IR\n\
         ingot ir        # print the IR\n\
         ```\n\n\
         `ingot build` writes `target/ingot/<agent>.ir.json`. The IR is the\n\
         target-neutral form a backend compiles into a runtime configuration.\n"
    )
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
    let mut target = resolve_target(args.target.path.as_deref())?;
    if let Some(out_dir) = &args.out_dir {
        target.out_dir = out_dir.clone();
    }

    if let Some(manifest) = &target.manifest {
        println!(
            "building {} {}",
            manifest.project.name, manifest.project.version
        );
    }

    let compilation = compile(&target)?;
    report(&compilation, color);
    if compilation.has_errors() {
        return Ok(EXIT_DIAGNOSTICS);
    }

    std::fs::create_dir_all(&target.out_dir)
        .with_context(|| format!("creating {}", target.out_dir.display()))?;

    for agent in &compilation.agents {
        let short_name = agent.agent.rsplit('.').next().unwrap_or(&agent.agent);
        let path = target.out_dir.join(format!("{short_name}.ir.json"));
        std::fs::write(&path, agent.to_canonical_json())
            .with_context(|| format!("writing {}", path.display()))?;
        println!("{} -> {}", agent.agent, path.display());
    }

    if compilation.agents.is_empty() {
        println!("nothing to build: the program declares no agent");
    }
    Ok(EXIT_OK)
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
        },
    )
}

/// The root policy paths are relative to: the flag, then the manifest, then the
/// project directory.
fn workspace(flag: Option<&Path>, target: &Target) -> Result<PathBuf> {
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

fn compile(target: &Target) -> Result<Compilation> {
    compile_path(&target.entry).with_context(|| format!("compiling {}", target.entry.display()))
}

/// Print diagnostics, then a one-line summary.
///
/// Everything here goes to stderr, including the success line: stdout carries
/// machine-readable output such as `ingot ir`, and a status message must never
/// end up in a pipe someone is parsing.
fn report(compilation: &Compilation, color: RenderColor) {
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
        let source = starter_source("agent");
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
        let source = starter_source("research-agent");
        assert!(source.contains("package research_agent"), "{source}");
    }
}
