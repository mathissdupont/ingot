//! `ingot run` against real MCP tools.
//!
//! These are the tests that join the two halves: a compiled artifact executed
//! by the interpreter, calling tools served by `ingot-mcp-fs` in a separate
//! process over stdio. The model side is the usual stub HTTP server, so a run
//! here needs no API key and no network, but the tool side is entirely real —
//! the files these tests assert on are written by the server, not by a mock.

mod support;

use std::path::{Path, PathBuf};
use std::sync::Once;

use support::{
    code, repo_root, run, stderr, stdout, stub_provider, text_reply, TempDir, EXIT_DIAGNOSTICS,
    EXIT_OK,
};

/// Where the reference server binary is, building it if this test binary was
/// invoked in a way that did not.
fn fs_server() -> PathBuf {
    static BUILD: Once = Once::new();

    let mut dir = std::env::current_exe().expect("the test binary has a path");
    dir.pop();
    if dir.ends_with("deps") {
        dir.pop();
    }
    let path = dir.join(format!("ingot-mcp-fs{}", std::env::consts::EXE_SUFFIX));

    BUILD.call_once(|| {
        if path.is_file() {
            return;
        }
        // `cargo test -p ingot-cli` builds the ingot-mcp library but not its
        // binaries, so build it rather than failing with a puzzle.
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
        let mut command = std::process::Command::new(cargo);
        command.current_dir(repo_root()).args([
            "build",
            "-p",
            "ingot-mcp",
            "--bin",
            "ingot-mcp-fs",
        ]);
        if dir.ends_with("release") {
            command.arg("--release");
        }
        let status = command.status().expect("cargo must be runnable");
        assert!(status.success(), "building ingot-mcp-fs failed");
    });

    assert!(
        path.is_file(),
        "expected the reference MCP server at {}",
        path.display()
    );
    path
}

const DIGEST_SOURCE: &str = include_str!("../../../examples/repo-digest/main.ing");

/// A throwaway project with a workspace for the server to serve.
struct Project {
    dir: TempDir,
}

impl Project {
    fn new(tag: &str, source: &str, configure_tools: bool) -> Project {
        let dir = TempDir::new(tag);
        let root = dir.path();
        let data = root.join("data");
        std::fs::create_dir_all(&data).expect("creating the sample data");
        std::fs::write(data.join("README.md"), "# Sample\n\nA sample workspace.\n").unwrap();
        std::fs::write(data.join("notes.md"), "notes\n").unwrap();
        // Something outside the sandbox to try, and fail, to reach.
        std::fs::write(root.join("secret.txt"), "do not read me\n").unwrap();

        std::fs::write(root.join("main.ing"), source).expect("writing the source");

        let mut manifest = String::from(
            "[project]\nname = \"digest\"\n\n[build]\nentry = \"main.ing\"\nout-dir = \"target/ingot\"\n",
        );
        if configure_tools {
            manifest.push_str(&format!(
                "\n[mcp]\ntimeout-seconds = 10\n\n[[mcp.server]]\nname = \"workspace\"\ncommand = {}\nargs = [\"--root\", \"data\", \"--allow-write\"]\n",
                toml_string(&fs_server().display().to_string())
            ));
        }
        std::fs::write(root.join("ingot.toml"), manifest).expect("writing the manifest");

        Project { dir }
    }

    fn path(&self) -> String {
        self.dir.path().display().to_string()
    }

    fn workspace(&self) -> &Path {
        self.dir.path()
    }
}

