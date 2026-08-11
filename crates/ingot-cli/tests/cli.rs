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
        ("container.configured-image", "pass"),
        ("container.reference-image", "warn"),
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
fn doctor_reports_a_stale_reference_image_override() {
    let scratch = TempDir::new("doctor-stale-image");
    let project = scratch.path().join("project");
    let init = run(&["init", &project.display().to_string()]);
    assert_eq!(code(&init), EXIT_OK, "{}", stderr(&init));

    let manifest_path = project.join("ingot.toml");
    let mut manifest = std::fs::read_to_string(&manifest_path).unwrap();
    manifest.push_str("\n[run]\nimage = \"ingot/run:0.0.1\"\n");
    std::fs::write(&manifest_path, manifest).unwrap();

    let output = Command::new(binary())
        .args(["doctor", &project.display().to_string(), "--json"])
        .arg("--color")
        .arg("never")
        .env("PATH", "")
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("OPENAI_API_KEY")
        .output()
        .expect("doctor must be runnable");

    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let mismatch = report["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|check| check["id"] == "container.image-version")
        .expect("doctor must name a stale reference tag");
    assert_eq!(mismatch["status"], "fail");
    assert!(
        mismatch["summary"]
            .as_str()
            .unwrap()
            .contains("ingot/run:0.0.1"),
        "{}",
        stdout(&output)
    );
    assert!(
        mismatch["fix"]
            .as_str()
            .unwrap()
            .contains("ingot image build"),
        "{}",
        stdout(&output)
    );
}

#[test]
fn image_build_refuses_a_checkout_for_another_binary_version() {
    let source = TempDir::new("image-version-mismatch");
    std::fs::create_dir_all(source.path().join("tools")).unwrap();
    std::fs::write(
        source.path().join("Cargo.toml"),
        "[workspace]\n[workspace.package]\nversion = \"9.9.9\"\n",
    )
    .unwrap();
    std::fs::write(
        source.path().join("tools/ingot.Dockerfile"),
        "FROM scratch\n",
    )
    .unwrap();

    let output = run(&["image", "build", &source.path().display().to_string()]);
    assert_eq!(code(&output), EXIT_FAILURE);
    let log = stderr(&output);
    assert!(log.contains("source version 9.9.9"), "{log}");
    assert!(log.contains(env!("CARGO_PKG_VERSION")), "{log}");
    assert!(log.contains("does not match"), "{log}");
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
            "ingot run --provider replay --input topic=\"compiler design\"",
            &["--input", "topic=compiler design"][..],
        ),
        (
            "document-workflow",
            "DocumentWorkflow",
            Some("examples/document.txt"),
            "ingot run --provider replay --input document=@examples/document.txt --input audience=\"project leads\"",
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
        // The short form the README and `ingot init` both print: one cassette,
        // so no path.
        let mut direct = vec!["run", "--provider", "replay"];
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

        // And the long form keeps working, because a project with two
        // recordings has to be able to say which.
        let mut explicit = vec![
            "run",
            "--provider",
            "replay",
            "--cassette",
            "tests/cassettes/example.json",
        ];
        explicit.extend_from_slice(replay_args);
        let named = run_in(&project, &explicit);
        assert_eq!(
            code(&named),
            EXIT_OK,
            "naming the cassette must still work:\n{}",
            stderr(&named)
        );
        assert_eq!(stdout(&named), stdout(&replay));
    }
}

#[test]
fn two_cassettes_make_the_choice_a_question_rather_than_a_guess() {
    // One recording is obvious. Two is a real question — which run did you mean
    // to replay? — and answering it by picking the alphabetically first would
    // replay the wrong thing quietly, which is the only outcome worse than an
    // error.
    let dir = TempDir::new("two-cassettes");
    let project = dir.path().join("agent");
    assert_eq!(
        code(&run(&["init", &project.display().to_string()])),
        EXIT_OK
    );
    std::fs::copy(
        project.join("tests/cassettes/example.json"),
        project.join("tests/cassettes/another.json"),
    )
    .expect("a second cassette");

    let output = run_in(
        &project,
        &[
            "run",
            "--provider",
            "replay",
            "--input",
            "topic=compiler design",
        ],
    );
    assert_ne!(code(&output), EXIT_OK);
    let message = stderr(&output);
    assert!(message.contains("--cassette"), "{message}");
    // Both are named, so the answer is a copy rather than a search.
    assert!(message.contains("example.json"), "{message}");
    assert!(message.contains("another.json"), "{message}");
}

