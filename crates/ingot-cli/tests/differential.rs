//! The same artifact through two independent backends.
//!
//! This is the test the project's central claim rests on. Everything else about
//! portability is a design intention; this is the only thing that can observe it.
//! Two programs — one Rust, one generated Python — read the same Agent IR
//! document and the same recorded exchange, and their artifacts are compared byte
//! for byte.
//!
//! A disagreement is a finding, never a flake. When one appears, the
//! specification decides who is wrong, and if the specification is silent that is
//! the most valuable outcome the test can produce. See
//! [RFC-0006](../../../rfcs/0006-a-second-backend.md).
//!
//! The tests skip where no Python 3 is on PATH. `INGOT_REQUIRE_PYTHON=1` — as CI
//! does — turns that skip into a failure, because a test that silently does not
//! run is worse than no test.

mod support;

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use support::*;

/// A Python 3 interpreter, if this machine has one.
fn python() -> Option<String> {
    for candidate in ["python3", "python"] {
        let ok = Command::new(candidate)
            .arg("--version")
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false);
        if ok {
            return Some(candidate.to_string());
        }
    }
    if std::env::var_os("INGOT_REQUIRE_PYTHON").is_some() {
        panic!("INGOT_REQUIRE_PYTHON is set and neither `python3` nor `python` is on PATH");
    }
    eprintln!("skipping: no python3 on PATH");
    None
}

fn example(name: &str) -> String {
    repo_root()
        .join("examples")
        .join(name)
        .display()
        .to_string()
}

fn summarizer_cassette() -> String {
    repo_root()
        .join("examples/document-summarizer/tests/cassettes/brief.json")
        .display()
        .to_string()
}

/// The document the shipped cassette was recorded against, byte for byte.
///
/// Written to a file rather than passed inline: the recorded text has newlines,
/// and a shell would mangle them differently on each platform — which would look
/// like a backend disagreement and be nothing of the kind.
fn recorded_document(dir: &TempDir) -> String {
    let text = std::fs::read_to_string(summarizer_cassette()).expect("the cassette must be there");
    let cassette: serde_json::Value = serde_json::from_str(&text).expect("it must parse");
    let document = cassette["inputs"]["document"]
        .as_str()
        .expect("the cassette records the document");
    let path = dir.path().join("doc.txt");
    std::fs::write(&path, document).expect("writing the document");
    format!("document=@{}", path.display())
}

/// Compile an agent for the python target, into `dir`.
fn build_python(project: &str, dir: &Path, extra: &[&str]) -> Output {
    let out_dir = dir.display().to_string();
    let mut args = vec![
        "build",
        project,
        "--target",
        "python",
        "--out-dir",
        &out_dir,
    ];
    args.extend_from_slice(extra);
    run_env(&args, &[])
}

/// The one `.py` a build produced.
fn only_program(dir: &Path) -> PathBuf {
    let mut found: Vec<PathBuf> = std::fs::read_dir(dir)
        .expect("the output directory must exist")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().map(|ext| ext == "py").unwrap_or(false))
        .collect();
    found.sort();
    assert_eq!(found.len(), 1, "expected one generated program: {found:?}");
    found.remove(0)
}

/// Run a generated program, with a clean environment.
fn run_generated(python: &str, program: &Path, args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut command = Command::new(python);
    command.arg(program).args(args);
    for name in [
        "ANTHROPIC_API_KEY",
        "INGOT_ANTHROPIC_BASE_URL",
        "OPENAI_API_KEY",
        "INGOT_OPENAI_BASE_URL",
    ] {
        command.env_remove(name);
    }
    // So a mismatched console code page cannot turn a byte comparison into a
    // platform difference.
    command.env("PYTHONIOENCODING", "utf-8");
    for (name, value) in env {
        command.env(name, value);
    }
    command.output().expect("python must be runnable")
}

