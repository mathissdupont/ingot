//! End-to-end tests for `ingot run` and `ingot test`.
//!
//! A stub HTTP server stands in for the provider, so these exercise the whole
//! chain — compile, lower, execute, record, replay — over the real HTTP path,
//! with no API key and no network.

mod support;

use std::sync::atomic::Ordering;

use serde_json::Value;
use support::{
    code, repo_root, run, stderr, stdout, stub_provider, text_reply, TempDir, EXIT_DIAGNOSTICS,
    EXIT_OK,
};

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
    let dir = TempDir::new("no-cassettes");
    let project = dir.path().join("agent");
    assert_eq!(
        code(&run(&["init", &project.display().to_string()], None)),
        EXIT_OK
    );

    let output = run(&["test", &project.display().to_string()], None);
    assert_eq!(code(&output), EXIT_OK);
    assert!(
        stderr(&output).contains("nothing to test"),
        "{}",
        stderr(&output)
    );
}
