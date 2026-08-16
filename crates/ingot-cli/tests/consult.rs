//! End-to-end tests for `consult` — a question put to a person.
//!
//! The property under test is not "a person can be asked". It is that **a run
//! with a person in it is as reproducible as one without**: recorded like a
//! model call, replayed like one, and refused loudly when the recording no
//! longer matches what is being asked.
//!
//! See [RFC-0020](../../../rfcs/0020-a-person-in-the-loop.md) and
//! [GAP-042](../../../docs/gaps.md#gap-042).

mod support;

use std::path::Path;

use serde_json::Value;
use support::{
    code, run_conversing, run_env, stderr, stdout, stub_provider, text_reply, TempDir, EXIT_OK,
};

/// An agent that asks a person how to frame a report, then writes it that way.
///
/// The answer is a value the flow reads — which is the whole of GAP-042, and
/// what an approval gate could never do.
fn source(question: &str, choices: &str) -> String {
    format!(
        r#"language 0.3

/// Asks a person how to frame a report, then writes it that way.
agent Framing(topic: string) -> report<markdown> {{
  model requires {{
    structured_output
  }}

  budget {{
    steps <= 6
    tokens <= 20000
  }}

  policy {{
    human allow
    network deny
  }}

  flow {{
    framing = consult("{question}", choices: [{choices}])
    emit report = ask<markdown>("Write about ${{topic}} as ${{framing}}.")
  }}
}}
"#
    )
}

const QUESTION: &str = "Which framing should the report take?";
const CHOICES: &str = r#""technical", "executive""#;

fn project(tag: &str) -> TempDir {
    let dir = TempDir::new(tag);
    write_source(dir.path(), &source(QUESTION, CHOICES));
    std::fs::write(
        dir.path().join("ingot.toml"),
        "[project]\nname = \"consult\"\n",
    )
    .expect("writing the manifest");
    dir
}

fn write_source(dir: &Path, text: &str) {
    std::fs::write(dir.join("main.ing"), text).expect("writing the source");
}

fn stub_env(url: &str) -> Vec<(&str, &str)> {
    vec![
        ("ANTHROPIC_API_KEY", "stub-key"),
        ("INGOT_ANTHROPIC_BASE_URL", url),
    ]
}

fn live_args(dir: &Path) -> Vec<String> {
    vec![
        "run".to_string(),
        dir.display().to_string(),
        "--input".to_string(),
        "topic=the harbour".to_string(),
        "--events".to_string(),
        "json".to_string(),
        "--approvals".to_string(),
        "stdin".to_string(),
    ]
}

fn as_args(owned: &[String]) -> Vec<&str> {
    owned.iter().map(String::as_str).collect()
}