#[test]
fn a_project_with_no_recording_is_told_how_to_make_one() {
    let dir = TempDir::new("no-cassette");
    std::fs::write(
        dir.path().join("main.ing"),
        "language 0.1\n\
         agent A(x: string) -> y<markdown> {\n\
         \x20 model requires { structured_output }\n\
         \x20 budget { steps <= 2 }\n\
         \x20 policy { network deny }\n\
         \x20 flow { emit y = ask<markdown>(\"${x}\") }\n\
         }\n",
    )
    .expect("a source");
    std::fs::write(
        dir.path().join("ingot.toml"),
        "[project]\nname = \"bare\"\n\n[build]\nentry = \"main.ing\"\nout-dir = \"target/ingot\"\n",
    )
    .expect("a manifest");

    let output = run_in(
        dir.path(),
        &["run", "--provider", "replay", "--input", "x=hello"],
    );
    assert_ne!(code(&output), EXIT_OK);
    let message = stderr(&output);
    assert!(message.contains("--record"), "{message}");
    assert!(message.contains("tests/cassettes"), "{message}");
}

#[test]
fn init_prints_commands_that_work_and_none_that_need_a_key() {
    // The first thing anybody sees. Printed instructions that do not run are
    // worse than none, so every command in this list is executed here.
    let dir = TempDir::new("init-next-steps");
    let project = dir.path().join("agent");
    let created = run(&["init", &project.display().to_string()]);
    assert_eq!(code(&created), EXIT_OK, "{}", stderr(&created));

    let printed = stdout(&created);
    assert!(
        printed.contains("none of these need an API key"),
        "{printed}"
    );
    assert!(printed.contains("ingot studio"), "{printed}");

    for line in printed.lines() {
        let command = line.trim();
        let Some(rest) = command.strip_prefix("ingot ") else {
            continue;
        };
        // `ingot studio` serves until interrupted, so it is named rather than
        // run; everything else has to finish.
        let rest = rest.split('#').next().unwrap_or(rest).trim();
        if rest == "studio" {
            continue;
        }
        let args = shell_words(rest);
        let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
        let output = run_in(&project, &borrowed);
        assert_eq!(
            code(&output),
            EXIT_OK,
            "`ingot {rest}` is printed by init and does not work:\n{}",
            stderr(&output)
        );
    }
}

/// Split a printed command line, honouring the double quotes `init` prints.
fn shell_words(line: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for character in line.chars() {
        match character {
            '"' => quoted = !quoted,
            ' ' if !quoted => {
                if !current.is_empty() {
                    words.push(std::mem::take(&mut current));
                }
            }
            other => current.push(other),
        }
    }
    if !current.is_empty() {
        words.push(current);
    }
    words
}

