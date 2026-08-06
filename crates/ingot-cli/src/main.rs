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

use manifest::{resolve_target, Manifest, Target, MANIFEST_NAME};

const EXIT_OK: u8 = 0;
const EXIT_DIAGNOSTICS: u8 = 1;
const EXIT_FAILURE: u8 = 2;

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

fn starter_source(name: &str) -> String {
    let package = name.replace(['-', ' '], "_").to_lowercase();
    format!(
        r#"language 0.1
package {package}

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
