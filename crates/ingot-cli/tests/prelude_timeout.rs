//! How long a generated Python program waits for a model.
//!
//! The reference interpreter takes its wait from `[[model.provider]]`. A
//! generated program has no manifest to read — it is one self-contained file —
//! so it takes the same fact from `INGOT_MODEL_TIMEOUT_SECONDS`, the surface it
//! already reads `INGOT_OPENAI_BASE_URL` from. This file holds the two halves
//! to the same rules: the variable is honoured, a value that is not a number is
//! refused rather than ignored, and a deadline ends the call instead of being
//! asked again.
//!
//! Driven by executing the prelude, so there is no second copy of it here. See
//! [GAP-040](../../../docs/gaps.md#gap-040).

mod support;

use std::process::Command;

use support::*;

fn python() -> Option<String> {
    for candidate in ["python3", "python"] {
        let ok = Command::new(candidate)
            .arg("--version")
            .output()
            .map(|out| out.status.success())
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

/// Run `script` with the prelude already defined, and return its stdout.
///
/// The environment is set from inside the script rather than on the child, so
/// each case owns its own variable and the cases cannot race each other.
fn drive(tag: &str, script: &str) -> Option<String> {
    let python = python()?;
    let dir = TempDir::new(&format!("prelude-timeout-{tag}"));

    let prelude =
        std::fs::read_to_string(repo_root().join("crates/ingot-backend-python/src/prelude.py"))
            .expect("the prelude must be there");

    let path = dir.path().join("driver.py");
    std::fs::write(&path, format!("{prelude}\n\n{script}\n")).expect("writing the driver");

    let output = Command::new(python)
        .arg(&path)
        .output()
        .expect("running python");
    assert!(
        output.status.success(),
        "driver failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[test]
fn the_wait_comes_from_the_environment_and_falls_back_to_the_default() {
    let Some(out) = drive(
        "resolve",
        r#"
os.environ.pop("INGOT_MODEL_TIMEOUT_SECONDS", None)
print(model_timeout())

os.environ["INGOT_MODEL_TIMEOUT_SECONDS"] = "900"
print(model_timeout())

# Zero waits indefinitely, which urllib spells as no deadline at all.
os.environ["INGOT_MODEL_TIMEOUT_SECONDS"] = "0"
print(model_timeout())

# Exported-but-empty is a shell accident, not a request to wait no time.
os.environ["INGOT_MODEL_TIMEOUT_SECONDS"] = "   "
print(model_timeout())
"#,
    ) else {
        return;
    };
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines, vec!["180", "900", "None", "180"], "{out}");
}

#[test]
fn a_value_that_is_not_a_number_is_refused_rather_than_ignored() {
    // Ignoring it would leave an operator believing they had raised a ceiling
    // on a run where they had not, and the only evidence would be the timeout
    // they were trying to avoid.
    let Some(out) = drive(
        "refused",
        r#"
os.environ["INGOT_MODEL_TIMEOUT_SECONDS"] = "15m"
try:
    model_timeout()
    print("ACCEPTED")
except RunFailed as failure:
    print("refused:", str(failure).splitlines()[0])
    print("operator:", failure.operator)
"#,
    ) else {
        return;
    };
    assert!(
        out.contains("INGOT_MODEL_TIMEOUT_SECONDS is `15m`"),
        "{out}"
    );
    assert!(out.contains("operator: True"), "{out}");
    assert!(!out.contains("ACCEPTED"), "{out}");
}

#[test]
fn a_read_deadline_is_a_named_failure_rather_than_a_traceback() {
    // It used to be a traceback: `urlopen` raises a read deadline bare, and
    // `_post` caught only `HTTPError` and `URLError`, so it escaped both
    // handlers and ended the program without naming anything.
    //
    // It is also not retried. Four attempts at a stated ceiling would make the
    // number an operator wrote mean four times itself, so the elapsed time is
    // asserted as well as the message.
    let Some(out) = drive(
        "deadline",
        r#"
import socket, threading, time

listener = socket.socket()
listener.bind(("127.0.0.1", 0))
listener.listen(8)
port = listener.getsockname()[1]

# Held open rather than closed: a closed socket is a different failure, and
# would prove nothing about a deadline.
held = []
threading.Thread(
    target=lambda: [held.append(listener.accept()) for _ in range(8)], daemon=True
).start()

url = "http://127.0.0.1:%d/v1/chat/completions" % port
started = time.time()
try:
    _post(url, {}, {"model": "m"}, 0.4)
    print("ANSWERED")
except RunFailed as failure:
    print("failed:", "timed out" in str(failure) or "timeout" in str(failure))
elapsed = time.time() - started
print("attempts_fit_in_two:", elapsed < 0.8)
"#,
    ) else {
        return;
    };
    assert!(out.contains("failed: True"), "{out}");
    assert!(out.contains("attempts_fit_in_two: True"), "{out}");
    assert!(!out.contains("ANSWERED"), "{out}");
}