/// A project with one agent, written fresh so a test can choose its budget.
fn project(dir: &Path, source: &str) {
    std::fs::write(dir.join("main.ing"), source).expect("writing the source");
    std::fs::write(
        dir.join("ingot.toml"),
        "[project]\nname = \"probe\"\nversion = \"0.1.0\"\n\n\
         [build]\nentry = \"main.ing\"\nout-dir = \"target/ingot\"\n",
    )
    .expect("writing the manifest");
}

// --- the claim ----------------------------------------------------------------

#[test]
fn the_document_summarizer_produces_identical_artifacts_in_both_backends() {
    let Some(python) = python() else { return };

    let work = TempDir::new("diff-summarizer");
    let document = recorded_document(&work);
    let build = work.path().join("build");
    let reference_out = work.path().join("reference").display().to_string();
    let python_out = work.path().join("python").display().to_string();
    let project = example("document-summarizer");
    let cassette = summarizer_cassette();

    let built = build_python(&project, &build, &[]);
    assert_eq!(code(&built), EXIT_OK, "{}", stderr(&built));

    let shared = [
        "--cassette",
        cassette.as_str(),
        "--input",
        "audience=engineering leads",
        "--input",
        document.as_str(),
    ];

    // The reference interpreter.
    let mut reference_args = vec!["run", project.as_str(), "--provider", "replay"];
    reference_args.extend_from_slice(&shared);
    reference_args.extend_from_slice(&["--out-dir", reference_out.as_str()]);
    let first = run_env(&reference_args, &[]);
    assert_eq!(code(&first), EXIT_OK, "{}", stderr(&first));

    // The generated program. No `ingot` in this process at all.
    let mut python_args = shared.to_vec();
    python_args.extend_from_slice(&["--out-dir", python_out.as_str()]);
    let second = run_generated(&python, &only_program(&build), &python_args, &[]);
    assert!(
        second.status.success(),
        "{}",
        String::from_utf8_lossy(&second.stderr)
    );

    let left = std::fs::read(Path::new(&reference_out).join("summary.md"))
        .expect("the reference wrote it");
    let right = std::fs::read(Path::new(&python_out).join("summary.md")).expect("python wrote it");
    assert_eq!(
        String::from_utf8_lossy(&left),
        String::from_utf8_lossy(&right),
        "two backends, one artifact — a difference here is a finding, not a flake"
    );
    assert_eq!(left, right, "byte for byte, not merely equal as text");
}

#[test]
fn the_event_streams_agree_on_kind_and_order() {
    // Runtime 0.1 §9 forbids timestamps and durations in events for exactly this
    // reason: the sequence is comparable across implementations.
    let Some(python) = python() else { return };

    let work = TempDir::new("diff-events");
    let document = recorded_document(&work);
    let build = work.path().join("build");
    let project = example("document-summarizer");
    let cassette = summarizer_cassette();

    let built = build_python(&project, &build, &[]);
    assert_eq!(code(&built), EXIT_OK, "{}", stderr(&built));

    let shared = [
        "--cassette",
        cassette.as_str(),
        "--input",
        "audience=engineering leads",
        "--input",
        document.as_str(),
        "--events",
        "json",
    ];

    let mut reference_args = vec!["run", project.as_str(), "--provider", "replay"];
    reference_args.extend_from_slice(&shared);
    let first = run_env(&reference_args, &[]);
    assert_eq!(code(&first), EXIT_OK, "{}", stderr(&first));

    let second = run_generated(&python, &only_program(&build), &shared, &[]);
    assert!(
        second.status.success(),
        "{}",
        String::from_utf8_lossy(&second.stderr)
    );

    let kinds = |text: &str| -> Vec<String> {
        text.lines()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .filter_map(|event| event["event"].as_str().map(str::to_string))
            .collect()
    };
    let left = kinds(&stderr(&first));
    let right = kinds(&String::from_utf8_lossy(&second.stderr));

    assert!(!left.is_empty(), "the reference emitted no events");
    assert_eq!(
        left, right,
        "the two backends disagree on which events happen, or in what order"
    );
}

// --- the second backend must be no weaker than the first ----------------------

