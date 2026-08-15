//! End-to-end tests for the approval channel — `ingot run --approvals stdin`.
//!
//! The property under test is not "a gate can be answered". It is that **every
//! way of failing to answer one is a refusal**, because the single thing an
//! approval gate exists to prevent is an effect happening without a person.
//!
//! Each test drives the exchange the way a parent process will: watch the event
//! stream for `approvalRequested`, write one line back on standard input. The
//! node id is read off the stream rather than hard-coded, so these exercise the
//! whole round trip rather than a guess about lowering.
//!
//! See [RFC-0020](../../../rfcs/0020-a-person-in-the-loop.md) and
//! [GAP-041](../../../docs/gaps.md#gap-041).

mod support;

use std::path::Path;

use support::{
    code, fs_server, run_answering, run_env, stderr, stub_provider, text_reply, toml_string,
    TempDir, EXIT_OK,
};

/// An agent that writes one file, behind a gate the compiler inserts.
const GATED: &str = r#"language 0.1

/// Writes a UTF-8 text file into the workspace.
tool fs.write_file(path: string, content: text) -> file !filesystem_write

/// Writes one file, behind a gate a person has to open.
agent Gatekeeper(note: string) -> receipt<markdown> {
  model requires {
    structured_output
  }

  tools {
    mcp fs.write_file
  }

  budget {
    steps <= 6
    tokens <= 20000
  }

  policy {
    filesystem_write require approval
    network deny
    secrets deny export
  }

  flow {
    body = ask<markdown>("Write one line about ${note}.")
    _filed = call fs.write_file("out/note.md", body)
    emit receipt = body
  }
}
"#;

struct Project {
    dir: TempDir,
}

impl Project {
    fn new(tag: &str) -> Project {
        let dir = TempDir::new(tag);
        let root = dir.path();
        std::fs::create_dir_all(root.join("data")).expect("creating the workspace");
        std::fs::write(root.join("main.ing"), GATED).expect("writing the source");
        std::fs::write(
            root.join("ingot.toml"),
            format!(
                "[project]\nname = \"gate\"\n\n[mcp]\ntimeout-seconds = 10\n\n\
                 [[mcp.server]]\nname = \"workspace\"\ncommand = {}\nargs = [\"--root\", \"data\", \"--allow-write\"]\n",
                toml_string(&fs_server().display().to_string())
            ),
        )
        .expect("writing the manifest");
        Project { dir }
    }

    fn path(&self) -> String {
        self.dir.path().display().to_string()
    }

    /// Where the gated write lands, if it is ever allowed to.
    fn written(&self) -> &Path {
        self.dir.path()
    }
}

fn args(project: &Project) -> Vec<String> {
    vec![
        "run".to_string(),
        project.path(),
        "--input".to_string(),
        "note=the harbour".to_string(),
        "--events".to_string(),
        "json".to_string(),
        "--approvals".to_string(),
        "stdin".to_string(),
    ]
}

fn as_args(owned: &[String]) -> Vec<&str> {
    owned.iter().map(String::as_str).collect()
}

fn stub_env(url: &str) -> Vec<(&str, &str)> {
    vec![
        ("ANTHROPIC_API_KEY", "stub-key"),
        ("INGOT_ANTHROPIC_BASE_URL", url),
    ]
}

