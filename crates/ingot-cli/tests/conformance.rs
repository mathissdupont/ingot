//! Both shipped backends, against the published suite.
//!
//! This is the claim [GAP-017](../../../docs/gaps.md#gap-017) was about: not
//! that the reference works, but that *conformance is a thing somebody else can
//! check*. The suite is data plus a contract; these tests are the proof that the
//! contract is implementable twice.
//!
//! The reference is not privileged here either. It goes through
//! `ingot conform --adapter`, the same door `tools/python-adapter.py` uses.
//!
//! Skips where no Python 3 is on PATH, except for the reference, which needs
//! none. `INGOT_REQUIRE_PYTHON=1` — as CI does — turns that skip into a failure.

mod support;

use std::process::Command;

use support::*;

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

fn suite() -> String {
    repo_root().join("specs/conformance").display().to_string()
}

#[test]
fn the_reference_interpreter_conforms() {
    let output = run(&["conform", "--suite", &suite()], None);
    assert_eq!(
        code(&output),
        EXIT_OK,
        "{}\n{}",
        stdout(&output),
        stderr(&output)
    );
    assert!(stdout(&output).contains("all pass"), "{}", stdout(&output));
}

#[test]
fn the_python_backend_conforms() {
    // The one that matters. A suite only one implementation passes is a
    // regression test wearing a suit.
    let Some(python) = python() else { return };
    let adapter = repo_root()
        .join("specs/conformance/tools/python-adapter.py")
        .display()
        .to_string();
    let backend = format!("{python} {adapter}");

    let output = run(
        &["conform", "--suite", &suite(), "--backend", &backend],
        None,
    );
    assert_eq!(
        code(&output),
        EXIT_OK,
        "{}\n{}",
        stdout(&output),
        stderr(&output)
    );
    assert!(stdout(&output).contains("all pass"), "{}", stdout(&output));
}

#[test]
fn a_backend_that_does_nothing_fails_every_case() {
    // The suite has to be able to say no. A runner that passes anything is
    // worse than none, because it reads as evidence.
    let Some(python) = python() else { return };
    let dir = TempDir::new("conform-idle");
    let idle = dir.path().join("idle.py");
    std::fs::write(&idle, "import sys\nsys.exit(0)\n").expect("writing the idle backend");

    let backend = format!("{python} {}", idle.display());
    let output = run(
        &["conform", "--suite", &suite(), "--backend", &backend],
        None,
    );

    assert_eq!(code(&output), EXIT_DIAGNOSTICS, "{}", stdout(&output));
    let report = stdout(&output);
    assert!(report.contains("case(s) fail"), "{report}");
    // And it says what was wrong rather than only that something was.
    assert!(report.contains("the stream ended early"), "{report}");
    // Every failing case names the clause it is enforcing.
    assert!(report.contains("pins Runtime"), "{report}");
}

#[test]
fn every_case_names_the_clauses_it_pins() {
    // A case that cannot say which rule makes its expectation the right one is
    // a fixture, not a conformance test.
    let output = run(&["conform", "--suite", &suite(), "--list", "--json"], None);
    assert_eq!(code(&output), EXIT_OK, "{}", stderr(&output));

    let described: serde_json::Value =
        serde_json::from_str(&stdout(&output)).expect("the listing must be JSON");
    let cases = described["cases"].as_array().expect("a list of cases");
    assert!(!cases.is_empty(), "the suite must have cases");

    for case in cases {
        let name = case["case"].as_str().unwrap_or_default();
        assert!(
            !case["what"].as_str().unwrap_or_default().is_empty(),
            "{name} says nothing about what it is for"
        );
        let pins = case["pins"].as_array().expect("pins is a list");
        assert!(!pins.is_empty(), "{name} pins no clause");
        for pin in pins {
            let text = pin.as_str().unwrap_or_default();
            assert!(
                text.contains('§'),
                "{name} pins `{text}`, which names no section"
            );
        }
    }
}

#[test]
fn an_adapter_refuses_a_contract_version_it_does_not_implement() {
    // Both directions of the same rule: a backend must not guess which fields
    // changed, and the shipped adapter must demonstrate that.
    let dir = TempDir::new("conform-future");
    let request = dir.path().join("request.json");
    std::fs::write(
        &request,
        serde_json::json!({
            "conformance": "9.9",
            "artifact": "nowhere.json",
            "cassette": "nowhere.json",
            "inputs": {},
            "outDir": dir.path(),
        })
        .to_string(),
    )
    .expect("writing the request");

    let output = run(
        &["conform", "--adapter", &request.display().to_string()],
        None,
    );
    assert_ne!(code(&output), EXIT_OK);
    assert!(
        stderr(&output).contains("refusing rather than guessing"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn the_built_in_suite_is_the_suite_in_the_tree() {
    // The binary carries a copy, and in a checkout the runner prefers the tree
    // -- so the two could drift for a whole release without anybody noticing.
    // This is the only thing that stops that, and it goes through `--export`
    // rather than the module, so what it compares is what ships.
    let dir = TempDir::new("suite-drift");
    let exported = dir.path().join("suite");
    let output = run(
        &["conform", "--export", &exported.display().to_string()],
        None,
    );
    assert_eq!(code(&output), EXIT_OK, "{}", stderr(&output));

    let tree = repo_root().join("specs").join("conformance");
    let mut differing = Vec::new();
    let mut missing = Vec::new();
    let mut stack = vec![tree.clone()];
    while let Some(here) = stack.pop() {
        for entry in std::fs::read_dir(&here)
            .expect("reading the suite")
            .flatten()
        {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let relative = path.strip_prefix(&tree).expect("under the suite");
            // `tools/` is a worked example somebody reads in the repository,
            // and is deliberately not carried in the binary.
            if relative.starts_with("tools") {
                continue;
            }
            let copy = exported.join(relative);
            if !copy.is_file() {
                missing.push(relative.display().to_string());
                continue;
            }
            if std::fs::read(&path).ok() != std::fs::read(&copy).ok() {
                differing.push(relative.display().to_string());
            }
        }
    }
    assert!(missing.is_empty(), "not built into the binary: {missing:?}");
    assert!(
        differing.is_empty(),
        "the built-in copy has drifted: {differing:?}"
    );
}

#[test]
fn the_built_in_suite_runs_with_no_checkout_in_sight() {
    // The point of embedding it. A backend author downloads the binary, points
    // it at their command, and gets a verdict -- no clone, nothing to keep in
    // step with a release.
    let dir = TempDir::new("suite-standalone");
    let output = run_in(dir.path(), &["conform", "--list"]);
    assert_eq!(code(&output), EXIT_OK, "{}", stderr(&output));
    assert!(stdout(&output).contains("prose"), "{}", stdout(&output));
}
