//! End-to-end tests for persistent memory.
//!
//! RFC-0018. These are the tests that need two runs to mean anything: the whole
//! point of a persistent field is that the second run sees what the first left,
//! and no in-process test can show that.
//!
//! The agent here calls no model, so the runs need no provider — a cassette
//! with no interactions is enough, and it keeps the tests offline.

mod support;

use std::path::Path;

use serde_json::Value;
use support::{code, run, stderr, TempDir, EXIT_FAILURE, EXIT_OK};

/// A counter that also keeps every note it was given.
///
/// `text` throughout because `string` is not assignable to a `text` artifact,
/// and the point here is the store rather than the type lattice.
fn source(persistent: &str, body: &str) -> String {
    format!(
        r#"language 0.2
package test.recall

agent Counter(note: text) -> log<text> {{
  memory {{
    working ephemeral {{ scratch: text }}
    persistent {{ {persistent} }}
  }}

  budget {{ steps <= 3 }}
  policy {{ network deny }}

  flow {{
    state.scratch = note
{body}
    emit log = state.scratch
  }}
}}
"#
    )
}

fn project(tag: &str, persistent: &str, body: &str) -> TempDir {
    let dir = TempDir::new(tag);
    std::fs::write(dir.path().join("main.ing"), source(persistent, body))
        .expect("writing the agent");
    // A recording with no interactions: this agent asks nothing, so replay has
    // nothing to answer and the run stays offline.
    std::fs::write(
        dir.path().join("empty.json"),
        r#"{"cassetteVersion":"0.1","agent":"test.recall.Counter","interactions":[]}"#,
    )
    .expect("writing the cassette");
    dir
}

fn once(dir: &Path, note: &str, extra: &[&str]) -> std::process::Output {
    let main = dir.join("main.ing");
    let cassette = dir.join("empty.json");
    let out = dir.join("out");
    let mut args: Vec<String> = vec![
        "run".into(),
        main.display().to_string(),
        "--input".into(),
        format!("note={note}"),
        "--provider".into(),
        "replay".into(),
        "--cassette".into(),
        cassette.display().to_string(),
        "--out-dir".into(),
        out.display().to_string(),
        "--events".into(),
        "quiet".into(),
    ];
    args.extend(extra.iter().map(|argument| (*argument).to_string()));
    let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
    run(&borrowed, None)
}

fn store(dir: &Path) -> Value {
    let path = dir
        .join("target")
        .join("ingot")
        .join("memory")
        .join("test.recall.Counter.json");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("reading {}: {error}", path.display()));
    serde_json::from_str(&text).expect("the store is JSON")
}

#[test]
fn the_second_run_sees_what_the_first_one_wrote() {
    let dir = project(
        "memory-persists",
        "depth: int = 0",
        "    memory.depth = memory.depth + 1",
    );

    let first = once(dir.path(), "a", &[]);
    assert_eq!(code(&first), EXIT_OK, "{}", stderr(&first));
    assert_eq!(store(dir.path())["fields"]["depth"], 1);

    let second = once(dir.path(), "b", &[]);
    assert_eq!(code(&second), EXIT_OK, "{}", stderr(&second));
    assert_eq!(
        store(dir.path())["fields"]["depth"],
        2,
        "the second run started from the declared value instead of the store"
    );
}

#[test]
fn a_store_records_the_declaration_it_was_written_under() {
    // Stored in full rather than as a digest, because a digest can only say no
    // and the case worth handling well is naming the field that changed.
    let dir = project(
        "memory-shape",
        "depth: int = 0",
        "    memory.depth = memory.depth + 1",
    );
    once(dir.path(), "a", &[]);
    assert_eq!(store(dir.path())["shape"]["depth"], "int");
    assert_eq!(store(dir.path())["kind"], "memory");
    assert_eq!(store(dir.path())["agent"], "test.recall.Counter");
}

#[test]
fn a_changed_declaration_is_refused_and_the_message_names_the_field() {
    let dir = project(
        "memory-changed",
        "depth: int = 0",
        "    memory.depth = memory.depth + 1",
    );
    assert_eq!(code(&once(dir.path(), "a", &[])), EXIT_OK);

    // Rename the field. The store still holds `depth`.
    std::fs::write(
        dir.path().join("main.ing"),
        source("height: int = 0", "    memory.height = memory.height + 1"),
    )
    .expect("rewriting the agent");

    let refused = once(dir.path(), "b", &[]);
    // An operational failure rather than a diagnostic: the source is fine and
    // the store on disk is what does not fit it.
    assert_eq!(code(&refused), EXIT_FAILURE);
    let text = stderr(&refused);
    assert!(
        text.contains("written for a different declaration"),
        "{text}"
    );
    assert!(text.contains("added:"), "{text}");
    assert!(text.contains("height"), "{text}");
    assert!(text.contains("removed:"), "{text}");
    assert!(text.contains("depth"), "{text}");
    assert!(text.contains("--migrate-memory"), "{text}");
    // Refusing must not have touched it.
    assert_eq!(store(dir.path())["shape"]["depth"], "int");
}

