//! End-to-end tests for the `ingot` binary.
//!
//! Exit codes are a public contract that CI depends on, so they are asserted
//! here rather than inferred from behaviour.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const EXIT_OK: i32 = 0;
const EXIT_DIAGNOSTICS: i32 = 1;
const EXIT_FAILURE: i32 = 2;

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_ingot")
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("the crate must live two levels below the repository root")
        .to_path_buf()
}

/// A scratch directory that cleans itself up.
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> TempDir {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock is before the epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("ingot-cli-{tag}-{unique}"));
        std::fs::create_dir_all(&path).expect("creating the scratch directory");
        TempDir(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn run(args: &[&str]) -> Output {
    Command::new(binary())
        .args(args)
        .arg("--color")
        .arg("never")
        .output()
        .expect("the ingot binary must be runnable")
}

/// Run commands exactly as the generated README presents them: from the
/// project directory and with no provider credential inherited from the shell.
fn run_in(dir: &Path, args: &[&str]) -> Output {
    let mut command = Command::new(binary());
    command
        .current_dir(dir)
        .args(args)
        .arg("--color")
        .arg("never");
    for name in [
        "ANTHROPIC_API_KEY",
        "INGOT_ANTHROPIC_BASE_URL",
        "OPENAI_API_KEY",
        "INGOT_OPENAI_BASE_URL",
    ] {
        command.env_remove(name);
    }
    command.output().expect("the ingot binary must be runnable")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn code(output: &Output) -> i32 {
    output
        .status
        .code()
        .expect("the process must exit normally")
}

#[test]
fn check_succeeds_on_every_reference_example() {
    for example in ["document-summarizer", "research-agent", "code-review-team"] {
        let path = repo_root().join("examples").join(example);
        let output = run(&["check", &path.display().to_string()]);
        assert_eq!(
            code(&output),
            EXIT_OK,
            "`ingot check {example}` failed:\n{}",
            stderr(&output)
        );
    }
}

#[test]
fn check_reports_diagnostics_with_exit_code_one() {
    let dir = TempDir::new("check-fail");
    let source = dir.path().join("broken.ing");
    std::fs::write(
        &source,
        "language 0.1\nagent A(topic: string) -> report<markdown> {\n  flow { }\n}\n",
    )
    .expect("writing the source");

    let output = run(&["check", &source.display().to_string()]);
    assert_eq!(code(&output), EXIT_DIAGNOSTICS);
    assert!(stderr(&output).contains("ING6001"), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("failed: 1 error"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn a_missing_path_is_a_command_failure_not_a_diagnostic() {
    let output = run(&["check", "definitely/not/here.ing"]);
    assert_eq!(code(&output), EXIT_FAILURE);
    assert!(
        stderr(&output).contains("does not exist"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn doctor_names_every_missing_run_prerequisite_without_revealing_a_secret() {
    let dir = TempDir::new("doctor-missing");
    std::fs::write(
        dir.path().join("main.ing"),
        r#"language 0.1

tool web.search(query: string) -> text !network

agent Research(topic: string) -> brief<markdown> {
  model exact "ghost/model"

  tools {
    mcp web.search
  }

  budget {
    steps <= 4
    tokens <= 20000
  }

  policy {
    network allow ["example.com"]
  }

  flow {
    source = call web.search(topic)
    emit brief = ask<markdown>("Summarise ${source}.")
  }
}
"#,
    )
    .expect("writing doctor source");
    std::fs::write(
        dir.path().join("ingot.toml"),
        r#"[project]
name = "doctor-missing"

[[model.provider]]
name = "secure"
kind = "openai"
base-url = "https://example.invalid/v1/chat/completions"
api-key-env = "DOCTOR_SECRET_TOKEN"

[[mcp.server]]
name = "first"
command = "ingot-doctor-no-such-server"
pass-env = ["DOCTOR_SECRET_TOKEN"]

[mcp.server.tools]
"web.search" = "first.search"

[[mcp.server]]
name = "second"
command = "ingot-doctor-also-missing"

[mcp.server.tools]
"web.search" = "second.search"
"#,
    )
    .expect("writing doctor manifest");

    let secret = "never-print-this-doctor-secret";
    let output = Command::new(binary())
        .args(["doctor", &dir.path().display().to_string(), "--json"])
        // Makes runtime and fake MCP command detection deterministic without
        // affecting the already-resolved path to the `ingot` test binary.
        .env("PATH", "")
        .env("DOCTOR_SECRET_TOKEN", secret)
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("OPENAI_API_KEY")
        .output()
        .expect("doctor must be runnable");

    assert_eq!(code(&output), EXIT_DIAGNOSTICS, "{}", stderr(&output));
    let printed = format!("{}{}", stdout(&output), stderr(&output));
    assert!(!printed.contains(secret), "secret value leaked:\n{printed}");
    assert!(
        printed.contains("DOCTOR_SECRET_TOKEN"),
        "the variable name is actionable:\n{printed}"
    );

    let report: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("doctor stdout must be one JSON document");
    assert_eq!(report["schemaVersion"], 1);
    assert_eq!(report["ready"], false);
    let checks = report["checks"].as_array().expect("checks array");
    for (id, status) in [
        ("source.compile", "pass"),
        ("provider.route.ghost", "fail"),
        ("tools.server.first.command", "fail"),
        ("tools.server.second.command", "fail"),
        ("tools.route.web.search", "fail"),
        ("container.runtime", "fail"),
        ("container.configured-image", "fail"),
    ] {
        assert!(
            checks
                .iter()
                .any(|check| check["id"] == id && check["status"] == status),
            "missing {status} check `{id}`:\n{}",
            stdout(&output)
        );
    }
}

#[test]
fn init_creates_a_project_that_checks_and_builds() {
    let dir = TempDir::new("init");
    let project = dir.path().join("my-agent");

    let output = run(&["init", &project.display().to_string()]);
    assert_eq!(code(&output), EXIT_OK, "{}", stderr(&output));
    assert!(project.join("ingot.toml").is_file());
    assert!(project.join("main.ing").is_file());
    assert!(project.join("README.md").is_file());
    assert!(project.join(".gitignore").is_file());
    assert!(project.join("tests/cassettes/example.json").is_file());

    let output = run(&["check", &project.display().to_string()]);
    assert_eq!(
        code(&output),
        EXIT_OK,
        "a freshly generated project must check cleanly:\n{}",
        stderr(&output)
    );

    let output = run(&["build", &project.display().to_string()]);
    assert_eq!(code(&output), EXIT_OK, "{}", stderr(&output));
    assert!(
        project.join("target/ingot/Brief.ir.json").is_file(),
        "build must write the IR to the manifest's out-dir"
    );
}

#[test]
fn a_template_project_checks_builds_and_replays_without_a_key() {
    for (template, agent, extra_file, replay_command, replay_args) in [
        (
            "brief",
            "Brief",
            None,
            "ingot run --provider replay --cassette tests/cassettes/example.json --input topic=\"compiler design\"",
            &["--input", "topic=compiler design"][..],
        ),
        (
            "document-workflow",
            "DocumentWorkflow",
            Some("examples/document.txt"),
            "ingot run --provider replay --cassette tests/cassettes/example.json --input document=@examples/document.txt --input audience=\"project leads\"",
            &[
                "--input",
                "document=@examples/document.txt",
                "--input",
                "audience=project leads",
            ][..],
        ),
    ] {
        let dir = TempDir::new(&format!("init-{template}"));
        let project = dir.path().join(format!("{template}-agent"));
        let output = run(&[
            "init",
            &project.display().to_string(),
            "--template",
            template,
        ]);
        assert_eq!(code(&output), EXIT_OK, "{}", stderr(&output));
        assert!(project.join("tests/cassettes/example.json").is_file());
        let readme = std::fs::read_to_string(project.join("README.md")).expect("starter README");
        for command in ["ingot check", "ingot build", "ingot test", replay_command] {
            assert!(
                readme.lines().any(|line| line == command),
                "template `{template}` does not print `{command}` as an executable README line:\n{readme}"
            );
        }
        assert!(
            readme.lines().any(|line| line == "ingot dev"),
            "template `{template}` does not expose the integrated edit loop:\n{readme}"
        );
        assert!(
            readme.contains("ingot dev --run --provider replay"),
            "template `{template}` does not expose the opt-in offline run:\n{readme}"
        );
        if let Some(path) = extra_file {
            assert!(project.join(path).is_file(), "{template}: missing {path}");
        }

        // These are the first three commands printed in the generated README.
        for args in [&["check"][..], &["build"][..], &["test"][..]] {
            let output = run_in(&project, args);
            assert_eq!(
                code(&output),
                EXIT_OK,
                "template `{template}`, command `{}`:\n{}",
                args.join(" "),
                stderr(&output)
            );
        }

        assert!(
            project
                .join(format!("target/ingot/{agent}.ir.json"))
                .is_file(),
            "template `{template}` did not build its declared agent"
        );
        let mut direct = vec![
            "run",
            "--provider",
            "replay",
            "--cassette",
            "tests/cassettes/example.json",
        ];
        direct.extend_from_slice(replay_args);
        let replay = run_in(&project, &direct);
        assert_eq!(
            code(&replay),
            EXIT_OK,
            "the direct replay printed in `{template}` README must work:\n{}",
            stderr(&replay)
        );
        assert!(
            !stdout(&replay).trim().is_empty(),
            "{template}: no artifact"
        );
    }
}

#[test]
fn init_refuses_to_overwrite_an_existing_project() {
    let dir = TempDir::new("init-twice");
    let project = dir.path().join("agent");
    assert_eq!(
        code(&run(&["init", &project.display().to_string()])),
        EXIT_OK
    );

    let output = run(&["init", &project.display().to_string()]);
    assert_eq!(code(&output), EXIT_FAILURE);
    assert!(
        stderr(&output).contains("already contains"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn build_writes_canonical_ir_for_each_agent() {
    let dir = TempDir::new("build");
    let source = repo_root().join("examples/code-review-team/main.ing");

    let output = run(&[
        "build",
        &source.display().to_string(),
        "--out-dir",
        &dir.path().display().to_string(),
    ]);
    assert_eq!(code(&output), EXIT_OK, "{}", stderr(&output));

    for agent in ["SecurityReviewer", "CodeReviewTeam"] {
        let path = dir.path().join(format!("{agent}.ir.json"));
        assert!(path.is_file(), "expected {}", path.display());
        let text = std::fs::read_to_string(&path).expect("reading the IR");
        let ir = ingot_ir::AgentIr::from_json(&text).expect("IR must parse");
        assert_eq!(ir.to_canonical_json(), text, "written IR must be canonical");
    }
}

#[test]
fn ir_prints_to_stdout_and_requires_a_name_when_ambiguous() {
    let source = repo_root()
        .join("examples/code-review-team/main.ing")
        .display()
        .to_string();

    let output = run(&["ir", &source]);
    assert_eq!(code(&output), EXIT_FAILURE);
    assert!(stderr(&output).contains("--agent"), "{}", stderr(&output));

    let output = run(&["ir", &source, "--agent", "CodeReviewTeam"]);
    assert_eq!(code(&output), EXIT_OK, "{}", stderr(&output));
    let printed = stdout(&output);
    assert!(printed.starts_with('{'));
    let ir = ingot_ir::AgentIr::from_json(&printed).expect("printed IR must parse");
    assert_eq!(ir.agent, "heptapus.examples.review.CodeReviewTeam");
}

#[test]
fn fmt_check_accepts_the_reference_examples() {
    for example in ["document-summarizer", "research-agent", "code-review-team"] {
        let path = repo_root().join("examples").join(example);
        let output = run(&["fmt", "--check", &path.display().to_string()]);
        assert_eq!(
            code(&output),
            EXIT_OK,
            "example `{example}` is not canonically formatted:\n{}",
            stderr(&output)
        );
    }
}

#[test]
fn fmt_rewrites_a_badly_formatted_file() {
    let dir = TempDir::new("fmt");
    let source = dir.path().join("main.ing");
    let ugly = "language 0.1\nagent A(topic:string)->out<markdown>{flow{emit out=ask<markdown>(\"hi ${topic}\")}}\n";
    std::fs::write(&source, ugly).expect("writing the source");

    let output = run(&["fmt", "--check", &source.display().to_string()]);
    assert_eq!(code(&output), EXIT_DIAGNOSTICS);
    assert!(
        stderr(&output).contains("not formatted"),
        "{}",
        stderr(&output)
    );

    let output = run(&["fmt", &source.display().to_string()]);
    assert_eq!(code(&output), EXIT_OK, "{}", stderr(&output));

    let formatted = std::fs::read_to_string(&source).expect("reading the formatted source");
    assert_ne!(formatted, ugly);
    assert!(formatted.contains("agent A(topic: string) -> out<markdown> {"));

    // Formatting is a fixed point.
    let output = run(&["fmt", "--check", &source.display().to_string()]);
    assert_eq!(code(&output), EXIT_OK, "{}", stderr(&output));
}

#[test]
fn explain_prints_a_known_code_and_rejects_an_unknown_one() {
    let output = run(&["explain", "ING4001"]);
    assert_eq!(code(&output), EXIT_OK);
    assert!(
        stdout(&output).contains("default-deny"),
        "{}",
        stdout(&output)
    );

    let output = run(&["explain", "ing4001"]);
    assert_eq!(code(&output), EXIT_OK, "codes are case-insensitive");

    let output = run(&["explain", "ING9999"]);
    assert_eq!(code(&output), EXIT_FAILURE);
    assert!(
        stderr(&output).contains("explained codes"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn a_failing_build_writes_no_artifact() {
    let dir = TempDir::new("no-artifact");
    let source = dir.path().join("broken.ing");
    std::fs::write(
        &source,
        "language 0.1\nagent A(topic: string) -> report<markdown> {\n  flow { }\n}\n",
    )
    .expect("writing the source");
    let out_dir = dir.path().join("out");

    let output = run(&[
        "build",
        &source.display().to_string(),
        "--out-dir",
        &out_dir.display().to_string(),
    ]);
    assert_eq!(code(&output), EXIT_DIAGNOSTICS);
    assert!(!out_dir.exists(), "a failed build must not create output");
}