fn allow(node: &str) -> String {
    format!(r#"{{"node":"{node}","allowed":true}}"#)
}

/// The run stopped *at the gate*, and the file was not written.
///
/// Asserting the exit code alone would let these pass for any reason at all —
/// an MCP server that would not start looks identical from outside. So the
/// refusal has to be named.
fn assert_refused_at_the_gate(project: &Project, output: &std::process::Output) {
    let message = stderr(output);
    assert_ne!(code(output), EXIT_OK, "{message}");
    assert!(
        message.contains("approval was refused at node"),
        "the run had to stop at the gate rather than somewhere else: {message}"
    );
    assert!(
        !project.written().join("data/out/note.md").exists(),
        "the gate was not opened, so nothing may have been written"
    );
}

#[test]
fn a_gate_answered_over_the_channel_lets_the_run_finish() {
    let project = Project::new("gate-allow");
    let stub = stub_provider(vec![text_reply("A line about the harbour.\n")]);
    let owned = args(&project);

    let output = run_answering(&as_args(&owned), &stub_env(&stub.url), |node, _| {
        Some(allow(node))
    });

    assert_eq!(code(&output), EXIT_OK, "{}", stderr(&output));
    let written = project.written().join("data/out/note.md");
    assert!(
        written.is_file(),
        "the gate was opened, so the write must have happened: {}",
        stderr(&output)
    );
    assert!(
        stderr(&output).contains(r#""event":"approvalDecided""#),
        "the decision belongs in the record too: {}",
        stderr(&output)
    );
}

#[test]
fn without_the_channel_the_same_run_is_still_refused_at_the_gate() {
    // The control, and the entry this work exists to close: standard input is a
    // pipe rather than a terminal, so an artifact that asks for a person is
    // denied. Nothing about that changed — the flag is what makes the
    // difference, and this test is what proves the flag is load-bearing.
    let project = Project::new("gate-default");
    let stub = stub_provider(vec![text_reply("A line about the harbour.\n")]);
    let owned = vec![
        "run".to_string(),
        project.path(),
        "--input".to_string(),
        "note=the harbour".to_string(),
        "--events".to_string(),
        "json".to_string(),
    ];

    let output = run_answering(&as_args(&owned), &stub_env(&stub.url), |node, _| {
        Some(allow(node))
    });

    assert_refused_at_the_gate(&project, &output);
}

#[test]
fn a_gate_refused_over_the_channel_stops_the_run_before_the_effect() {
    let project = Project::new("gate-deny");
    let stub = stub_provider(vec![text_reply("A line about the harbour.\n")]);
    let owned = args(&project);

    let output = run_answering(&as_args(&owned), &stub_env(&stub.url), |node, _| {
        Some(format!(r#"{{"node":"{node}","allowed":false}}"#))
    });

    assert_refused_at_the_gate(&project, &output);
}

#[test]
fn an_answer_naming_another_gate_is_refused_rather_than_applied() {
    // The channel being one message out of step would decide *this* gate with
    // *that* intent. Refusing is the only reading that cannot consent by
    // accident.
    let project = Project::new("gate-mismatch");
    let stub = stub_provider(vec![text_reply("A line about the harbour.\n")]);
    let owned = args(&project);

    let output = run_answering(&as_args(&owned), &stub_env(&stub.url), |_, _| {
        Some(allow("some-other-node"))
    });

    assert_refused_at_the_gate(&project, &output);
    assert!(
        stderr(&output).contains("some-other-node"),
        "the mismatch has to name the gate the answer claimed: {}",
        stderr(&output)
    );
}

#[test]
fn a_parent_that_goes_away_is_a_refusal_and_not_a_hang() {
    // A closed pipe must never read as consent, and it must not wait forever
    // either — the two failures this channel could have.
    let project = Project::new("gate-eof");
    let stub = stub_provider(vec![text_reply("A line about the harbour.\n")]);
    let owned = args(&project);

    let output = run_answering(&as_args(&owned), &stub_env(&stub.url), |_, _| None);

    assert_refused_at_the_gate(&project, &output);
    assert!(
        stderr(&output).contains("closed before"),
        "the run has to say why it refused: {}",
        stderr(&output)
    );
}

#[test]
fn an_unreadable_answer_is_refused_and_says_what_was_expected() {
    let project = Project::new("gate-garbage");
    let stub = stub_provider(vec![text_reply("A line about the harbour.\n")]);
    let owned = args(&project);

    let output = run_answering(&as_args(&owned), &stub_env(&stub.url), |_, _| {
        Some("yes please".to_string())
    });

    assert_refused_at_the_gate(&project, &output);
    assert!(
        stderr(&output).contains("unreadable answer"),
        "the message has to show the shape it wanted: {}",
        stderr(&output)
    );
}

#[test]
fn an_answer_carrying_an_invented_field_is_refused() {
    // `deny_unknown_fields`, for the reason the studio's request struct has it:
    // inventing a field must be a refusal rather than something ignored.
    let project = Project::new("gate-extra");
    let stub = stub_provider(vec![text_reply("A line about the harbour.\n")]);
    let owned = args(&project);

    let output = run_answering(&as_args(&owned), &stub_env(&stub.url), |node, _| {
        Some(format!(
            r#"{{"node":"{node}","allowed":true,"forever":true}}"#
        ))
    });

    assert_refused_at_the_gate(&project, &output);
}

#[test]
fn the_channel_is_refused_when_the_gate_could_not_be_seen() {
    // Two processes waiting for each other is the failure the channel exists to
    // remove, so the combination that would cause it never starts.
    let project = Project::new("gate-unanswerable");
    let output = run_env(
        &[
            "run",
            &project.path(),
            "--input",
            "note=the harbour",
            "--events",
            "text",
            "--approvals",
            "stdin",
        ],
        &[],
    );

    assert_ne!(code(&output), EXIT_OK);
    let message = stderr(&output);
    assert!(message.contains("--events json"), "{message}");
}

#[test]
fn a_blanket_yes_and_a_channel_cannot_be_asked_for_together() {
    // `--yes` answers every gate before the run; the channel answers one gate at
    // the moment it is reached. Asking for both names no coherent behaviour.
    let project = Project::new("gate-both");
    let output = run_env(
        &["run", &project.path(), "--approvals", "stdin", "--yes"],
        &[],
    );

    assert_ne!(code(&output), EXIT_OK);
    let message = stderr(&output);
    assert!(
        message.contains("--yes") && message.contains("--approvals"),
        "{message}"
    );
}
