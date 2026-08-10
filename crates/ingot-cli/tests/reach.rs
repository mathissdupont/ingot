//! Where a tool may reach, end to end.
//!
//! A policy's value list has always been advisory where nothing enforces it,
//! and Ingot says so. `!network("arxiv.org")` is a stronger statement: it says
//! this tool must be bounded to that host. These tests pin the two things that
//! makes true — the compiler compares it against the grant, and a run that
//! cannot keep it stops instead of pretending.
//!
//! See [RFC-0014](../../../rfcs/0014-a-capabilitys-reach.md).

mod support;

use std::path::Path;

use support::*;

fn project(dir: &Path, effects: &str, policy: &str) {
    std::fs::write(
        dir.join("main.ing"),
        format!(
            "language 0.1\n\
             tool feed.fetch(url: string) -> string {effects}\n\
             agent Reader(topic: string) -> note<markdown> {{\n\
             \x20 model requires {{ structured_output }}\n\
             \x20 tools {{ mcp feed.fetch }}\n\
             \x20 budget {{ steps <= 4 }}\n\
             \x20 policy {{ {policy} }}\n\
             \x20 flow {{\n\
             \x20   page = call feed.fetch(topic)\n\
             \x20   emit note = ask<markdown>(\"Summarise it.\", context: page)\n\
             \x20 }}\n\
             }}\n"
        ),
    )
    .expect("writing the source");
    std::fs::write(
        dir.join("ingot.toml"),
        "[project]\nname = \"reader\"\nversion = \"0.1.0\"\n\n\
         [build]\nentry = \"main.ing\"\nout-dir = \"target/ingot\"\n",
    )
    .expect("writing the manifest");
}

fn check_project(dir: &Path) -> std::process::Output {
    run_env(&["check", &dir.display().to_string()], &[])
}

#[test]
fn a_tool_that_reaches_beyond_the_policy_does_not_compile() {
    let dir = TempDir::new("reach-beyond");
    project(
        dir.path(),
        r#"!network("arxiv.org", "github.com")"#,
        r#"network allow ["arxiv.org"]"#,
    );

    let output = check_project(dir.path());
    assert_eq!(code(&output), EXIT_DIAGNOSTICS, "{}", stderr(&output));

    let log = stderr(&output);
    assert!(log.contains("ING4009"), "{log}");
    assert!(log.contains("github.com"), "{log}");
    // Both halves of the mistake are named, because either one may be wrong.
    assert!(log.contains("granted here"), "{log}");
}

#[test]
fn a_tool_within_the_policy_compiles() {
    let dir = TempDir::new("reach-within");
    project(
        dir.path(),
        r#"!network("arxiv.org")"#,
        r#"network allow ["arxiv.org", "github.com"]"#,
    );

    let output = check_project(dir.path());
    assert_eq!(code(&output), EXIT_OK, "{}", stderr(&output));
}

#[test]
fn a_run_that_cannot_keep_a_declared_reach_refuses_before_starting() {
    let dir = TempDir::new("reach-refuse");
    project(
        dir.path(),
        r#"!network("arxiv.org")"#,
        r#"network allow ["arxiv.org"]"#,
    );

    let output = run_env(
        &[
            "run",
            &dir.path().display().to_string(),
            "--provider",
            "replay",
            "--cassette",
            "nonexistent.json",
            "--input",
            "topic=compilers",
        ],
        &[],
    );

    // The reach is checked before anything else about the run, including the
    // cassette that is not there: refusing after the work has started is the
    // failure mode this check exists to avoid.
    let log = stderr(&output);
    assert_ne!(code(&output), EXIT_OK, "{log}");
    assert!(log.contains("where its tools may reach"), "{log}");
    assert!(log.contains("GAP-001"), "{log}");
    assert!(log.contains("feed.fetch"), "{log}");
    assert!(log.contains("--allow-unenforced-scopes"), "{log}");
    assert!(
        !log.contains("nonexistent.json"),
        "the cassette should never have been opened:\n{log}"
    );
}

#[test]
fn the_refusal_can_be_acknowledged_and_says_what_is_advisory() {
    let dir = TempDir::new("reach-ack");
    project(
        dir.path(),
        r#"!network("arxiv.org")"#,
        r#"network allow ["arxiv.org"]"#,
    );

    let output = run_env(
        &[
            "run",
            &dir.path().display().to_string(),
            "--provider",
            "replay",
            "--cassette",
            "nonexistent.json",
            "--allow-unenforced-scopes",
            "--input",
            "topic=compilers",
        ],
        &[],
    );

    let log = stderr(&output);
    // The run proceeds past the reach check and fails on the missing cassette,
    // which is the next real problem rather than this one.
    assert!(
        log.contains("warning: proceeding with a reach nothing enforces"),
        "{log}"
    );
    assert!(!log.contains("where its tools may reach"), "{log}");
}

#[test]
fn an_artifact_that_declares_no_reach_is_unaffected() {
    // Opt-in strictness: everything written before RFC-0014 runs as it did.
    let dir = TempDir::new("reach-none");
    project(dir.path(), "!network", r#"network allow ["arxiv.org"]"#);

    let output = run_env(
        &[
            "run",
            &dir.path().display().to_string(),
            "--provider",
            "replay",
            "--cassette",
            "nonexistent.json",
            "--input",
            "topic=compilers",
        ],
        &[],
    );

    let log = stderr(&output);
    assert!(!log.contains("where its tools may reach"), "{log}");
}

#[test]
fn the_declared_reach_reaches_the_artifact() {
    // A backend that will one day enforce this has to be able to read it.
    let dir = TempDir::new("reach-ir");
    project(
        dir.path(),
        r#"!network("github.com", "arxiv.org")"#,
        r#"network allow ["arxiv.org", "github.com"]"#,
    );

    let built = run_env(&["build", &dir.path().display().to_string()], &[]);
    assert_eq!(code(&built), EXIT_OK, "{}", stderr(&built));

    let ir = dir.path().join("target/ingot/Reader.ir.json");
    let text = std::fs::read_to_string(&ir).unwrap_or_else(|_| panic!("{}", ir.display()));
    let parsed: serde_json::Value = serde_json::from_str(&text).expect("valid IR");
    assert_eq!(
        parsed["tools"][0]["scopes"]["network"],
        serde_json::json!(["arxiv.org", "github.com"]),
        "sorted at the declaration, so the encoding is canonical:\n{text}"
    );
}