#[test]
fn migrating_drops_what_no_longer_fits_and_says_so() {
    let dir = project(
        "memory-migrate",
        "depth: int = 0, kept: int = 0",
        "    memory.depth = memory.depth + 1\n    memory.kept = 5",
    );
    assert_eq!(code(&once(dir.path(), "a", &[])), EXIT_OK);
    assert_eq!(store(dir.path())["fields"]["kept"], 5);

    std::fs::write(
        dir.path().join("main.ing"),
        source(
            "height: int = 0, kept: int = 0",
            "    memory.height = memory.height + 1",
        ),
    )
    .expect("rewriting the agent");

    let migrated = once(dir.path(), "b", &["--migrate-memory"]);
    assert_eq!(code(&migrated), EXIT_OK, "{}", stderr(&migrated));
    let text = stderr(&migrated);
    // `--events quiet` asks for less chatter, not for silence about data going
    // away, so the loss is reported even here.
    assert!(text.contains("warning:"), "{text}");
    assert!(text.contains("dropped"), "{text}");

    let after = store(dir.path());
    assert_eq!(after["fields"]["kept"], 5, "a matching field survived");
    assert!(
        after["fields"].get("depth").is_none(),
        "the dropped field is gone: {after}"
    );
    assert_eq!(after["shape"]["height"], "int");
}

#[test]
fn no_memory_starts_from_the_declared_values_and_leaves_the_store_alone() {
    let dir = project(
        "memory-disabled",
        "depth: int = 0",
        "    memory.depth = memory.depth + 1",
    );
    assert_eq!(code(&once(dir.path(), "a", &[])), EXIT_OK);
    assert_eq!(store(dir.path())["fields"]["depth"], 1);

    let disabled = once(dir.path(), "b", &["--no-memory"]);
    assert_eq!(code(&disabled), EXIT_OK, "{}", stderr(&disabled));
    assert_eq!(
        store(dir.path())["fields"]["depth"],
        1,
        "--no-memory wrote to the store"
    );
}

#[test]
fn an_agent_that_declares_no_persistent_block_opens_no_store() {
    let dir = TempDir::new("memory-absent");
    std::fs::write(
        dir.path().join("main.ing"),
        r#"language 0.2
package test.recall

agent Counter(note: text) -> log<text> {
  memory { working ephemeral { scratch: text } }
  policy { network deny }
  flow {
    state.scratch = note
    emit log = state.scratch
  }
}
"#,
    )
    .expect("writing the agent");
    std::fs::write(
        dir.path().join("empty.json"),
        r#"{"cassetteVersion":"0.1","agent":"test.recall.Counter","interactions":[]}"#,
    )
    .expect("writing the cassette");

    let output = once(dir.path(), "a", &[]);
    assert_eq!(code(&output), EXIT_OK, "{}", stderr(&output));
    assert!(
        !dir.path().join("target/ingot/memory").exists(),
        "this affects exactly the programs that asked for it"
    );
}

// --- the second backend -----------------------------------------------------

fn python() -> Option<String> {
    for candidate in ["python3", "python"] {
        let ok = std::process::Command::new(candidate)
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

#[test]
fn a_generated_program_keeps_memory_across_runs_too() {
    // The conformance suite covers a single run, because a request carries an
    // artifact, a cassette and inputs and never a store. Nothing there can
    // show the second run reading the first one's file, so this does.
    let Some(python) = python() else { return };

    let dir = project(
        "memory-python",
        "depth: int = 0",
        "    memory.depth = memory.depth + 1",
    );
    let built = dir.path().join("py");
    let build = run(
        &[
            "build",
            &dir.path().join("main.ing").display().to_string(),
            "--target",
            "python",
            "--out-dir",
            &built.display().to_string(),
        ],
        None,
    );
    assert_eq!(code(&build), EXIT_OK, "{}", stderr(&build));

    let program = std::fs::read_dir(&built)
        .expect("the build wrote something")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| path.extension().is_some_and(|ext| ext == "py"))
        .expect("one generated program");

    let store_path = dir.path().join("py-store.json");
    let once = |note: &str| {
        std::process::Command::new(&python)
            .arg(&program)
            .args(["--input", &format!("note={note}")])
            .args([
                "--cassette",
                &dir.path().join("empty.json").display().to_string(),
            ])
            .args(["--memory", &store_path.display().to_string()])
            .args(["--events", "quiet"])
            .args([
                "--out-dir",
                &dir.path().join("py-out").display().to_string(),
            ])
            .env("PYTHONIOENCODING", "utf-8")
            .output()
            .expect("python must be runnable")
    };

    let first = once("a");
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let after_first: Value =
        serde_json::from_str(&std::fs::read_to_string(&store_path).expect("a store was written"))
            .expect("the store is JSON");
    assert_eq!(after_first["fields"]["depth"], 1);
    // The same format the reference writes, so one backend's store is readable
    // by the other.
    assert_eq!(after_first["kind"], "memory");
    assert_eq!(after_first["shape"]["depth"], "int");

    let second = once("b");
    assert!(
        second.status.success(),
        "{}",
        String::from_utf8_lossy(&second.stderr)
    );
    let after_second: Value = serde_json::from_str(
        &std::fs::read_to_string(&store_path).expect("the store is still there"),
    )
    .expect("the store is JSON");
    assert_eq!(after_second["fields"]["depth"], 2);
}