fn toml_string(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

fn digest_args(project: &Project) -> Vec<String> {
    vec![
        "run".to_string(),
        project.path(),
        "--input".to_string(),
        "directory=.".to_string(),
        "--input".to_string(),
        "out=out/digest.md".to_string(),
        "--events".to_string(),
        "quiet".to_string(),
    ]
}

fn as_args(owned: &[String]) -> Vec<&str> {
    owned.iter().map(String::as_str).collect()
}

#[test]
fn a_run_reaches_real_tools_over_stdio_and_the_bytes_land_on_disk() {
    let project = Project::new("digest", DIGEST_SOURCE, true);
    let stub = stub_provider(vec![text_reply("# Digest\n\nTwo markdown files.\n")]);

    let args = digest_args(&project);
    let output = run(&as_args(&args), Some(&stub.url));

    assert_eq!(code(&output), EXIT_OK, "{}", stderr(&output));
    assert!(stdout(&output).contains("# Digest"), "{}", stdout(&output));

    let written = project.workspace().join("data/out/digest.md");
    assert!(written.is_file(), "expected {}", written.display());
    assert_eq!(
        std::fs::read_to_string(&written).unwrap(),
        "# Digest\n\nTwo markdown files.\n"
    );
}

#[test]
fn the_resolved_routing_is_reported_before_the_run() {
    let project = Project::new("routing", DIGEST_SOURCE, true);
    let stub = stub_provider(vec![text_reply("# Digest")]);

    let args = digest_args(&project);
    let output = run(&as_args(&args), Some(&stub.url));

    assert_eq!(code(&output), EXIT_OK, "{}", stderr(&output));
    let log = stderr(&output);
    assert!(
        log.contains("tool fs.read_file <- workspace:fs.read_file"),
        "{log}"
    );
    assert!(
        log.contains("tool fs.write_file <- workspace:fs.write_file"),
        "{log}"
    );
}

#[test]
fn a_recorded_run_replays_with_the_tools_still_live() {
    let project = Project::new("replay", DIGEST_SOURCE, true);
    let cassette = project.workspace().join("digest.json");
    let stub = stub_provider(vec![text_reply("# Recorded digest\n")]);

    let mut record = digest_args(&project);
    record.push("--record".to_string());
    record.push(cassette.display().to_string());
    let recorded = run(&as_args(&record), Some(&stub.url));
    assert_eq!(code(&recorded), EXIT_OK, "{}", stderr(&recorded));
    assert!(cassette.is_file());

    // Remove the whole output directory, not just the file. The agent lists the
    // workspace and puts the listing in the prompt, so leaving an empty `out/`
    // behind would change the prompt and the cassette would — correctly —
    // refuse to replay it.
    let written = project.workspace().join("data/out/digest.md");
    assert!(written.is_file(), "the first run must have written it");
    std::fs::remove_dir_all(project.workspace().join("data/out")).unwrap();

    let mut replay = digest_args(&project);
    replay.push("--provider".to_string());
    replay.push("replay".to_string());
    replay.push("--cassette".to_string());
    replay.push(cassette.display().to_string());
    let replayed = run(&as_args(&replay), None);

    assert_eq!(code(&replayed), EXIT_OK, "{}", stderr(&replayed));
    assert_eq!(stdout(&replayed).trim(), "# Recorded digest");
    assert_eq!(
        std::fs::read_to_string(&written).unwrap(),
        "# Recorded digest\n"
    );
}

#[test]
fn no_tools_makes_the_agent_stop_at_the_call_naming_the_tool() {
    let project = Project::new("no-tools", DIGEST_SOURCE, true);
    let stub = stub_provider(vec![text_reply("# never reached")]);

    let mut args = digest_args(&project);
    args.push("--no-tools".to_string());
    let output = run(&as_args(&args), Some(&stub.url));

    assert_eq!(code(&output), EXIT_DIAGNOSTICS);
    let message = stderr(&output);
    assert!(message.contains("fs.list_dir"), "{message}");
    assert!(message.contains("no host provides"), "{message}");
}

#[test]
fn a_project_with_tools_and_no_server_warns_before_it_fails() {
    let project = Project::new("unconfigured", DIGEST_SOURCE, false);
    let stub = stub_provider(vec![text_reply("# never reached")]);

    let args = digest_args(&project);
    let output = run(&as_args(&args), Some(&stub.url));

    assert_eq!(code(&output), EXIT_DIAGNOSTICS);
    let message = stderr(&output);
    assert!(message.contains("configures no MCP server"), "{message}");
    assert!(message.contains("ingot tools"), "{message}");
}

/// The same agent, but pointed at a path that climbs out of the sandbox.
const ESCAPE_SOURCE: &str = r#"language 0.1

tool fs.read_file(path: string) -> text !filesystem_read

agent Escape() -> leak<markdown> {
  model requires {
    structured_output
  }

  tools {
    mcp fs.read_file
  }

  budget {
    steps <= 4
  }

  policy {
    filesystem_read allow ["."]
    network deny
  }

  flow {
    stolen = call fs.read_file("../secret.txt")
    emit leak = ask<markdown>("Report: ${stolen}")
  }
}
"#;

#[test]
fn a_path_out_of_the_server_root_is_refused_even_though_the_policy_allows_reading() {
    // The artifact's policy permits `filesystem_read`, so the compiler and the
    // interpreter both let the call through. The sandbox is the server's, and
    // it is the one that has to hold.
    let project = Project::new("escape", ESCAPE_SOURCE, true);
    let stub = stub_provider(vec![text_reply("# never reached")]);

    let output = run(
        &["run", &project.path(), "--events", "quiet"],
        Some(&stub.url),
    );

    assert_eq!(code(&output), EXIT_DIAGNOSTICS, "{}", stdout(&output));
    let message = stderr(&output);
    assert!(message.contains("refused"), "{message}");
    assert!(
        !message.contains("do not read me"),
        "the file's contents must never appear: {message}"
    );
}

#[test]
fn ingot_tools_lists_the_servers_and_the_routing() {
    let project = Project::new("tools-ok", DIGEST_SOURCE, true);
    let output = run(&["tools", &project.path()], None);

    assert_eq!(code(&output), EXIT_OK, "{}", stderr(&output));
    let listing = stdout(&output);
    assert!(listing.contains("ingot-mcp-fs"), "{listing}");
    assert!(listing.contains("fs.write_file"), "{listing}");
    assert!(listing.contains("-> workspace:fs.list_dir"), "{listing}");
}

#[test]
fn ingot_tools_exits_non_zero_when_a_declared_tool_has_no_server() {
    let project = Project::new("tools-missing", DIGEST_SOURCE, false);
    let output = run(&["tools", &project.path()], None);

    assert_eq!(code(&output), EXIT_DIAGNOSTICS);
    let listing = stdout(&output);
    assert!(listing.contains("no MCP server is configured"), "{listing}");
    assert!(listing.contains("fs.read_file"), "{listing}");
    assert!(listing.contains("[[mcp.server]]"), "{listing}");
}

// --- ingot sandbox ----------------------------------------------------------

#[test]
fn sandbox_derives_the_boundary_from_the_policy_and_names_its_source() {
    let project = Project::new("sandbox", DIGEST_SOURCE, true);
    let output = run(&["sandbox", &project.path()], None);

    assert_eq!(code(&output), EXIT_OK, "{}", stderr(&output));
    let plan = stdout(&output);
    assert!(plan.contains("/workspace/data      ro"), "{plan}");
    assert!(plan.contains("/workspace/data/out  rw"), "{plan}");
    assert!(plan.contains("filesystem_read allow [\"data\"]"), "{plan}");
    assert!(plan.contains("network  none"), "{plan}");
    assert!(
        plan.contains("every policy rule above is enforced"),
        "{plan}"
    );
}

#[test]
fn sandbox_refuses_a_policy_path_that_is_not_there() {
    // Mounting an empty directory would make a missing checkout look like an
    // empty one, so the plan fails rather than producing a plausible box.
    let source = DIGEST_SOURCE.replace(
        "filesystem_read allow [\"data\"]",
        "filesystem_read allow [\"absent\"]",
    );
    let project = Project::new("sandbox-missing", &source, true);
    let output = run(&["sandbox", &project.path()], None);

    assert_eq!(code(&output), EXIT_DIAGNOSTICS);
    let message = stderr(&output);
    assert!(message.contains("absent"), "{message}");
    assert!(message.contains("does not exist"), "{message}");
    assert!(message.contains("--workspace"), "{message}");
}

#[test]
fn the_workspace_can_be_moved_from_the_command_line() {
    // The artifact says `data`; the operator says where `data` lives. Pointed
    // at a directory that has no `data`, the same artifact cannot be contained.
    let project = Project::new("sandbox-workspace", DIGEST_SOURCE, true);
    let elsewhere = TempDir::new("sandbox-elsewhere");

    let output = run(
        &[
            "sandbox",
            &project.path(),
            "--workspace",
            &elsewhere.path().display().to_string(),
        ],
        None,
    );
    assert_eq!(code(&output), EXIT_DIAGNOSTICS, "{}", stdout(&output));
    assert!(
        stderr(&output).contains("does not exist"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn sandbox_plans_are_machine_readable() {
    let project = Project::new("sandbox-json", DIGEST_SOURCE, true);
    let output = run(&["sandbox", &project.path(), "--json"], None);

    assert_eq!(code(&output), EXIT_OK, "{}", stderr(&output));
    let plans: serde_json::Value =
        serde_json::from_str(&stdout(&output)).expect("--json must emit JSON on stdout");
    let plan = &plans[0];
    assert_eq!(plan["network"]["mode"], "none");
    assert_eq!(plan["workdir"], "/workspace");
    assert_eq!(plan["mounts"][0]["guest"], "/workspace/data");
    assert_eq!(plan["mounts"][0]["writable"], false);
    assert_eq!(plan["unenforceable"].as_array().unwrap().len(), 0);
}

#[test]
fn sandbox_says_so_when_nothing_would_be_contained() {
    let project = Project::new("sandbox-untooled", DIGEST_SOURCE, false);
    let output = run(&["sandbox", &project.path()], None);

    assert_eq!(code(&output), EXIT_OK);
    assert!(
        stderr(&output).contains("nothing would be contained"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn a_program_without_tools_needs_no_servers() {
    let path = repo_root()
        .join("examples/document-summarizer")
        .display()
        .to_string();
    let output = run(&["tools", &path], None);

    assert_eq!(code(&output), EXIT_OK, "{}", stderr(&output));
    assert!(
        stdout(&output).contains("declares no tools"),
        "{}",
        stdout(&output)
    );
}
