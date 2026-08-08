//! End-to-end tests for `ingot run` and `ingot test`.
//!
//! A stub HTTP server stands in for the provider, so these exercise the whole
//! chain — compile, lower, execute, record, replay — over the real HTTP path,
//! with no API key and no network.

mod support;

use std::sync::atomic::Ordering;

use serde_json::Value;
use support::{
    code, openai_reply, repo_root, run, run_env, stderr, stdout, stub_provider, text_reply,
    TempDir, EXIT_DIAGNOSTICS, EXIT_OK,
};

/// An agent that pins a vendor, so the run has to choose a provider from the
/// artifact rather than from a flag.
fn pinned_agent(reference: &str) -> String {
    format!(
        r#"language 0.1

agent Pinned(topic: string) -> brief<markdown> {{
  model exact "{reference}"

  budget {{
    steps <= 2
    tokens <= 20000
  }}

  policy {{
    network deny
  }}

  flow {{
    emit brief = ask<markdown>("Write about ${{topic}}.")
  }}
}}
"#
    )
}

fn pinned_project(tag: &str, reference: &str) -> TempDir {
    project_with(tag, reference, "")
}

fn project_with(tag: &str, reference: &str, extra_manifest: &str) -> TempDir {
    let dir = TempDir::new(tag);
    std::fs::write(dir.path().join("main.ing"), pinned_agent(reference)).unwrap();
    std::fs::write(
        dir.path().join("ingot.toml"),
        format!("[project]\nname = \"pinned\"\n{extra_manifest}"),
    )
    .unwrap();
    dir
}

