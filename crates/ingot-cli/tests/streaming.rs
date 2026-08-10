//! What an operator sees while a model is still answering.
//!
//! Two channels leave a run, and these tests exist to keep them apart. The
//! event stream is the record: ordered, timestamp-free, and reproduced byte for
//! byte by a replay, which is what makes it assertable. The live text is not a
//! record of anything — it is how an answer happened to arrive over one
//! connection — so it must never appear in the stream a replay has to
//! reproduce.
//!
//! See [RFC-0013](../../../rfcs/0013-streaming.md) and Runtime 0.3.

mod support;

use std::path::Path;

use support::*;

/// The smallest artifact that makes one model call.
fn project(dir: &Path) {
    std::fs::write(
        dir.join("main.ing"),
        "language 0.1\n\
         agent Note(topic: string) -> note<markdown> {\n\
         \x20 model requires { structured_output }\n\
         \x20 budget { steps <= 2 }\n\
         \x20 policy { network deny }\n\
         \x20 flow {\n\
         \x20   emit note = ask<markdown>(\"One line about ${topic}.\")\n\
         \x20 }\n\
         }\n",
    )
    .expect("writing the source");
    std::fs::write(
        dir.join("ingot.toml"),
        "[project]\nname = \"note\"\nversion = \"0.1.0\"\n\n\
         [build]\nentry = \"main.ing\"\nout-dir = \"target/ingot\"\n",
    )
    .expect("writing the manifest");
}

fn note(dir: &Path, events: &str, url: &str) -> std::process::Output {
    run_env(
        &[
            "run",
            &dir.display().to_string(),
            "--provider",
            "anthropic",
            "--events",
            events,
            "--input",
            "topic=compilers",
        ],
        &[
            ("ANTHROPIC_API_KEY", "stub-key"),
            ("INGOT_ANTHROPIC_BASE_URL", url),
        ],
    )
}

#[test]
fn the_answer_appears_while_it_is_still_being_written() {
    let dir = TempDir::new("stream-text");
    project(dir.path());
    let stub = stub_provider(vec![text_reply(
        "# Compilers\n\nA compiler turns one language into another.",
    )]);

    let output = note(dir.path(), "text", &stub.url);
    assert_eq!(code(&output), EXIT_OK, "{}", stderr(&output));

    // The text reaches the operator on the trace stream, before the line that
    // reports the finished call.
    let log = stderr(&output);
    let text_at = log
        .find("A compiler turns one language into another.")
        .unwrap_or_else(|| panic!("the answer never appeared while it was arriving:\n{log}"));
    let call_at = log
        .find("model.call")
        .unwrap_or_else(|| panic!("the model call was never reported:\n{log}"));
    assert!(
        text_at < call_at,
        "the text arrived after the call was reported, which is not live:\n{log}"
    );

    // And stdout still carries only the artifact. A pipeline reading a run's
    // output must not have half-finished text spliced into it.
    assert_eq!(
        stdout(&output)
            .matches("A compiler turns one language")
            .count(),
        1,
        "{}",
        stdout(&output)
    );
}

#[test]
fn the_json_event_stream_carries_no_fragments() {
    let dir = TempDir::new("stream-json");
    project(dir.path());
    let stub = stub_provider(vec![text_reply("# Compilers\n\nShort.")]);

    let output = note(dir.path(), "json", &stub.url);
    assert_eq!(code(&output), EXIT_OK, "{}", stderr(&output));

    let log = stderr(&output);
    let mut events = 0;
    let mut fragments = 0;
    for line in log.lines() {
        let Ok(parsed) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if parsed.get("event").is_some() {
            events += 1;
            assert!(
                !line.contains("Short."),
                "a fragment reached the event stream: {line}"
            );
        } else if parsed.get("delta").is_some() || parsed.get("settled").is_some() {
            fragments += 1;
        }
    }

    // The contract the UI reads by: an event is a line with an `event` key, and
    // everything else on the stream is live text that a replay will not produce.
    assert!(events > 0, "no events at all:\n{log}");
    assert!(fragments > 0, "nothing arrived live:\n{log}");
    assert!(
        log.contains(r#""settled":{"kept":true"#),
        "the watcher was never told the text became the answer:\n{log}"
    );
}

#[test]
fn a_replay_produces_the_same_events_and_no_live_text() {
    let dir = TempDir::new("stream-replay");
    project(dir.path());
    let cassette = dir.path().join("note.json");
    let stub = stub_provider(vec![text_reply("# Compilers\n\nShort.")]);

    let recorded = run_env(
        &[
            "run",
            &dir.path().display().to_string(),
            "--provider",
            "anthropic",
            "--events",
            "json",
            "--record",
            &cassette.display().to_string(),
            "--input",
            "topic=compilers",
        ],
        &[
            ("ANTHROPIC_API_KEY", "stub-key"),
            ("INGOT_ANTHROPIC_BASE_URL", &stub.url),
        ],
    );
    assert_eq!(code(&recorded), EXIT_OK, "{}", stderr(&recorded));

    let replayed = run_env(
        &[
            "run",
            &dir.path().display().to_string(),
            "--provider",
            "replay",
            "--cassette",
            &cassette.display().to_string(),
            "--events",
            "json",
            "--input",
            "topic=compilers",
        ],
        &[],
    );
    assert_eq!(code(&replayed), EXIT_OK, "{}", stderr(&replayed));

    // `runStarted` is excluded because it names the provider that answered,
    // which is the one thing that genuinely differs between these two runs.
    // Everything after it must not.
    let events = |output: &std::process::Output| -> Vec<String> {
        stderr(output)
            .lines()
            .filter(|line| line.contains("\"event\":"))
            .filter(|line| !line.contains("runStarted"))
            .map(str::to_string)
            .collect()
    };

    // Byte for byte, per Runtime 0.1 §9 — which is the reason a fragment could
    // not simply have been added to the stream as a new event.
    assert_eq!(events(&recorded), events(&replayed));

    // A cassette produces its answer at once, so there is nothing live to show.
    // Correct rather than unfortunate: inventing fragments for a replay would
    // make it look like a call that never happened.
    let log = stderr(&replayed);
    assert!(
        !log.contains(r#"{"delta":"#),
        "a replay showed live text:\n{log}"
    );
}
