//! The conformance suite: what a backend must do, made runnable.
//!
//! A backend under test is a **command**. The suite writes one request file per
//! case, runs the command with that file as its only argument, and compares
//! what came back against what the case says must come back. Nothing here knows
//! anything about the backend's language, its flags, or how it reaches a model —
//! only the contract in [`specs/conformance/README.md`].
//!
//! The reference interpreter is not privileged. It reaches the suite through
//! the same adapter a third party writes (`ingot conform --adapter`), so
//! "the reference passes" is a claim the suite can actually check rather than
//! an assumption baked into the runner.
//!
//! [`specs/conformance/README.md`]: https://github.com/mathissdupont/ingot/blob/main/specs/conformance/README.md

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// The contract version a request declares. A backend that does not recognise
/// it should refuse rather than guess.
pub const CONTRACT_VERSION: &str = "0.1";

/// What the suite hands a backend, as one file.
///
/// One file rather than flags, because a backend's command line is its own
/// business: an adapter that reads this is three lines in any language, and the
/// alternative is a template with placeholders that every backend spells
/// differently.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Request {
    /// The contract version, currently [`CONTRACT_VERSION`].
    pub conformance: String,
    /// The Agent IR document to run.
    pub artifact: PathBuf,
    /// The recorded exchange that must be its only source of completions.
    pub cassette: PathBuf,
    /// The agent's declared inputs.
    pub inputs: BTreeMap<String, Value>,
    /// Where the run's artifacts must be written.
    pub out_dir: PathBuf,
}

/// What one case pins down.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Case {
    /// What this case is about, in one sentence.
    pub what: String,
    /// The specification clauses it holds a backend to. Prose, for the report:
    /// a failing case should say which rule it is enforcing.
    pub pins: Vec<String>,
    /// Whether the run is expected to finish or to fail.
    pub outcome: Outcome,
    /// For a failing case, a fragment the failure must name. A run that fails
    /// for an unrelated reason is not a pass.
    #[serde(default)]
    pub failure_names: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    Finished,
    Failed,
}

/// Fields a backend is allowed to differ on, and why.
///
/// The list is short on purpose and belongs to the specification rather than to
/// this file: every entry is a licence to disagree, and an unexamined one is
/// how a suite stops testing the thing it was written for.
///
/// `provider` names the implementation that answered. A conforming backend
/// replaying a cassette has still replayed it, whatever it calls the thing
/// doing the replaying.
///
/// `reason` is a failure's prose. No specification standardises the wording of
/// an error, and one that did would be standardising the wrong thing: what a
/// case requires is that the run failed *and named the thing that failed*, and
/// `failure-names` in `case.toml` is what checks that. Comparing the sentence
/// would make every improvement to an error message a conformance break.
const VARIABLE_FIELDS: &[(&str, &str)] = &[("runStarted", "provider"), ("runFailed", "reason")];

/// One case's verdict.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Verdict {
    pub case: String,
    pub what: String,
    pub pins: Vec<String>,
    pub passed: bool,
    /// Every way this case disagreed with what it requires. Empty when passed.
    pub findings: Vec<String>,
}

/// The whole run.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub contract: String,
    pub backend: String,
    pub cases: Vec<Verdict>,
}

impl Report {
    pub fn conformant(&self) -> bool {
        self.cases.iter().all(|case| case.passed)
    }

    pub fn render(&self) -> String {
        use std::fmt::Write;
        let mut out = String::new();
        let _ = writeln!(
            out,
            "conformance {} against `{}`\n",
            self.contract, self.backend
        );
        for case in &self.cases {
            let _ = writeln!(
                out,
                "{} {}",
                if case.passed { "pass" } else { "FAIL" },
                case.case
            );
            if !case.passed {
                let _ = writeln!(out, "     {}", case.what);
                for pin in &case.pins {
                    let _ = writeln!(out, "     pins {pin}");
                }
                for finding in &case.findings {
                    for (index, line) in finding.lines().enumerate() {
                        let _ = writeln!(out, "     {} {line}", if index == 0 { "-" } else { " " });
                    }
                }
            }
        }
        let failed = self.cases.iter().filter(|case| !case.passed).count();
        let _ = writeln!(out);
        if failed == 0 {
            let _ = writeln!(
                out,
                "{} case(s), all pass. This backend conforms to the suite as it stands.",
                self.cases.len()
            );
        } else {
            let _ = writeln!(
                out,
                "{failed} of {} case(s) fail. Each names the clause it holds you to.",
                self.cases.len()
            );
        }
        out.trim_end().to_string()
    }
}