#[test]
fn model_assistance_leaves_a_project_that_works_without_the_model() {
    let dir = TempDir::new("new-offline-project");
    let project = dir.path().join("audience-brief");

    let output = run(&[
        "new",
        "--out-dir",
        &project.display().to_string(),
        "summarise",
        "documents",
        "for",
        "project",
        "leads",
    ]);
    assert_eq!(code(&output), EXIT_OK, "{}", stderr(&output));
    let out = stdout(&output);
    assert!(
        out.contains("Created compiler-verified agent project"),
        "{out}"
    );
    assert!(out.contains("Template: document-workflow"), "{out}");

    for path in [
        "ingot.toml",
        "main.ing",
        "README.md",
        ".gitignore",
        "examples/document.txt",
        "tests/cassettes/example.json",
    ] {
        assert!(project.join(path).is_file(), "missing {path}");
    }

    let manifest = std::fs::read_to_string(project.join("ingot.toml")).expect("manifest");
    assert!(
        manifest.contains("Authored from workflow: summarise documents for project leads"),
        "{manifest}"
    );
    assert!(
        !manifest.contains("api") && !manifest.contains("key") && !manifest.contains("token"),
        "generated manifest must not contain credential-shaped fields:\n{manifest}"
    );

    for args in [&["check"][..], &["build"][..], &["test"][..]] {
        let output = run_in(&project, args);
        assert_eq!(
            code(&output),
            EXIT_OK,
            "generated project command `{}` must work without a provider key:\n{}",
            args.join(" "),
            stderr(&output)
        );
    }

    assert!(
        project
            .join("target/ingot/DocumentWorkflow.ir.json")
            .is_file(),
        "build must leave a normal IR artifact"
    );

    let replay = run_in(
        &project,
        &[
            "run",
            "--provider",
            "replay",
            "--cassette",
            "tests/cassettes/example.json",
            "--input",
            "document=@examples/document.txt",
            "--input",
            "audience=project leads",
        ],
    );
    assert_eq!(
        code(&replay),
        EXIT_OK,
        "generated project replay must work without a provider key:\n{}",
        stderr(&replay)
    );
    assert!(!stdout(&replay).trim().is_empty());
}

// --- provider-backed authoring ---------------------------------------------

/// A recorded authoring session: one reply per proposal, in order.
///
/// Authoring replays leniently, so a fixture stays valid when the authoring
/// prompt gains a sentence. What it pins is the source a model proposed — which
/// the compiler then verifies from scratch — and not the prompt that asked.
fn authoring_cassette(dir: &Path, name: &str, replies: &[&str]) -> PathBuf {
    let interactions: Vec<serde_json::Value> = replies
        .iter()
        .enumerate()
        .map(|(index, reply)| {
            serde_json::json!({
                "index": index,
                "node": format!("authoring.{index}"),
                "requestDigest": "0".repeat(64),
                "responseType": "text",
                "value": format!("```ingot\n{reply}```"),
                "usage": { "inputTokens": 800, "outputTokens": 200 },
                "model": "test/authoring",
            })
        })
        .collect();
    let cassette = serde_json::json!({
        "cassetteVersion": "0.1",
        "agent": "ingot.authoring",
        "interactions": interactions,
    });

    let path = dir.join(name);
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&cassette).expect("a cassette is serializable"),
    )
    .expect("writing the authoring cassette");
    path
}

/// What a well-behaved authoring model returns: ordinary source, no tool it
/// cannot route, and the default-deny policy left alone.
const AUTHORED_SOURCE: &str = r#"language 0.1
package audience_brief

/// Summarises a document for a named audience.
agent DocumentBrief(document: text, audience: string) -> summary<markdown> {
  model requires {
    structured_output
  }

  budget {
    steps <= 4
    tokens <= 20000
  }

  policy {
    network deny
  }

  flow {
    emit summary = ask<markdown>("Summarise ${document} for ${audience}.")
  }
}
"#;