#[test]
fn an_operator_can_declare_their_own_model_service() {
    // The whole point: somebody running Ollama, vLLM or llama.cpp names it,
    // pins it in the source, and needs no key and no vendor account.
    let stub = stub_provider(vec![openai_reply("# From my own server")]);
    let project = project_with(
        "own-llm",
        "local/llama-test",
        &format!(
            "\n[[model.provider]]\nname = \"local\"\nkind = \"openai\"\nbase-url = \"{}\"\n",
            stub.url
        ),
    );

    let output = run_env(
        &[
            "run",
            &project.path().display().to_string(),
            "--input",
            "topic=compilers",
            "--events",
            "quiet",
        ],
        &[],
    );

    assert_eq!(code(&output), EXIT_OK, "{}", stderr(&output));
    assert_eq!(stdout(&output).trim(), "# From my own server");
    assert!(
        stderr(&output).contains("model calls go to local"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn a_declared_provider_can_take_the_place_of_a_built_in_name() {
    // Pointing the familiar name somewhere else — a company gateway that
    // fronts OpenAI, say — without every artifact having to be edited.
    let stub = stub_provider(vec![openai_reply("# Through the gateway")]);
    let project = project_with(
        "override-openai",
        "openai/gpt-test",
        &format!(
            "\n[[model.provider]]\nname = \"openai\"\nkind = \"openai\"\nbase-url = \"{}\"\n",
            stub.url
        ),
    );

    // A real key is exported and must be ignored: the declaration wins, and it
    // asks for no key at all.
    let output = run_env(
        &[
            "run",
            &project.path().display().to_string(),
            "--input",
            "topic=compilers",
            "--events",
            "quiet",
        ],
        &[("OPENAI_API_KEY", "should-not-be-used")],
    );

    assert_eq!(code(&output), EXIT_OK, "{}", stderr(&output));
    assert_eq!(stdout(&output).trim(), "# Through the gateway");
}

#[test]
fn a_declaration_naming_a_key_variable_that_is_not_set_stops_the_run() {
    // A declared provider is a stated intention, so a missing key is an error
    // rather than a provider that quietly is not there.
    let project = project_with(
        "declared-nokey",
        "local/llama-test",
        "\n[[model.provider]]\nname = \"local\"\nkind = \"openai\"\n\
         base-url = \"http://127.0.0.1:1/v1/chat/completions\"\n\
         api-key-env = \"A_KEY_NOBODY_EXPORTED\"\n",
    );

    let output = run_env(
        &[
            "run",
            &project.path().display().to_string(),
            "--input",
            "topic=compilers",
        ],
        &[],
    );

    assert_ne!(code(&output), EXIT_OK);
    assert!(
        stderr(&output).contains("A_KEY_NOBODY_EXPORTED"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn a_default_naming_no_provider_is_refused_before_anything_runs() {
    let project = project_with(
        "bad-default",
        "local/llama-test",
        "\n[model]\ndefault = \"typo\"\n\n[[model.provider]]\nname = \"local\"\n\
         kind = \"openai\"\nbase-url = \"http://127.0.0.1:1/v1/chat/completions\"\n",
    );

    let output = run_env(
        &[
            "run",
            &project.path().display().to_string(),
            "--input",
            "topic=compilers",
        ],
        &[],
    );

    assert_ne!(code(&output), EXIT_OK);
    let message = stderr(&output);
    assert!(message.contains("typo"), "{message}");
    assert!(message.contains("local"), "{message}");
}

#[test]
fn an_artifact_that_pins_openai_reaches_openai() {
    // The point of pinning: the source names the vendor, and the run honours it
    // without the operator repeating it on the command line.
    let project = pinned_project("pin-openai", "openai/gpt-test");
    let stub = stub_provider(vec![openai_reply("# From OpenAI")]);

    let output = run_env(
        &[
            "run",
            &project.path().display().to_string(),
            "--input",
            "topic=compilers",
            "--events",
            "quiet",
        ],
        &[
            ("OPENAI_API_KEY", "stub-key"),
            ("INGOT_OPENAI_BASE_URL", &stub.url),
        ],
    );

    assert_eq!(code(&output), EXIT_OK, "{}", stderr(&output));
    assert_eq!(stdout(&output).trim(), "# From OpenAI");
    assert!(
        stderr(&output).contains("model calls go to openai"),
        "the run must say which service answered: {}",
        stderr(&output)
    );
}

#[test]
fn an_artifact_pinning_a_vendor_with_no_key_is_refused_rather_than_redirected() {
    // An artifact that says `openai/…` must never be answered by Anthropic and
    // come back with a plausible answer from the wrong model.
    let project = pinned_project("pin-unavailable", "openai/gpt-test");
    let stub = stub_provider(vec![text_reply("# From the wrong vendor")]);

    let output = run_env(
        &[
            "run",
            &project.path().display().to_string(),
            "--input",
            "topic=compilers",
            "--events",
            "quiet",
        ],
        &[
            ("ANTHROPIC_API_KEY", "stub-key"),
            ("INGOT_ANTHROPIC_BASE_URL", &stub.url),
        ],
    );

    assert_ne!(code(&output), EXIT_OK);
    let message = stderr(&output);
    assert!(message.contains("openai"), "{message}");
    assert!(message.contains("no provider"), "{message}");
}

#[test]
fn with_no_key_at_all_the_run_says_what_to_export() {
    let project = pinned_project("pin-nokey", "openai/gpt-test");
    let output = run_env(
        &[
            "run",
            &project.path().display().to_string(),
            "--input",
            "topic=compilers",
        ],
        &[],
    );

    assert_ne!(code(&output), EXIT_OK);
    let message = stderr(&output);
    assert!(message.contains("OPENAI_API_KEY"), "{message}");
    assert!(message.contains("--provider replay"), "{message}");
}

fn summarizer() -> String {
    repo_root()
        .join("examples/document-summarizer")
        .display()
        .to_string()
}

#[test]
fn run_executes_an_agent_against_a_provider_and_prints_the_artifact() {
    let stub = stub_provider(vec![text_reply("# Summary\n\nThe document is short.")]);
    let output = run(
        &[
            "run",
            &summarizer(),
            "--input",
            "document=A short document about compilers.",
            "--input",
            "audience=engineers",
            "--events",
            "quiet",
        ],
        Some(&stub.url),
    );

    assert_eq!(code(&output), EXIT_OK, "{}", stderr(&output));
    assert!(stdout(&output).contains("# Summary"), "{}", stdout(&output));
    assert_eq!(
        stub.served.load(Ordering::SeqCst),
        1,
        "exactly one model call expected"
    );
}

#[test]
fn run_writes_artifacts_with_the_right_extension() {
    let dir = TempDir::new("out");
    let stub = stub_provider(vec![text_reply("# Written to disk")]);
    let output = run(
        &[
            "run",
            &summarizer(),
            "--input",
            "document=text",
            "--input",
            "audience=all",
            "--out-dir",
            &dir.path().display().to_string(),
            "--events",
            "quiet",
        ],
        Some(&stub.url),
    );

    assert_eq!(code(&output), EXIT_OK, "{}", stderr(&output));
    let path = dir.path().join("summary.md");
    assert!(path.is_file(), "expected {}", path.display());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "# Written to disk");
}

#[test]
fn a_missing_input_is_reported_before_any_provider_call() {
    let stub = stub_provider(vec![text_reply("never used")]);
    let output = run(
        &["run", &summarizer(), "--events", "quiet"],
        Some(&stub.url),
    );

    assert_eq!(code(&output), EXIT_DIAGNOSTICS);
    assert!(
        stderr(&output).contains("missing input"),
        "{}",
        stderr(&output)
    );
    assert_eq!(
        stub.served.load(Ordering::SeqCst),
        0,
        "nothing should be sent"
    );
}

#[test]
fn an_input_file_can_be_read_with_an_at_prefix() {
    let dir = TempDir::new("input-file");
    let doc = dir.path().join("document.txt");
    std::fs::write(&doc, "Contents loaded from a file.").unwrap();

    let stub = stub_provider(vec![text_reply("# Read it")]);
    let output = run(
        &[
            "run",
            &summarizer(),
            "--input",
            &format!("document=@{}", doc.display()),
            "--input",
            "audience=readers",
            "--events",
            "quiet",
        ],
        Some(&stub.url),
    );
    assert_eq!(code(&output), EXIT_OK, "{}", stderr(&output));
}

#[test]
fn the_event_stream_can_be_emitted_as_json_lines() {
    let stub = stub_provider(vec![text_reply("# Events")]);
    let output = run(
        &[
            "run",
            &summarizer(),
            "--input",
            "document=d",
            "--input",
            "audience=a",
            "--events",
            "json",
        ],
        Some(&stub.url),
    );
    assert_eq!(code(&output), EXIT_OK, "{}", stderr(&output));

    let events: Vec<Value> = stderr(&output)
        .lines()
        .filter(|line| line.starts_with('{'))
        .map(|line| serde_json::from_str(line).expect("each event line must be JSON"))
        .collect();
    assert!(events.iter().any(|e| e["event"] == "runStarted"));
    assert!(events.iter().any(|e| e["event"] == "modelCall"));
    assert!(events.iter().any(|e| e["event"] == "emitted"));
    assert!(events.iter().any(|e| e["event"] == "runFinished"));
}

#[test]
fn a_recorded_cassette_replays_without_a_provider() {
    let dir = TempDir::new("record");
    let cassette = dir.path().join("summarize.json");
    let stub = stub_provider(vec![text_reply("# Recorded once")]);

    let record = run(
        &[
            "run",
            &summarizer(),
            "--input",
            "document=the source",
            "--input",
            "audience=everyone",
            "--record",
            &cassette.display().to_string(),
            "--events",
            "quiet",
        ],
        Some(&stub.url),
    );
    assert_eq!(code(&record), EXIT_OK, "{}", stderr(&record));
    assert!(cassette.is_file());

    // Replay with no key and no server at all.
    let replay = run(
        &[
            "run",
            &summarizer(),
            "--input",
            "document=the source",
            "--input",
            "audience=everyone",
            "--provider",
            "replay",
            "--cassette",
            &cassette.display().to_string(),
            "--events",
            "quiet",
        ],
        None,
    );
    assert_eq!(code(&replay), EXIT_OK, "{}", stderr(&replay));
    assert_eq!(stdout(&replay).trim(), "# Recorded once");
}

#[test]
fn replaying_with_different_inputs_fails_loudly() {
    let dir = TempDir::new("mismatch");
    let cassette = dir.path().join("summarize.json");
    let stub = stub_provider(vec![text_reply("# Recorded")]);

    let record = run(
        &[
            "run",
            &summarizer(),
            "--input",
            "document=the original",
            "--input",
            "audience=everyone",
            "--record",
            &cassette.display().to_string(),
            "--events",
            "quiet",
        ],
        Some(&stub.url),
    );
    assert_eq!(code(&record), EXIT_OK, "{}", stderr(&record));

    let replay = run(
        &[
            "run",
            &summarizer(),
            "--input",
            "document=something else entirely",
            "--input",
            "audience=everyone",
            "--provider",
            "replay",
            "--cassette",
            &cassette.display().to_string(),
            "--events",
            "quiet",
        ],
        None,
    );
    assert_eq!(code(&replay), EXIT_DIAGNOSTICS);
    assert!(stderr(&replay).contains("re-record"), "{}", stderr(&replay));
}

#[test]
fn a_tool_using_agent_stops_because_no_host_provides_the_tool() {
    // Honest failure while MCP is unimplemented: the artifact needs a tool, no
    // host offers one, and the run says exactly that instead of pretending.
    //
    // The first node is `queries = ask<string[]>(...)`, so the stub has to
    // answer with a schema-shaped value for execution to reach the tool call.
    let stub = stub_provider(vec![text_reply(r#"{"value":["one","two"]}"#)]);
    let path = repo_root()
        .join("examples/research-agent")
        .display()
        .to_string();
    let output = run(
        &[
            "run",
            &path,
            "--input",
            "topic=compilers",
            "--events",
            "quiet",
        ],
        Some(&stub.url),
    );

    assert_eq!(code(&output), EXIT_DIAGNOSTICS);
    let message = stderr(&output);
    assert!(message.contains("web.search"), "{message}");
    assert!(message.contains("no host provides"), "{message}");
}

#[test]
fn replay_without_a_cassette_explains_how_to_make_one() {
    let output = run(
        &[
            "run",
            &summarizer(),
            "--provider",
            "replay",
            "--input",
            "document=d",
            "--input",
            "audience=a",
        ],
        None,
    );
    assert_ne!(code(&output), EXIT_OK);
    assert!(stderr(&output).contains("--record"), "{}", stderr(&output));
}

#[test]
fn ingot_test_replays_the_checked_in_cassettes() {
    let path = repo_root()
        .join("examples/document-summarizer")
        .display()
        .to_string();
    let output = run(&["test", &path], None);
    assert_eq!(code(&output), EXIT_OK, "{}", stderr(&output));
    assert!(stdout(&output).contains("passed"), "{}", stdout(&output));
}

#[test]
fn ingot_test_reports_no_cassettes_rather_than_failing() {
    // `ingot init` now deliberately includes an offline starter cassette. Use
    // a hand-written project to preserve the separate contract that projects
    // with no fixture still report an empty suite rather than failing.
    let project = project_with("no-cassettes", "anthropic/claude-opus-5", "");

    let output = run(&["test", &project.path().display().to_string()], None);
    assert_eq!(code(&output), EXIT_OK);
    assert!(
        stderr(&output).contains("nothing to test"),
        "{}",
        stderr(&output)
    );
}