/// Every case in a suite directory, in a stable order.
pub fn cases(suite: &Path) -> Result<Vec<(String, PathBuf)>> {
    let dir = suite.join("cases");
    let mut found: Vec<(String, PathBuf)> = std::fs::read_dir(&dir)
        .with_context(|| format!("reading the suite at {}", dir.display()))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.join("case.toml").is_file())
        .filter_map(|path| {
            let name = path.file_name()?.to_str()?.to_string();
            Some((name, path))
        })
        .collect();
    found.sort();
    Ok(found)
}

/// Run every case in `suite` against `backend`, and report.
///
/// `backend` is a command line. Its arguments are taken as written and the
/// request file is appended, so `--backend "python adapter.py"` runs
/// `python adapter.py <request>`.
pub fn run(suite: &Path, backend: &str, only: Option<&str>, work: &Path) -> Result<Report> {
    let mut verdicts = Vec::new();
    for (name, dir) in cases(suite)? {
        if let Some(only) = only {
            if only != name {
                continue;
            }
        }
        verdicts.push(one(&name, &dir, backend, &work.join(&name))?);
    }
    if verdicts.is_empty() {
        anyhow::bail!(
            "no cases matched{}",
            only.map(|name| format!(" `{name}`")).unwrap_or_default()
        );
    }
    Ok(Report {
        contract: CONTRACT_VERSION.to_string(),
        backend: backend.to_string(),
        cases: verdicts,
    })
}

fn one(name: &str, dir: &Path, backend: &str, work: &Path) -> Result<Verdict> {
    let case: Case = toml::from_str(&read(&dir.join("case.toml"))?)
        .with_context(|| format!("reading the case `{name}`"))?;

    let out_dir = work.join("out");
    std::fs::create_dir_all(&out_dir).with_context(|| format!("creating {}", out_dir.display()))?;

    let inputs: BTreeMap<String, Value> = serde_json::from_str(&read(&dir.join("inputs.json"))?)
        .with_context(|| format!("reading the inputs of `{name}`"))?;

    let request = Request {
        conformance: CONTRACT_VERSION.to_string(),
        artifact: absolute(&dir.join("agent.ir.json")),
        cassette: absolute(&dir.join("cassette.json")),
        inputs,
        out_dir: absolute(&out_dir),
    };
    let request_path = work.join("request.json");
    std::fs::write(&request_path, serde_json::to_string_pretty(&request)?)
        .with_context(|| format!("writing {}", request_path.display()))?;

    let mut parts = backend.split_whitespace();
    let program = parts
        .next()
        .context("the backend command is empty")?
        .to_string();
    let output = Command::new(&program)
        .args(parts)
        .arg(&request_path)
        .output()
        .with_context(|| format!("running `{program}`"))?;

    let mut findings = Vec::new();
    check_outcome(&case, &output, &mut findings);
    check_events(dir, &output, &mut findings)?;
    check_artifacts(dir, &out_dir, &mut findings)?;

    Ok(Verdict {
        case: name.to_string(),
        what: case.what,
        pins: case.pins,
        passed: findings.is_empty(),
        findings,
    })
}

