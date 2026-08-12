//! End-to-end tests for stopping at a checkpoint and continuing.
//!
//! RFC-0018 §4. The property that makes resumption more than a plausible story
//! needs three runs to state, so it lives here rather than in a unit test:
//! the events of the two halves, concatenated, must equal the events of one
//! uninterrupted run, byte for byte.

mod support;

use std::path::{Path, PathBuf};
use std::process::Output;

use support::{code, run, stderr, TempDir, EXIT_DIAGNOSTICS, EXIT_FAILURE, EXIT_OK};

/// An agent with a top-level checkpoint and a nested one.
///
/// It calls no model, so the runs need no provider and stay offline; a cassette
/// with no interactions is enough.
const SOURCE: &str = r#"language 0.2
package test.pause

agent Phases(topic: text) -> report<text> {
  memory { working ephemeral { first: text, second: text } }
  budget { steps <= 4 }
  policy { network deny }

  flow {
    state.first = topic
    checkpoint "half-way"
    if true {
      checkpoint "inside"
    }
    state.second = state.first
    emit report = state.second
  }
}
"#;

fn project(tag: &str) -> TempDir {
    let dir = TempDir::new(tag);
    std::fs::write(dir.path().join("main.ing"), SOURCE).expect("writing the agent");
    std::fs::write(
        dir.path().join("empty.json"),
        r#"{"cassetteVersion":"0.1","agent":"test.pause.Phases","interactions":[]}"#,
    )
    .expect("writing the cassette");
    dir
}

fn once(dir: &Path, extra: &[&str]) -> Output {
    let mut args: Vec<String> = vec![
        "run".into(),
        dir.join("main.ing").display().to_string(),
        "--provider".into(),
        "replay".into(),
        "--cassette".into(),
        dir.join("empty.json").display().to_string(),
        "--out-dir".into(),
        dir.join("out").display().to_string(),
        "--events".into(),
        "json".into(),
    ];
    args.extend(extra.iter().map(|argument| (*argument).to_string()));
    let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
    run(&borrowed, None)
}

/// The events of a run, with the four that frame it removed.
///
/// `runStarted` and its counterparts say where a *process* began and ended, and
/// an interrupted run has two of those where an uninterrupted one has one. What
/// has to match is everything in between.
fn node_events(output: &Output) -> Vec<String> {
    const FRAMING: [&str; 4] = ["runStarted", "runFinished", "runFailed", "runStopped"];
    stderr(output)
        .lines()
        .filter(|line| line.starts_with('{') && line.contains("\"event\""))
        .filter(|line| {
            !FRAMING
                .iter()
                .any(|name| line.contains(&format!("\"event\":\"{name}\"")))
        })
        .map(str::to_string)
        .collect()
}

fn snapshot(dir: &Path) -> PathBuf {
    dir.join("target")
        .join("ingot")
        .join("snapshots")
        .join("test.pause.Phases-half-way.json")
}

#[test]
fn the_two_halves_produce_exactly_the_events_one_run_would() {
    // RFC-0018 §4.4, as an executable assertion.
    let whole_dir = project("resume-whole");
    let whole = once(whole_dir.path(), &["--input", "topic=x"]);
    assert_eq!(code(&whole), EXIT_OK, "{}", stderr(&whole));

    let split_dir = project("resume-split");
    let first = once(
        split_dir.path(),
        &["--input", "topic=x", "--stop-at", "half-way"],
    );
    assert_eq!(code(&first), EXIT_OK, "{}", stderr(&first));

    let second = once(
        split_dir.path(),
        &[
            "--resume",
            &snapshot(split_dir.path()).display().to_string(),
        ],
    );
    assert_eq!(code(&second), EXIT_OK, "{}", stderr(&second));

    let mut halves = node_events(&first);
    halves.extend(node_events(&second));
    assert_eq!(
        halves,
        node_events(&whole),
        "the two halves did not reproduce the uninterrupted run"
    );
}