#[test]
fn the_step_budget_is_carried_by_the_generated_program() {
    // Runtime 0.1 §8. A backend may be stricter than the artifact and must never
    // be looser, so the number has to travel with the program rather than being
    // re-derived by whoever runs it.
    let dir = TempDir::new("diff-steps");
    project(
        dir.path(),
        "language 0.1\n\
         agent Once(topic: string) -> note<markdown> {\n\
         \x20 model requires { structured_output }\n\
         \x20 budget { steps <= 1 tokens <= 4000 }\n\
         \x20 policy { network deny }\n\
         \x20 flow {\n\
         \x20   emit note = ask<markdown>(\"One line about ${topic}.\")\n\
         \x20 }\n\
         }\n",
    );

    let build = dir.path().join("build");
    let built = build_python(&dir.path().display().to_string(), &build, &[]);
    assert_eq!(code(&built), EXIT_OK, "{}", stderr(&built));

    let source = std::fs::read_to_string(only_program(&build)).unwrap();
    assert!(
        source.contains("\"maxSteps\": 1"),
        "the program has to carry the budget it enforces:\n{source}"
    );
    assert!(
        source.contains("`steps` budget"),
        "and the code that enforces it:\n{source}"
    );
}

#[test]
fn the_token_budget_is_enforced_the_same_way_in_both_backends() {
    let Some(python) = python() else { return };

    let dir = TempDir::new("diff-tokens");
    // The stub reports 120 in and 40 out per call, so a budget of 100 is exceeded
    // by the first one. The budget must stop the run, not the provider.
    project(
        dir.path(),
        "language 0.1\n\
         agent Small(topic: string) -> note<markdown> {\n\
         \x20 model requires { structured_output }\n\
         \x20 budget { steps <= 2 tokens <= 100 }\n\
         \x20 policy { network deny }\n\
         \x20 flow {\n\
         \x20   emit note = ask<markdown>(\"One line about ${topic}.\")\n\
         \x20 }\n\
         }\n",
    );
    let project_path = dir.path().display().to_string();

    let build = dir.path().join("build");
    let built = build_python(&project_path, &build, &[]);
    assert_eq!(code(&built), EXIT_OK, "{}", stderr(&built));

    let stub = stub_provider(vec![text_reply("one")]);
    let refused = run_generated(
        &python,
        &only_program(&build),
        &["--provider", "anthropic", "--input", "topic=compilers"],
        &[
            ("ANTHROPIC_API_KEY", "stub-key"),
            ("INGOT_ANTHROPIC_BASE_URL", &stub.url),
        ],
    );
    let log = String::from_utf8_lossy(&refused.stderr);
    assert!(!refused.status.success(), "{log}");
    assert!(log.contains("`tokens` budget of 100"), "{log}");

    // And the reference interpreter refuses the same artifact with the same
    // sentence. Two backends agreeing on a failure matters as much as agreeing on
    // a success.
    let other = stub_provider(vec![text_reply("one")]);
    let same = run_env(
        &[
            "run",
            &project_path,
            "--provider",
            "anthropic",
            "--input",
            "topic=compilers",
        ],
        &[
            ("ANTHROPIC_API_KEY", "stub-key"),
            ("INGOT_ANTHROPIC_BASE_URL", &other.url),
        ],
    );
    assert_eq!(code(&same), EXIT_DIAGNOSTICS, "{}", stderr(&same));
    assert!(
        stderr(&same).contains("`tokens` budget of 100"),
        "{}",
        stderr(&same)
    );
}

#[test]
fn a_missing_input_is_the_operators_problem_in_both_backends() {
    let Some(python) = python() else { return };

    let work = TempDir::new("diff-inputs");
    let build = work.path().join("build");
    let cassette = summarizer_cassette();
    let built = build_python(&example("document-summarizer"), &build, &[]);
    assert_eq!(code(&built), EXIT_OK, "{}", stderr(&built));

    let refused = run_generated(
        &python,
        &only_program(&build),
        &[
            "--cassette",
            cassette.as_str(),
            "--input",
            "audience=engineering leads",
        ],
        &[],
    );
    let log = String::from_utf8_lossy(&refused.stderr);
    assert!(!refused.status.success(), "{log}");
    assert!(log.contains("missing input `document`"), "{log}");
    assert!(log.contains("not with the agent itself"), "{log}");
}