fn check_outcome(case: &Case, output: &std::process::Output, findings: &mut Vec<String>) {
    let finished = output.status.success();
    let stderr = String::from_utf8_lossy(&output.stderr);
    match (case.outcome, finished) {
        (Outcome::Finished, false) => findings.push(format!(
            "the run was expected to finish and failed instead:\n{}",
            tail(&stderr)
        )),
        (Outcome::Failed, true) => {
            findings.push("the run was expected to fail and finished instead".to_string())
        }
        _ => {}
    }
    if case.outcome == Outcome::Failed && !finished {
        if let Some(fragment) = &case.failure_names {
            if !stderr.contains(fragment.as_str()) {
                findings.push(format!(
                    "the run failed, but not for the reason this case is about: \
                     nothing said `{fragment}`\n{}",
                    tail(&stderr)
                ));
            }
        }
    }
}

/// Compare the event stream, in order and field for field.
///
/// A line with no `event` key is not an event: it is the live channel, which
/// Runtime 0.3 §2.1 forbids asserting on. Dropping those lines here is that
/// rule made executable rather than merely written down.
fn check_events(
    dir: &Path,
    output: &std::process::Output,
    findings: &mut Vec<String>,
) -> Result<()> {
    let expected = events(&read(&dir.join("expected").join("events.jsonl"))?);
    let actual = events(&String::from_utf8_lossy(&output.stderr));

    for (index, expect) in expected.iter().enumerate() {
        let Some(got) = actual.get(index) else {
            findings.push(format!(
                "the stream ended early: event {index} should have been `{}`",
                kind(expect)
            ));
            return Ok(());
        };
        if let Some(difference) = differs(expect, got) {
            findings.push(format!("event {index}: {difference}"));
        }
    }
    if actual.len() > expected.len() {
        findings.push(format!(
            "{} event(s) too many; the first extra one is `{}`",
            actual.len() - expected.len(),
            kind(&actual[expected.len()])
        ));
    }
    Ok(())
}

fn events(text: &str) -> Vec<Value> {
    text.lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|value| value.get("event").and_then(Value::as_str).is_some())
        .collect()
}

fn kind(event: &Value) -> String {
    event
        .get("event")
        .and_then(Value::as_str)
        .unwrap_or("?")
        .to_string()
}

/// How two events disagree, ignoring the fields the contract allows to vary.
fn differs(expected: &Value, actual: &Value) -> Option<String> {
    let expected_kind = kind(expected);
    if expected_kind != kind(actual) {
        return Some(format!(
            "expected `{expected_kind}`, got `{}`",
            kind(actual)
        ));
    }
    let allowed: Vec<&str> = VARIABLE_FIELDS
        .iter()
        .filter(|(on, _)| *on == expected_kind)
        .map(|(_, field)| *field)
        .collect();

    let strip = |value: &Value| -> Value {
        let mut object = value.clone();
        if let Some(map) = object.as_object_mut() {
            for field in &allowed {
                map.remove(*field);
            }
        }
        object
    };
    let (left, right) = (strip(expected), strip(actual));
    if left == right {
        return None;
    }
    Some(format!(
        "`{expected_kind}` differs\n  expected {left}\n  got      {right}"
    ))
}

/// Compare every artifact the case declares, byte for byte.
fn check_artifacts(dir: &Path, out_dir: &Path, findings: &mut Vec<String>) -> Result<()> {
    let expected_dir = dir.join("expected").join("outputs");
    if !expected_dir.is_dir() {
        return Ok(());
    }
    let mut entries: Vec<PathBuf> = std::fs::read_dir(&expected_dir)
        .with_context(|| format!("reading {}", expected_dir.display()))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .collect();
    entries.sort();

    for expected in entries {
        let Some(name) = expected.file_name() else {
            continue;
        };
        let actual = out_dir.join(name);
        let want = std::fs::read(&expected)?;
        let Ok(got) = std::fs::read(&actual) else {
            findings.push(format!(
                "no artifact `{}` was written",
                name.to_string_lossy()
            ));
            continue;
        };
        if want != got {
            findings.push(format!(
                "artifact `{}` differs\n  expected {} byte(s)\n  got      {} byte(s)",
                name.to_string_lossy(),
                want.len(),
                got.len()
            ));
        }
    }
    Ok(())
}

fn read(path: &Path) -> Result<String> {
    std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))
}