#[test]
fn a_model_authored_project_is_ordinary_source_that_needs_no_model_afterwards() {
    let dir = TempDir::new("new-model-authored");
    let cassette = authoring_cassette(dir.path(), "authoring.json", &[AUTHORED_SOURCE]);
    let project = dir.path().join("audience-brief");

    let output = run(&[
        "new",
        "--out-dir",
        &project.display().to_string(),
        "--provider",
        "replay",
        "--cassette",
        &cassette.display().to_string(),
        "summarise",
        "a",
        "document",
        "for",
        "an",
        "audience",
    ]);
    assert_eq!(code(&output), EXIT_OK, "{}", stderr(&output));

    for path in [
        "ingot.toml",
        "main.ing",
        "README.md",
        ".gitignore",
        "examples/document.txt",
    ] {
        assert!(project.join(path).is_file(), "missing {path}");
    }

    // The authored source is what was written — not a template wearing its name.
    let source = std::fs::read_to_string(project.join("main.ing")).expect("authored source");
    assert_eq!(source, AUTHORED_SOURCE);

    let manifest = std::fs::read_to_string(project.join("ingot.toml")).expect("manifest");
    assert!(
        manifest.contains("Authored from workflow: summarise a document for an audience"),
        "{manifest}"
    );
    assert!(
        !manifest.contains("key") && !manifest.contains("token"),
        "a generated manifest must carry no credential-shaped field:\n{manifest}"
    );

    // Every command the project needs from here works with no provider at all.
    for args in [&["check"][..], &["build"][..], &["test"][..]] {
        let output = run_in(&project, args);
        assert_eq!(
            code(&output),
            EXIT_OK,
            "authored project command `{}` must work without a provider key:\n{}",
            args.join(" "),
            stderr(&output)
        );
    }
    assert!(
        project.join("target/ingot/DocumentBrief.ir.json").is_file(),
        "build must leave a normal IR artifact"
    );

    // No cassette is fabricated, so the README has to say how to make a real one.
    let readme = std::fs::read_to_string(project.join("README.md")).expect("authored README");
    let record = "ingot run --record tests/cassettes/example.json \
                  --input audience=\"...\" --input document=@examples/document.txt";
    assert!(
        readme.lines().any(|line| line.trim() == record),
        "the README must print the one command that creates the offline test:\n{readme}"
    );
    assert!(
        !project.join("tests/cassettes/example.json").exists(),
        "a recorded answer no model produced would be a test that proves nothing"
    );
}

#[test]
fn a_model_repair_is_bounded_and_driven_by_compiler_diagnostics() {
    let dir = TempDir::new("new-model-repair");
    let broken = AUTHORED_SOURCE.replace(
        "ask<markdown>(\"Summarise ${document} for ${audience}.\")",
        "missing",
    );
    let cassette = authoring_cassette(dir.path(), "authoring.json", &[&broken, AUTHORED_SOURCE]);
    let project = dir.path().join("repaired");

    let output = run(&[
        "new",
        "--out-dir",
        &project.display().to_string(),
        "--provider",
        "replay",
        "--cassette",
        &cassette.display().to_string(),
        "--max-repairs",
        "1",
        "summarise",
        "a",
        "document",
    ]);
    assert_eq!(code(&output), EXIT_OK, "{}", stderr(&output));
    assert_eq!(
        std::fs::read_to_string(project.join("main.ing")).expect("authored source"),
        AUTHORED_SOURCE
    );

    // The same cassette with no repair allowance stops, showing its work.
    let stopped = dir.path().join("stopped");
    let output = run(&[
        "new",
        "--out-dir",
        &stopped.display().to_string(),
        "--provider",
        "replay",
        "--cassette",
        &cassette.display().to_string(),
        "--max-repairs",
        "0",
        "summarise",
        "a",
        "document",
    ]);
    assert_eq!(code(&output), EXIT_DIAGNOSTICS, "{}", stderr(&output));
    let out = stdout(&output);
    assert!(
        out.contains("compiler repair reached retry ceiling after 1 attempt(s)"),
        "{out}"
    );
    assert!(out.contains("ING2001"), "{out}");
    assert!(!stopped.exists(), "a stopped loop must write no project");
}