#[test]
fn an_input_this_agent_does_not_declare_is_refused_by_the_generated_program() {
    let Some(python) = python() else { return };

    let work = TempDir::new("diff-unknown-input");
    let build = work.path().join("build");
    let cassette = summarizer_cassette();
    let built = build_python(&example("document-summarizer"), &build, &[]);
    assert_eq!(code(&built), EXIT_OK, "{}", stderr(&built));

    let refused = run_generated(
        &python,
        &only_program(&build),
        &["--cassette", cassette.as_str(), "--input", "nonsense=1"],
        &[],
    );
    let log = String::from_utf8_lossy(&refused.stderr);
    assert!(!refused.status.success(), "{log}");
    assert!(log.contains("no input named `nonsense`"), "{log}");
    assert!(log.contains("audience, document"), "{log}");
}

#[test]
fn an_edited_prompt_is_a_loud_cassette_mismatch_in_the_generated_program() {
    // The digest is recomputed in Python from the same fields the recorder used.
    // If that computation had drifted, replay would quietly serve a stale answer
    // — which would make every differential test above meaningless.
    let Some(python) = python() else { return };

    let work = TempDir::new("diff-digest");
    let build = work.path().join("build");
    let cassette = summarizer_cassette();
    let built = build_python(&example("document-summarizer"), &build, &[]);
    assert_eq!(code(&built), EXIT_OK, "{}", stderr(&built));

    let refused = run_generated(
        &python,
        &only_program(&build),
        &[
            "--cassette",
            cassette.as_str(),
            "--input",
            "audience=engineering leads",
            "--input",
            "document=not what was recorded",
        ],
        &[],
    );
    let log = String::from_utf8_lossy(&refused.stderr);
    assert!(!refused.status.success(), "{log}");
    assert!(log.contains("re-record the cassette"), "{log}");
}

// --- the report ---------------------------------------------------------------

#[test]
fn an_agent_using_an_unimplemented_construct_is_refused_by_default() {
    // `repo-digest` uses tools, which this target does not implement yet. The
    // build has to fail rather than emit a program that silently does nothing at
    // the call.
    let work = TempDir::new("report-refused");
    let built = build_python(&example("repo-digest"), work.path(), &[]);

    assert_ne!(code(&built), EXIT_OK);
    let log = stderr(&built);
    assert!(log.contains("tool.call"), "{log}");
    assert!(log.contains("not implemented"), "{log}");
    assert!(log.contains("--allow-unimplemented"), "{log}");
    assert!(
        std::fs::read_dir(work.path())
            .map(|entries| !entries.filter_map(Result::ok).any(|entry| entry
                .path()
                .extension()
                .map(|ext| ext == "py")
                .unwrap_or(false)))
            .unwrap_or(true),
        "a refused build must write no program"
    );
}

#[test]
fn allowing_the_unimplemented_still_refuses_to_emit_a_program_with_a_hole_in_it() {
    // The flag says "build one that will not do it", and the honest reading of
    // that for a `tool.call` is still a failure: there is no Python to emit for a
    // node whose whole purpose is the call.
    let work = TempDir::new("report-allowed");
    let built = build_python(
        &example("repo-digest"),
        work.path(),
        &["--allow-unimplemented"],
    );

    assert_ne!(code(&built), EXIT_OK);
    assert!(
        stderr(&built).contains("silently skip"),
        "{}",
        stderr(&built)
    );
}