fn absolute(path: &Path) -> PathBuf {
    std::fs::canonicalize(path)
        .map(|resolved| {
            // Windows canonicalisation prefixes a verbatim marker that many
            // runtimes cannot open. Strip it for a plain drive path.
            let text = resolved.display().to_string();
            match text.strip_prefix(r"\\?\") {
                Some(rest) if rest.chars().nth(1) == Some(':') => PathBuf::from(rest),
                _ => resolved,
            }
        })
        .unwrap_or_else(|_| path.to_path_buf())
}

fn tail(text: &str) -> String {
    let lines: Vec<&str> = text.lines().filter(|line| !line.is_empty()).collect();
    lines[lines.len().saturating_sub(4)..].join("\n")
}

// --- the reference adapter ---------------------------------------------------

/// Read a request and describe the run it asks for.
///
/// Deliberately thin. The reference interpreter reaches the suite the same way
/// a third-party backend does — through an adapter that reads the request file
/// — so nothing about the reference is privileged, and "the reference passes"
/// stays a claim the suite checks rather than one the runner assumes.
pub fn read_request(path: &Path) -> Result<Request> {
    let request: Request = serde_json::from_str(&read(path)?)
        .with_context(|| format!("reading the request at {}", path.display()))?;
    if request.conformance != CONTRACT_VERSION {
        anyhow::bail!(
            "this request declares conformance contract `{}` and this adapter implements `{}`; \
             refusing rather than guessing which fields changed",
            request.conformance,
            CONTRACT_VERSION
        );
    }
    Ok(request)
}

/// Run one request with the reference interpreter, as a backend would.
///
/// Everything a third-party adapter has to do is here, in the order it has to
/// do it: read the request, load the artifact, serve completions from the
/// cassette and nothing else, write the event stream to standard error as JSON
/// Lines, write the artifacts, and exit non-zero if the run failed.
pub fn adapt(path: &Path) -> Result<u8> {
    use ingot_ir::AgentIr;
    use ingot_runtime::{run as run_agent, ApprovalMode, Cassette, RunOptions};

    let request = read_request(path)?;

    let ir = AgentIr::from_json(&read(&request.artifact)?)
        .map_err(|error| anyhow::anyhow!("{}: {error}", request.artifact.display()))?;
    let cassette = Cassette::from_json(&read(&request.cassette)?)
        .map_err(|error| anyhow::anyhow!("{}: {error}", request.cassette.display()))?;

    let registry = std::iter::once((ir.agent.clone(), ir.clone())).collect();
    let recorded_tools = cassette.tool_calls.clone();
    let mut provider = ingot_runtime::ReplayProvider::new(cassette);
    let mut tools = ingot_runtime::ReplayToolHost::new(recorded_tools);
    let mut sink = JsonLines;

    let result = run_agent(
        &ir,
        &registry,
        &mut provider,
        &mut tools,
        &mut sink,
        RunOptions {
            inputs: request.inputs.clone(),
            // A conformance run is unattended, and an approval nobody can grant
            // is a denial. Any case that needs one says so in its expectation.
            approval: ApprovalMode::Deny,
            max_steps: 1_000,
            pricing: Default::default(),
        },
    );

    let report = match result {
        Ok(report) => report,
        Err(error) => {
            eprintln!("{error}");
            return Ok(crate::EXIT_FAILURE);
        }
    };

    std::fs::create_dir_all(&request.out_dir)
        .with_context(|| format!("creating {}", request.out_dir.display()))?;
    for artifact in report.outputs.values() {
        let path = request
            .out_dir
            .join(format!("{}.{}", artifact.name, artifact.extension()));
        std::fs::write(&path, artifact.to_bytes())
            .with_context(|| format!("writing {}", path.display()))?;
    }
    Ok(crate::EXIT_OK)
}

/// The event stream, one JSON object per line, on standard error.
///
/// Standard output is left alone: it carries a run's artifacts, and splicing
/// events into it would break any pipeline reading them. Deltas are dropped
/// rather than written, because a replay produces none and a conformance run
/// has nothing live to show.
struct JsonLines;