#[test]
fn a_model_authored_policy_grant_is_refused_until_the_operator_accepts_it() {
    let dir = TempDir::new("new-model-policy");
    let source = r#"language 0.1

/// Fetches and summarises a page.
agent Fetcher(url: string) -> summary<markdown> {
  policy {
    network allow ["example.com"]
  }

  flow {
    emit summary = ask<markdown>("Summarise ${url}.")
  }
}
"#;
    let cassette = authoring_cassette(dir.path(), "authoring.json", &[source, source]);
    let refused = dir.path().join("refused");

    let output = run(&[
        "new",
        "--out-dir",
        &refused.display().to_string(),
        "--provider",
        "replay",
        "--cassette",
        &cassette.display().to_string(),
        "fetch",
        "and",
        "summarise",
        "a",
        "page",
    ]);
    assert_eq!(code(&output), EXIT_DIAGNOSTICS, "{}", stderr(&output));
    let out = stdout(&output);
    assert!(
        out.contains("agent Fetcher: network allow [\"example.com\"]"),
        "the grant must be named before it can be accepted:\n{out}"
    );
    assert!(out.contains("--accept-policy"), "{out}");
    assert!(
        !refused.exists(),
        "a grant the operator has not accepted must write nothing"
    );

    // Accepting is a separate, explicit decision — and is still recorded.
    let accepted = dir.path().join("accepted");
    let output = run(&[
        "new",
        "--out-dir",
        &accepted.display().to_string(),
        "--provider",
        "replay",
        "--cassette",
        &cassette.display().to_string(),
        "--accept-policy",
        "fetch",
        "and",
        "summarise",
        "a",
        "page",
    ]);
    assert_eq!(code(&output), EXIT_OK, "{}", stderr(&output));
    assert!(
        stdout(&output).contains("accepted policy grant: agent Fetcher: network allow"),
        "{}",
        stdout(&output)
    );
    assert!(accepted.join("main.ing").is_file());
}

#[test]
fn authoring_refuses_a_credential_before_it_reaches_a_file_or_a_prompt() {
    let dir = TempDir::new("new-model-credential");
    let key = "sk-live-4f9ac1d3b7e25a86";
    let source = format!(
        r#"language 0.1

agent Leaky(topic: string) -> brief<markdown> {{
  policy {{
    network deny
  }}

  flow {{
    emit brief = ask<markdown>("Use api_key=\"{key}\" and brief ${{topic}}.")
  }}
}}
"#
    );
    let cassette = authoring_cassette(dir.path(), "authoring.json", &[&source, &source, &source]);
    let project = dir.path().join("leaky");

    let output = run(&[
        "new",
        "--out-dir",
        &project.display().to_string(),
        "--provider",
        "replay",
        "--cassette",
        &cassette.display().to_string(),
        "brief",
        "a",
        "topic",
    ]);
    assert_eq!(code(&output), EXIT_DIAGNOSTICS, "{}", stderr(&output));
    let out = stdout(&output);
    assert!(out.contains("a vendor-prefixed API key"), "{out}");
    assert!(
        !out.contains(key) && !stderr(&output).contains(key),
        "the report must name the shape, never repeat the value"
    );
    assert!(
        !project.exists(),
        "a candidate carrying a credential must reach no file"
    );

    // And the same rule applies to the words the operator typed, before they
    // reach a prompt, a manifest or this terminal's history.
    let pasted = dir.path().join("pasted");
    let output = run(&[
        "new",
        "--out-dir",
        &pasted.display().to_string(),
        &format!("brief a topic with api_key={key}"),
    ]);
    assert_eq!(code(&output), EXIT_FAILURE, "{}", stdout(&output));
    assert!(!stderr(&output).contains(key), "{}", stderr(&output));
    assert!(!pasted.exists());
}