#[test]
fn the_json_report_is_usable_as_a_deployment_gate() {
    let work = TempDir::new("report-json");
    let built = build_python(
        &example("repo-digest"),
        work.path(),
        &["--json", "--allow-unimplemented"],
    );

    let payload: serde_json::Value =
        serde_json::from_str(&stdout(&built)).expect("--json must print only JSON");
    assert_eq!(payload["target"], "python");
    assert_eq!(payload["buildable"], false);
    let unimplemented: Vec<&str> = payload["unimplemented"]
        .as_array()
        .expect("a list")
        .iter()
        .map(|value| value.as_str().unwrap())
        .collect();
    assert!(unimplemented.contains(&"tool.call"), "{unimplemented:?}");

    // And the clean case is an empty list, which is what `jq -e` tests.
    let clean = TempDir::new("report-json-clean");
    let built = build_python(&example("document-summarizer"), clean.path(), &["--json"]);
    assert_eq!(code(&built), EXIT_OK, "{}", stderr(&built));
    let payload: serde_json::Value = serde_json::from_str(&stdout(&built)).expect("JSON");
    assert_eq!(payload["buildable"], true);
    assert_eq!(payload["unimplemented"].as_array().unwrap().len(), 0);
}

#[test]
fn the_report_flags_belong_to_a_target_that_has_something_to_report() {
    // `ir` is what every backend reads, so nothing can fail to express it and a
    // report would be a page of nothing.
    for flag in ["--json", "--allow-unimplemented"] {
        let output = run_env(&["build", &example("document-summarizer"), flag], &[]);
        assert_ne!(code(&output), EXIT_OK, "{flag}");
        assert!(
            stderr(&output).contains("--target python"),
            "{}",
            stderr(&output)
        );
    }
}

// --- hygiene ------------------------------------------------------------------

#[test]
fn the_generated_program_is_the_same_bytes_every_build() {
    // Build output that changes between runs cannot be reviewed, and cannot be
    // compared across platforms to catch an encoding difference.
    let work = TempDir::new("diff-stable");
    let first_dir = work.path().join("a");
    let second_dir = work.path().join("b");

    let built = build_python(&example("document-summarizer"), &first_dir, &[]);
    assert_eq!(code(&built), EXIT_OK, "{}", stderr(&built));
    let built = build_python(&example("document-summarizer"), &second_dir, &[]);
    assert_eq!(code(&built), EXIT_OK, "{}", stderr(&built));

    let first = std::fs::read(only_program(&first_dir)).unwrap();
    let second = std::fs::read(only_program(&second_dir)).unwrap();
    assert_eq!(first, second);
}

#[test]
fn the_generated_program_carries_no_credential() {
    // Build output may be committed. A build that could embed a key would make
    // that dangerous, so there must be no path from the environment into it.
    let work = TempDir::new("diff-secrets");
    let out_dir = work.path().display().to_string();
    let output = run_env(
        &[
            "build",
            &example("document-summarizer"),
            "--target",
            "python",
            "--out-dir",
            &out_dir,
        ],
        &[("ANTHROPIC_API_KEY", "sk-live-must-not-be-embedded")],
    );
    assert_eq!(code(&output), EXIT_OK, "{}", stderr(&output));

    let source = std::fs::read_to_string(only_program(work.path())).unwrap();
    assert!(!source.contains("sk-live-must-not-be-embedded"), "{source}");
    // It reads the variable by name at run time, which is the only correct way
    // for a credential to reach a provider.
    assert!(source.contains("ANTHROPIC_API_KEY"), "{source}");
    assert!(source.contains("os.environ.get"), "{source}");
}

#[test]
fn the_generated_program_is_valid_python_before_it_is_run() {
    // `py_compile` parses without executing, so a syntax error in the emitter is
    // caught even for an agent no test runs.
    let Some(python) = python() else { return };

    let work = TempDir::new("diff-syntax");
    let built = build_python(&example("document-summarizer"), work.path(), &[]);
    assert_eq!(code(&built), EXIT_OK, "{}", stderr(&built));

    let program = only_program(work.path());
    let checked = Command::new(&python)
        .args([
            "-c",
            "import py_compile,sys; py_compile.compile(sys.argv[1], doraise=True)",
        ])
        .arg(&program)
        .output()
        .expect("python must be runnable");
    assert!(
        checked.status.success(),
        "{}",
        String::from_utf8_lossy(&checked.stderr)
    );
}