impl ingot_runtime::EventSink for JsonLines {
    fn emit(&mut self, event: ingot_runtime::RunEvent) {
        eprintln!("{}", event.to_json_line());
    }
}

/// A machine-readable description of the suite, for a backend author.
pub fn describe(suite: &Path) -> Result<Value> {
    let mut described = Vec::new();
    for (name, dir) in cases(suite)? {
        let case: Case = toml::from_str(&read(&dir.join("case.toml"))?)
            .with_context(|| format!("reading the case `{name}`"))?;
        described.push(json!({
            "case": name,
            "what": case.what,
            "pins": case.pins,
            "outcome": case.outcome,
        }));
    }
    Ok(json!({
        "conformance": CONTRACT_VERSION,
        "variableFields": VARIABLE_FIELDS
            .iter()
            .map(|(on, field)| json!({ "event": on, "field": field }))
            .collect::<Vec<_>>(),
        "cases": described,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(kind: &str, extra: Value) -> Value {
        let mut value = json!({ "event": kind });
        if let (Some(map), Some(more)) = (value.as_object_mut(), extra.as_object()) {
            for (name, field) in more {
                map.insert(name.clone(), field.clone());
            }
        }
        value
    }

    #[test]
    fn a_line_with_no_event_key_is_not_an_event() {
        // Runtime 0.3 §2.1: a delta must not be asserted on by a conformance
        // test. The contract's own discrimination rule is what enforces it.
        let stream = concat!(
            r#"{"event":"runStarted","agent":"a","provider":"replay"}"#,
            "\n",
            r#"{"delta":{"node":"n0","text":"half an ans"}}"#,
            "\n",
            r#"{"settled":{"node":"n0","kept":true}}"#,
            "\n",
            r#"{"event":"runFinished","steps":1}"#,
            "\n",
        );
        let kinds: Vec<String> = events(stream).iter().map(kind).collect();
        assert_eq!(kinds, vec!["runStarted", "runFinished"]);
    }

    #[test]
    fn the_provider_name_is_allowed_to_differ_and_nothing_else_is() {
        let expected = event("runStarted", json!({ "agent": "a", "provider": "replay" }));
        let actual = event("runStarted", json!({ "agent": "a", "provider": "python" }));
        assert_eq!(differs(&expected, &actual), None);

        // The same licence does not extend to the agent it says it ran.
        let wrong = event("runStarted", json!({ "agent": "b", "provider": "python" }));
        assert!(differs(&expected, &wrong).is_some());
    }

    #[test]
    fn a_field_that_differs_on_any_other_event_is_a_finding() {
        // `provider` is licensed on `runStarted` alone. A backend inventing one
        // elsewhere is inventing a field.
        let expected = event("modelCall", json!({ "node": "n0", "model": "claude-x" }));
        let actual = event("modelCall", json!({ "node": "n0", "model": "gpt-x" }));
        let finding = differs(&expected, &actual).expect("a different model is a difference");
        assert!(finding.contains("modelCall"), "{finding}");
    }

    #[test]
    fn a_stream_of_the_right_kinds_in_the_wrong_order_is_a_finding() {
        let expected = event("verified", json!({ "node": "n1" }));
        let actual = event("emitted", json!({ "node": "n1" }));
        let finding = differs(&expected, &actual).expect("order is part of the record");
        assert!(finding.contains("expected `verified`"), "{finding}");
    }

    #[test]
    fn an_adapter_refuses_a_contract_it_does_not_implement() {
        let dir = std::env::temp_dir().join("ingot-conform-contract");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("request.json");
        std::fs::write(
            &path,
            json!({
                "conformance": "9.9",
                "artifact": "a.json",
                "cassette": "c.json",
                "inputs": {},
                "outDir": "out",
            })
            .to_string(),
        )
        .unwrap();

        let error = read_request(&path).expect_err("a future contract must be refused");
        assert!(error.to_string().contains("refusing rather than guessing"));
        let _ = std::fs::remove_file(&path);
    }
}