#[test]
fn an_authored_change_to_an_existing_project_is_a_diff_until_it_is_applied() {
    let dir = TempDir::new("new-model-diff");
    let project = dir.path().join("brief-agent");
    let output = run(&["init", &project.display().to_string()]);
    assert_eq!(code(&output), EXIT_OK, "{}", stderr(&output));

    let before = std::fs::read_to_string(project.join("main.ing")).expect("template source");
    let after = before.replace(
        "Use headings and bullet points.",
        "Use headings, and at most five bullet points.",
    );
    assert_ne!(before, after, "the fixture must actually change something");
    let cassette = authoring_cassette(dir.path(), "authoring.json", &[&after]);

    let propose = [
        "new",
        "--project",
        &project.display().to_string(),
        "--provider",
        "replay",
        "--cassette",
        &cassette.display().to_string(),
        "keep",
        "the",
        "brief",
        "shorter",
    ];
    let output = run(&propose);
    assert_eq!(code(&output), EXIT_OK, "{}", stderr(&output));
    let out = stdout(&output);
    assert!(
        out.contains("@@ "),
        "a proposal must be shown as a diff:\n{out}"
    );
    assert!(
        out.contains("-      \"Write a short, factual brief about ${topic}. Use headings and bullet points.\""),
        "{out}"
    );
    assert!(out.contains("nothing was written"), "{out}");
    assert_eq!(
        std::fs::read_to_string(project.join("main.ing")).expect("source"),
        before,
        "a proposal must not mutate the project"
    );

    let mut apply = propose.to_vec();
    apply.insert(1, "--apply");
    let output = run(&apply);
    assert_eq!(code(&output), EXIT_OK, "{}", stderr(&output));
    assert!(stdout(&output).contains("wrote"), "{}", stdout(&output));
    assert_eq!(
        std::fs::read_to_string(project.join("main.ing")).expect("source"),
        after
    );
    assert_eq!(
        code(&run_in(&project, &["check"])),
        EXIT_OK,
        "the applied source must still compile"
    );
}

#[test]
fn an_authoring_model_cannot_invent_a_tool_the_project_does_not_route() {
    let dir = TempDir::new("new-model-tool");
    let source = r#"language 0.1

tool web.search(query: string) -> string[] !network

agent Research(topic: string) -> report<markdown> {
  tools { mcp web.search }
  policy {
    network allow ["example.com"]
  }
  flow {
    hits = call web.search(topic)
    emit report = ask<markdown>("Draft", context: hits)
  }
}
"#;
    let cassette = authoring_cassette(dir.path(), "authoring.json", &[source]);
    let project = dir.path().join("research");

    // `--accept-policy` so the run gets past the grant and reaches the tool
    // check: the point here is the tool, not the policy.
    let output = run(&[
        "new",
        "--out-dir",
        &project.display().to_string(),
        "--provider",
        "replay",
        "--cassette",
        &cassette.display().to_string(),
        "--accept-policy",
        "--max-repairs",
        "0",
        "research",
        "a",
        "topic",
    ]);
    assert_eq!(code(&output), EXIT_DIAGNOSTICS, "{}", stderr(&output));
    let out = stdout(&output);
    assert!(
        out.contains("AUTHORING_UNROUTED_TOOL"),
        "an invented tool must be a named diagnostic:\n{out}"
    );
    assert!(out.contains("configures no MCP server"), "{out}");
    assert!(!project.exists(), "nothing may be written for it");
}

#[test]
fn new_review_separates_policy_from_automatic_repair() {
    let dir = TempDir::new("new-review-policy");
    let previous = dir.path().join("previous.ing");
    let candidate = dir.path().join("candidate.ing");
    std::fs::write(
        &previous,
        r#"
language 0.1
package demo

tool web.search(query: string) -> string[] !network

agent Research(topic: string) -> report<markdown> {
  tools { mcp web.search }
  flow {
    emit report = ask<markdown>("draft ${topic}")
  }
}
"#,
    )
    .expect("writing previous source");
    std::fs::write(
        &candidate,
        r##"
language 0.1
package demo

tool web.search(query: string) -> string[] !network

agent Research(topic: string) -> report<markdown> {
  tools { mcp web.search }
  policy {
    network allow ["example.com"]
  }
  flow {
    hits = call web.search(topic)
    emit report = ask<markdown>("draft", context: hits)
  }
}
"##,
    )
    .expect("writing candidate source");

    let output = run(&[
        "new",
        "--previous",
        &previous.display().to_string(),
        "--candidate",
        &candidate.display().to_string(),
        "review",
        "papers",
    ]);

    assert_eq!(code(&output), EXIT_DIAGNOSTICS, "{}", stderr(&output));
    let out = stdout(&output);
    assert!(
        out.contains("candidate source requests policy changes"),
        "{out}"
    );
    assert!(
        out.contains("agent Research: network allow [\"example.com\"]"),
        "{out}"
    );
    assert!(
        out.contains("not part of automatic compiler repair"),
        "{out}"
    );
}