/// Answer every question with `answer`, and refuse to answer a gate.
fn answering(answer: &'static str) -> impl Fn(&Value, usize) -> Option<String> + Send + 'static {
    move |event, _| {
        let node = event
            .get("node")
            .and_then(Value::as_str)
            .unwrap_or_default();
        Some(format!(r#"{{"node":"{node}","answer":"{answer}"}}"#))
    }
}

#[test]
fn a_person_is_asked_and_the_answer_becomes_a_value_the_flow_reads() {
    let dir = project("consult-live");
    let stub = stub_provider(vec![text_reply("# The harbour, for an executive\n")]);
    let owned = live_args(dir.path());

    let output = run_conversing(
        &as_args(&owned),
        &stub_env(&stub.url),
        answering("executive"),
    );

    assert_eq!(code(&output), EXIT_OK, "{}", stderr(&output));
    let log = stderr(&output);
    assert!(
        log.contains(r#""event":"consultationAsked""#),
        "the question has to leave before the run blocks on it: {log}"
    );
    assert!(
        log.contains(r#""answer":"executive""#),
        "the answer belongs in the record: {log}"
    );
    assert!(stdout(&output).contains("executive"), "{}", stdout(&output));
}

#[test]
fn an_answer_that_is_not_one_of_the_choices_is_refused() {
    // The program limited what may come back, so the runtime holds the limit.
    // A channel is not a trusted source just because a person is behind it.
    let dir = project("consult-not-a-choice");
    let stub = stub_provider(vec![text_reply("# Never written\n")]);
    let owned = live_args(dir.path());

    let output = run_conversing(
        &as_args(&owned),
        &stub_env(&stub.url),
        answering("whatever"),
    );

    assert_ne!(code(&output), EXIT_OK);
    let log = stderr(&output);
    assert!(log.contains("not one of the choices"), "{log}");
}

#[test]
fn a_run_with_no_channel_refuses_at_the_question_rather_than_waiting() {
    let dir = project("consult-no-channel");
    let stub = stub_provider(vec![text_reply("# Never written\n")]);

    let output = run_env(
        &[
            "run",
            &dir.path().display().to_string(),
            "--input",
            "topic=the harbour",
            "--events",
            "json",
        ],
        &stub_env(&stub.url),
    );

    assert_ne!(code(&output), EXIT_OK);
    let log = stderr(&output);
    assert!(log.contains("no channel to a person"), "{log}");
    assert!(
        log.contains(QUESTION),
        "the refusal names the question: {log}"
    );
}

#[test]
fn yes_approves_a_gate_and_cannot_answer_a_question() {
    // The asymmetry is the difference between a decision with a known safe side
    // and one without. Guessing would put a value nobody chose into the flow
    // and into the recording.
    let dir = project("consult-yes");
    let stub = stub_provider(vec![text_reply("# Never written\n")]);

    let output = run_env(
        &[
            "run",
            &dir.path().display().to_string(),
            "--input",
            "topic=the harbour",
            "--events",
            "json",
            "--yes",
        ],
        &stub_env(&stub.url),
    );

    assert_ne!(code(&output), EXIT_OK);
    let log = stderr(&output);
    assert!(log.contains("cannot answer a question"), "{log}");
}

// --- recording and replay ---------------------------------------------------

/// Record a run, answering its question, and return the cassette path.
fn record(dir: &Path, answer: &'static str, reply: &str) -> std::path::PathBuf {
    let cassette = dir.join("recorded.json");
    let stub = stub_provider(vec![text_reply(reply)]);
    let mut owned = live_args(dir);
    owned.push("--record".to_string());
    owned.push(cassette.display().to_string());

    let output = run_conversing(&as_args(&owned), &stub_env(&stub.url), answering(answer));
    assert_eq!(code(&output), EXIT_OK, "{}", stderr(&output));
    cassette
}

/// Replay with **no channel at all** — the point of a recording.
fn replay(dir: &Path, cassette: &Path) -> std::process::Output {
    run_env(
        &[
            "run",
            &dir.display().to_string(),
            "--input",
            "topic=the harbour",
            "--events",
            "json",
            "--provider",
            "replay",
            "--cassette",
            &cassette.display().to_string(),
        ],
        &[],
    )
}

#[test]
fn a_recorded_answer_replays_with_nobody_to_ask() {
    let dir = project("consult-replay");
    let cassette = record(dir.path(), "executive", "# The harbour, for an executive\n");

    let output = replay(dir.path(), &cassette);

    assert_eq!(code(&output), EXIT_OK, "{}", stderr(&output));
    assert!(stdout(&output).contains("executive"), "{}", stdout(&output));
    // Nobody was asked: there is no channel on this run at all, and the answer
    // still reached the flow.
    assert!(
        stderr(&output).contains(r#""answer":"executive""#),
        "{}",
        stderr(&output)
    );
}

#[test]
fn the_recording_keeps_a_persons_answer_in_its_own_list() {
    // The single most important thing to know about a recorded run is which
    // answers a machine produced and which a person did. One list could not
    // say that, which is why there are three.
    let dir = project("consult-shape");
    let cassette = record(dir.path(), "technical", "# The harbour, technically\n");

    let written: Value =
        serde_json::from_str(&std::fs::read_to_string(&cassette).expect("a cassette"))
            .expect("canonical json");

    assert_eq!(written["cassetteVersion"], "0.3");
    let consultations = written["consultations"]
        .as_array()
        .expect("a consultations list");
    assert_eq!(consultations.len(), 1, "{written:#}");
    assert_eq!(consultations[0]["answer"], "technical");
    // The question is recorded beside the digest, so a reviewer can see what was
    // asked without running anything.
    assert_eq!(consultations[0]["question"], QUESTION);
    assert_eq!(consultations[0]["choices"][1], "executive");
    assert!(
        consultations[0]["questionDigest"]
            .as_str()
            .map(|digest| digest.len() == 64)
            .unwrap_or(false),
        "{written:#}"
    );
    // And it is a separate list from the model's own answers.
    assert_eq!(
        written["interactions"]
            .as_array()
            .expect("interactions")
            .len(),
        1
    );
}

#[test]
fn a_replay_whose_question_changed_is_refused() {
    let dir = project("consult-changed-question");
    let cassette = record(dir.path(), "executive", "# The harbour\n");

    write_source(
        dir.path(),
        &source("Which framing do you prefer, on reflection?", CHOICES),
    );
    let output = replay(dir.path(), &cassette);

    assert_ne!(code(&output), EXIT_OK);
    let log = stderr(&output);
    assert!(log.contains("recorded for a different question"), "{log}");
    assert!(log.contains("re-record"), "{log}");
    // The cost of re-recording this one is a person's time, and the message says
    // so rather than implying it is free.
    assert!(log.contains("asking somebody again"), "{log}");
}

#[test]
fn a_replay_whose_choices_changed_is_refused() {
    // The choices are part of what determined the answer: somebody picking
    // "executive" from two options did not pick it from three.
    let dir = project("consult-changed-choices");
    let cassette = record(dir.path(), "executive", "# The harbour\n");

    write_source(
        dir.path(),
        &source(QUESTION, r#""technical", "executive", "narrative""#),
    );
    let output = replay(dir.path(), &cassette);

    assert_ne!(code(&output), EXIT_OK);
    assert!(
        stderr(&output).contains("recorded for a different question"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn a_replay_that_runs_out_of_recorded_answers_says_so() {
    let dir = project("consult-exhausted");
    let cassette = record(dir.path(), "executive", "# The harbour\n");

    // Two questions now, and the recording holds one.
    write_source(
        dir.path(),
        &source(QUESTION, CHOICES).replace(
            "    emit report = ask<markdown>",
            "    second = consult(\"And how long should it be?\", choices: [\"short\", \"long\"])\n    emit report = ask<markdown>",
        ),
    );
    let output = replay(dir.path(), &cassette);

    assert_ne!(code(&output), EXIT_OK);
    assert!(
        stderr(&output).contains("asked for another"),
        "{}",
        stderr(&output)
    );
}