#[test]
fn a_stopped_run_says_it_stopped_and_produces_no_artifact() {
    // Without `runStopped`, a stopped run and a run that finished having
    // produced nothing look identical in a record.
    let dir = project("resume-stopped-event");
    let first = once(dir.path(), &["--input", "topic=x", "--stop-at", "half-way"]);
    assert_eq!(code(&first), EXIT_OK, "{}", stderr(&first));

    let text = stderr(&first);
    assert!(text.contains(r#""event":"runStopped""#), "{text}");
    assert!(!text.contains(r#""event":"runFinished""#), "{text}");
    // The declared output was not produced, and that is not an error here.
    assert!(!dir.path().join("out").join("report.txt").exists());
    assert!(snapshot(dir.path()).is_file());
}

#[test]
fn stopping_at_a_nested_checkpoint_is_refused_rather_than_ignored() {
    // A flag that silently does nothing is worse than one that refuses: a
    // caller who asked to stop and got a finished run cannot tell that from a
    // run with no checkpoint.
    let dir = project("resume-nested");
    let refused = once(dir.path(), &["--input", "topic=x", "--stop-at", "inside"]);
    assert_eq!(code(&refused), EXIT_DIAGNOSTICS, "{}", stderr(&refused));

    let text = stderr(&refused);
    assert!(text.contains("inside"), "{text}");
    assert!(text.contains("inside a branch or a loop"), "{text}");
    assert!(
        text.contains("half-way"),
        "the message offers what is available: {text}"
    );
}

#[test]
fn stopping_at_a_label_nobody_wrote_says_what_is_available() {
    let dir = project("resume-unknown");
    let refused = once(dir.path(), &["--input", "topic=x", "--stop-at", "nowhere"]);
    assert_eq!(code(&refused), EXIT_DIAGNOSTICS);
    let text = stderr(&refused);
    assert!(text.contains("no checkpoint is labelled"), "{text}");
    assert!(text.contains("half-way"), "{text}");
}

#[test]
fn resuming_against_a_changed_artifact_is_refused_with_no_override() {
    let dir = project("resume-changed");
    let first = once(dir.path(), &["--input", "topic=x", "--stop-at", "half-way"]);
    assert_eq!(code(&first), EXIT_OK, "{}", stderr(&first));

    // Any edit that changes the canonical IR is a different program.
    std::fs::write(
        dir.path().join("main.ing"),
        SOURCE.replace("steps <= 4", "steps <= 9"),
    )
    .expect("rewriting the agent");

    let refused = once(
        dir.path(),
        &["--resume", &snapshot(dir.path()).display().to_string()],
    );
    assert_eq!(code(&refused), EXIT_FAILURE, "{}", stderr(&refused));
    let text = stderr(&refused);
    assert!(text.contains("has changed since the run stopped"), "{text}");
    assert!(text.contains("start the run again"), "{text}");
}

#[test]
fn a_resumption_does_not_want_the_inputs_again() {
    let dir = project("resume-inputs");
    assert_eq!(
        code(&once(
            dir.path(),
            &["--input", "topic=x", "--stop-at", "half-way"]
        )),
        EXIT_OK
    );

    // The same input is harmless — it is what the snapshot already carries.
    let same = once(
        dir.path(),
        &[
            "--input",
            "topic=x",
            "--resume",
            &snapshot(dir.path()).display().to_string(),
        ],
    );
    assert_eq!(code(&same), EXIT_OK, "{}", stderr(&same));

    // A different one is not: the two halves would disagree about what the run
    // was given, and nothing downstream could tell which was used.
    assert_eq!(
        code(&once(
            dir.path(),
            &["--input", "topic=y", "--stop-at", "half-way"]
        )),
        EXIT_OK
    );
    let conflicting = once(
        dir.path(),
        &[
            "--input",
            "topic=z",
            "--resume",
            &snapshot(dir.path()).display().to_string(),
        ],
    );
    assert_eq!(
        code(&conflicting),
        EXIT_DIAGNOSTICS,
        "{}",
        stderr(&conflicting)
    );
    assert!(
        stderr(&conflicting).contains("already carries the inputs"),
        "{}",
        stderr(&conflicting)
    );
}

#[test]
fn a_memory_store_pointed_at_resume_says_which_file_it_is() {
    // Two snapshot kinds, and mixing them up is a plausible mistake.
    let dir = project("resume-wrong-kind");
    let store = dir.path().join("store.json");
    std::fs::write(
        &store,
        r#"{"ingotSnapshot":"0.1","kind":"memory","agent":"a","shape":{},"fields":{}}"#,
    )
    .expect("writing a memory store");

    let refused = once(dir.path(), &["--resume", &store.display().to_string()]);
    assert_eq!(code(&refused), EXIT_FAILURE, "{}", stderr(&refused));
    let text = stderr(&refused);
    assert!(text.contains("`memory` snapshot"), "{text}");
    assert!(text.contains("--memory"), "{text}");
}