#[test]
fn new_repair_loop_stops_at_the_first_compiling_candidate() {
    let dir = TempDir::new("new-repair-success");
    let previous = dir.path().join("previous.ing");
    let candidate = dir.path().join("candidate.ing");
    let repaired = dir.path().join("repaired.ing");
    let source = r#"
language 0.1
package demo

agent Brief(topic: string) -> report<markdown> {
  flow {
    emit report = ask<markdown>("draft ${topic}")
  }
}
"#;
    std::fs::write(&previous, source).expect("writing previous source");
    std::fs::write(
        &candidate,
        source.replace("ask<markdown>(\"draft ${topic}\")", "missing"),
    )
    .expect("writing broken candidate source");
    std::fs::write(
        &repaired,
        source.replace("draft ${topic}", "repaired draft ${topic}"),
    )
    .expect("writing repaired source");

    let output = run(&[
        "new",
        "--previous",
        &previous.display().to_string(),
        "--candidate",
        &candidate.display().to_string(),
        "--repair-candidate",
        &repaired.display().to_string(),
        "--max-repairs",
        "1",
        "draft",
        "a",
        "brief",
    ]);

    assert_eq!(code(&output), EXIT_OK, "{}", stderr(&output));
    let out = stdout(&output);
    assert!(
        out.contains("compiler-verified authoring completed after 2 attempt(s)"),
        "{out}"
    );
    assert!(
        out.contains("attempt 1 failed compiler verification"),
        "{out}"
    );
    assert!(
        out.contains("attempt 2 passed compiler verification"),
        "{out}"
    );
    assert!(out.contains("repaired draft ${topic}"), "{out}");
}

#[test]
fn new_repair_loop_stops_with_source_and_diagnostics_at_the_ceiling() {
    let dir = TempDir::new("new-repair-ceiling");
    let previous = dir.path().join("previous.ing");
    let candidate = dir.path().join("candidate.ing");
    let source = r#"
language 0.1
package demo

agent Brief(topic: string) -> report<markdown> {
  flow {
    emit report = ask<markdown>("draft ${topic}")
  }
}
"#;
    let broken = source.replace("ask<markdown>(\"draft ${topic}\")", "missing");
    std::fs::write(&previous, source).expect("writing previous source");
    std::fs::write(&candidate, &broken).expect("writing broken candidate source");

    let output = run(&[
        "new",
        "--previous",
        &previous.display().to_string(),
        "--candidate",
        &candidate.display().to_string(),
        "--max-repairs",
        "0",
        "draft",
        "a",
        "brief",
    ]);

    assert_eq!(code(&output), EXIT_DIAGNOSTICS);
    let out = stdout(&output);
    assert!(
        out.contains("compiler repair reached retry ceiling after 1 attempt(s)"),
        "{out}"
    );
    assert!(out.contains("last source:"), "{out}");
    assert!(out.contains("emit report = missing"), "{out}");
    assert!(stderr(&output).contains("ING2001"), "{}", stderr(&output));
}

#[test]
fn a_verifier_nothing_can_perform_is_a_warning_before_the_run() {
    let dir = TempDir::new("verify-warning");
    let source = dir.path().join("main.ing");
    std::fs::write(
        &source,
        r#"language 0.1

verifier CitationCheck(draft: markdown, min_sources: int)

agent Brief(topic: string) -> report<markdown> {
  flow {
    draft = ask<markdown>("Write about ${topic}")
    verify CitationCheck(draft, min_sources: 8)
    emit report = draft
  }
}
"#,
    )
    .expect("writing the source");

    let output = run(&["check", &source.display().to_string()]);
    // A warning, not an error: the declaration is correct and keeps its meaning
    // when verifiers gain an execution model.
    assert_eq!(code(&output), EXIT_OK, "{}", stderr(&output));
    let message = stderr(&output);
    assert!(message.contains("warning[ING6006]"), "{message}");
    assert!(message.contains("CitationCheck"), "{message}");
    assert!(message.contains("notPerformed"), "{message}");

    let explained = run(&["explain", "ING6006"]);
    assert_eq!(code(&explained), EXIT_OK, "{}", stderr(&explained));
    assert!(
        stdout(&explained).contains("nothing can carry one out"),
        "{}",
        stdout(&explained)
    );
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

/// The editor and the command line must not disagree about a program.
///
/// Both read the same compiler, so this is a test that they keep doing so: a
/// second diagnostic path — a "quick" editor lint, a CLI-only filter — is how a
/// project ends up with two answers to "is this source correct".
///
/// It compares the **compiler's** diagnostics. `ING5007` is deliberately not
/// among them: an unchargeable cost budget is a fact about the deployment's
/// prices, which the CLI can see and an editor holding only source cannot.
#[test]
fn editor_and_cli_diagnostics_are_identical() {
    let dir = TempDir::new("editor-cli-parity");
    let cases: [(&str, &str); 3] = [
        (
            "errors.ing",
            r#"language 0.1

tool web.search(query: string) -> string[] !network

agent Broken(topic: string) -> report<markdown> {
  tools { mcp web.search }
  policy {
    network deny
  }
  flow {
    hits = call web.search(topic)
    emit report = missing
  }
}
"#,
        ),
        (
            "warnings.ing",
            r#"language 0.1

verifier CitationCheck(draft: markdown, min_sources: int)

agent Warned(topic: string) -> report<markdown> {
  flow {
    draft = ask<markdown>("Write about ${topic}")
    unused = ask<markdown>("Also write something else")
    verify CitationCheck(draft, min_sources: 8)
    emit report = draft
  }
}
"#,
        ),
        (
            "clean.ing",
            r#"language 0.1

agent Clean(topic: string) -> report<markdown> {
  flow {
    emit report = ask<markdown>("Write about ${topic}")
  }
}
"#,
        ),
    ];

    let service = ingot_language_service::LanguageService::new();
    for (name, source) in cases {
        let path = dir.path().join(name);
        std::fs::write(&path, source).expect("writing the source");

        let editor = service.check_source(name, source);
        let editor_seen: Vec<String> = editor
            .diagnostics
            .iter()
            .map(|diagnostic| format!("{:?}:{}", diagnostic.severity, diagnostic.code))
            .collect();

        let output = run(&["check", &path.display().to_string()]);
        let rendered = stderr(&output);
        let cli_seen: Vec<String> = rendered
            .lines()
            .filter_map(|line| {
                let (severity, rest) = line
                    .strip_prefix("error[")
                    .map(|rest| ("Error", rest))
                    .or_else(|| line.strip_prefix("warning[").map(|rest| ("Warning", rest)))?;
                let code = rest.split(']').next()?;
                Some(format!("{severity}:{code}"))
            })
            .collect();

        assert_eq!(
            editor_seen, cli_seen,
            "`{name}`: the editor and the CLI disagree\n{rendered}"
        );
        assert_eq!(
            editor.has_errors,
            code(&output) == EXIT_DIAGNOSTICS,
            "`{name}`: the editor and the CLI disagree about whether it builds"
        );
        assert_eq!(
            editor.warning_count,
            cli_seen
                .iter()
                .filter(|seen| seen.starts_with("Warning"))
                .count(),
            "`{name}`: warning counts differ\n{rendered}"
        );
    }

    // The cases have to be worth comparing: one that fails, one that only warns,
    // one that is clean.
    let broken = service.check_source("errors.ing", cases[0].1);
    assert!(broken.error_count > 0);
    let warned = service.check_source("warnings.ing", cases[1].1);
    assert!(!warned.has_errors && warned.warning_count > 0);
    let clean = service.check_source("clean.ing", cases[2].1);
    assert!(clean.diagnostics.is_empty(), "{:?}", clean.diagnostics);
}
