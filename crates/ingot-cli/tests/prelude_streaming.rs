//! The Python backend's streaming path, driven without a socket.
//!
//! The risk this file exists for is named in [RFC-0013](../../../rfcs/0013-streaming.md)
//! and in Runtime 0.3 §3.1: **one parser, two transports**. A second reader on
//! the streaming path would be a second answer to "did this response validate",
//! and the two would drift on exactly the inputs nobody tested. So the
//! accumulators here are checked for one property above all — that what they
//! hand onward is the payload a whole-body call would have returned, down to
//! the field names.
//!
//! Everything is exercised by executing the prelude directly and calling into
//! it, which keeps the framing, the accumulation and the ceiling testable
//! without a network. The end-to-end agreement between the two backends is
//! `differential.rs`; this is the layer underneath it.
//!
//! Skips where no Python 3 is on PATH. `INGOT_REQUIRE_PYTHON=1` — as CI does —
//! turns that skip into a failure, because a test that silently does not run is
//! worse than no test.

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

/// Parse one line of a driver's output, saying what it actually printed.
///
/// A bare `.expect("it must parse")` on a driver's stdout is close to
/// undiagnosable when it fails on a machine you cannot reach.
fn parsed(line: Option<&str>, whole: &str) -> serde_json::Value {
    let line = line.unwrap_or_else(|| {
        panic!(
            "the driver printed nothing:
{whole}"
        )
    });
    serde_json::from_str(line).unwrap_or_else(|error| {
        panic!(
            "expected JSON, got `{line}` ({error})
---
{whole}"
        )
    })
}

/// Run `script` with the prelude already defined, and return its stdout.
///
/// The prelude is read from the file the backend embeds, so a change there is
/// a change here — there is no second copy to fall out of date. `tag` keeps
/// each test's scratch directory its own, which matters because these run in
/// parallel and a shared one had them overwriting each other's driver.
fn drive(tag: &str, script: &str) -> Option<String> {
    let python = python()?;
    let dir = TempDir::new(&format!("prelude-{tag}"));

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

// --- event-stream framing ---------------------------------------------------

#[test]
fn framing_decodes_names_payloads_and_multi_line_data() {
    let Some(out) = drive(
        "framing",
        r#"
import io

seen = []
delivered = []
body = (
    b": keep-alive\n"
    b"event: message_start\n"
    b'data: {"a": 1}\n'
    b"\n"
    b"event: chunk\n"
    b'data: {"b":\n'
    b'data:  2}\n'
    b"\n"
    b"data: [DONE]\n"
    b"\n"
)
read_events(io.BytesIO(body), lambda name, data: seen.append((name, data)), delivered)
print(json.dumps(seen))
print(len(delivered))
"#,
    ) else {
        return;
    };

    let mut lines = out.lines();
    let seen = lines.next().expect("the events");
    // A comment is a keep-alive, and `[DONE]` is not JSON — parsing it would
    // fail the whole call at the last possible moment.
    assert_eq!(
        seen,
        r#"[["message_start", {"a": 1}], ["chunk", {"b": 2}]]"#
    );
    assert_eq!(lines.next(), Some("2"), "the terminator is not an event");
}

// --- Anthropic ---------------------------------------------------------------

/// Drive the accumulator the way a stream would, and report both what a
/// watcher saw and what the reader was handed.
const ANTHROPIC_DRIVER: &str = r#"
def replay(events):
    shown = []
    stream = _AnthropicStream()
    for name, data in events:
        stream.event(name, data, shown.append)
    return stream, shown
"#;

#[test]
fn a_streamed_message_rebuilds_the_payload_a_whole_body_call_returns() {
    let Some(out) = drive(
        "anthropic-text",
        &format!(
            r#"{ANTHROPIC_DRIVER}
stream, shown = replay([
    ("message_start", {{"message": {{"model": "claude-x", "usage": {{"input_tokens": 11, "cache_read_input_tokens": 4}}}}}}),
    ("ping", {{}}),
    ("content_block_delta", {{"delta": {{"type": "text_delta", "text": "Hel"}}}}),
    ("content_block_delta", {{"delta": {{"type": "thinking_delta", "thinking": "hmm"}}}}),
    ("content_block_delta", {{"delta": {{"type": "text_delta", "text": "lo"}}}}),
    ("message_delta", {{"delta": {{"stop_reason": "end_turn"}}, "usage": {{"output_tokens": 7}}}}),
    ("message_stop", {{}}),
])
print(json.dumps(stream.payload(), sort_keys=True))
print(json.dumps(shown))
"#
        ),
    ) else {
        return;
    };

    let mut lines = out.lines();
    let payload = parsed(lines.next(), &out);

    // The field names are the ones the whole-body reply uses. That is the
    // whole point: the reader cannot tell where this came from.
    assert_eq!(payload["model"], "claude-x");
    assert_eq!(payload["stop_reason"], "end_turn");
    assert_eq!(payload["content"][0]["type"], "text");
    assert_eq!(payload["content"][0]["text"], "Hello");
    assert_eq!(payload["usage"]["input_tokens"], 11);
    assert_eq!(payload["usage"]["output_tokens"], 7);
    assert_eq!(payload["usage"]["cache_read_input_tokens"], 4);

    // A thinking delta belongs to a block the reader drops, so showing it
    // would put a sentence on screen the run then throws away.
    let shown: Vec<String> =
        serde_json::from_value(parsed(lines.next(), &out)).expect("a list of strings");
    assert_eq!(shown, vec!["Hel", "lo"]);
}

#[test]
fn a_streamed_tool_response_is_shown_because_it_is_the_answer() {
    // This provider asks for a structured answer through a forced tool call,
    // so the JSON fragments are what becomes the value — unlike the text-block
    // route, there is nothing else to show.
    let Some(out) = drive(
        "anthropic-tool",
        &format!(
            r#"{ANTHROPIC_DRIVER}
stream, shown = replay([
    ("content_block_delta", {{"delta": {{"type": "input_json_delta", "partial_json": "{{\"n\":"}}}}),
    ("content_block_delta", {{"delta": {{"type": "input_json_delta", "partial_json": " 3}}"}}}}),
    ("message_delta", {{"delta": {{"stop_reason": "tool_use"}}}}),
])
print(json.dumps(stream.payload(), sort_keys=True))
print(json.dumps(shown))
"#
        ),
    ) else {
        return;
    };

    let mut lines = out.lines();
    let payload = parsed(lines.next(), &out);
    assert_eq!(payload["content"][0]["type"], "tool_use");
    assert_eq!(payload["content"][0]["input"]["n"], 3);

    let shown: Vec<String> =
        serde_json::from_value(parsed(lines.next(), &out)).expect("a list of strings");
    assert_eq!(shown.concat(), r#"{"n": 3}"#);
}

#[test]
fn a_stream_that_ends_without_a_stop_reason_is_an_incomplete_answer() {
    // The text collected so far may be a fragment, and a fragment that parses
    // is worse than no answer: it passes silently.
    let Some(out) = drive(
        "anthropic-cut",
        &format!(
            r#"{ANTHROPIC_DRIVER}
stream, _ = replay([
    ("content_block_delta", {{"delta": {{"type": "text_delta", "text": "half an ans"}}}}),
])
try:
    stream.payload()
    print("NO FAILURE")
except RunFailed as failure:
    print(str(failure))
"#
        ),
    ) else {
        return;
    };
    assert!(
        out.contains("the answer is incomplete"),
        "a cut connection is not a successful empty answer: {out}"
    );
}

#[test]
fn a_truncated_stream_fails_exactly_as_a_truncated_whole_body_call_does() {
    // Runtime 0.3 §3: on failure the run must fail exactly as an equivalent
    // non-streamed call fails. Both are checked here against the one reader,
    // which is what makes that structural rather than a coincidence.
    let Some(out) = drive(
        "anthropic-truncated",
        r#"
provider = Anthropic("k", None, "claude-x")
request = {
    "node": "n0",
    "prompt": "hi",
    "responseType": "markdown",
    "shape": ("prose", None, False),
    "maxTokens": 100,
}
truncated = {
    "model": "claude-x",
    "stop_reason": "max_tokens",
    "content": [{"type": "text", "text": "began"}],
    "usage": {},
}
for label in ("whole-body", "streamed"):
    try:
        provider._read(request, truncated, "claude-x")
        print(label + ": NO FAILURE")
    except RunFailed as failure:
        print(label + ": " + str(failure))
"#,
    ) else {
        return;
    };

    let lines: Vec<&str> = out.lines().collect();
    assert!(lines[0].contains("cut off at the 100 token limit"), "{out}");
    assert_eq!(
        lines[0].strip_prefix("whole-body: "),
        lines[1].strip_prefix("streamed: "),
        "one reader means one message"
    );
}

// --- OpenAI-compatible --------------------------------------------------------

#[test]
fn streamed_chunks_rebuild_a_chat_completions_reply() {
    let Some(out) = drive(
        "openai",
        r#"
shown = []
stream = _OpenAiStream()
for chunk in [
    {"model": "gpt-x", "choices": [{"delta": {"content": "one "}}]},
    {"choices": [{"delta": {"content": "two"}, "finish_reason": "stop"}]},
    {"choices": [], "usage": {"prompt_tokens": 5, "completion_tokens": 9}},
]:
    stream.chunk(chunk, shown.append)
print(json.dumps(stream.payload(), sort_keys=True))
print(json.dumps(shown))
"#,
    ) else {
        return;
    };

    let mut lines = out.lines();
    let payload = parsed(lines.next(), &out);
    assert_eq!(payload["model"], "gpt-x");
    assert_eq!(payload["choices"][0]["message"]["content"], "one two");
    assert_eq!(payload["choices"][0]["finish_reason"], "stop");
    // Usage arrives on a final chunk of its own, and the budget depends on it.
    assert_eq!(payload["usage"]["completion_tokens"], 9);

    let shown: Vec<String> =
        serde_json::from_value(parsed(lines.next(), &out)).expect("a list of strings");
    assert_eq!(shown, vec!["one ", "two"]);
}

// --- the ceiling ---------------------------------------------------------------

/// A provider that answers whichever way the test asks for.
const CEILING_DRIVER: &str = r#"
class Whole:
    name = "whole"
    def streams(self):
        return False

class Streamed:
    name = "streamed"
    def streams(self):
        return True

def ceiling(provider, budget):
    rt = Runtime("a", {}, Policy({}), budget, None, provider, Events("quiet"), None)
    return rt.max_output_tokens()
"#;

#[test]
fn the_ceiling_comes_from_the_transport_and_not_from_the_artifact() {
    let Some(out) = drive(
        "ceiling",
        &format!(
            r#"{CEILING_DRIVER}
print(ceiling(Whole(), {{}}))
print(ceiling(Streamed(), {{}}))
print(ceiling(Streamed(), {{"tokens": 500}}))
print(ceiling(Replay({{}}), {{}}))
"#
        ),
    ) else {
        return;
    };

    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines[0], "16000", "a whole-body transport keeps 16,000");
    assert_eq!(lines[1], "64000", "a streaming one earns 64,000");
    // Runtime 0.1 §8 allows stricter limits and forbids looser ones, so a
    // budget smaller than the ceiling still wins.
    assert_eq!(
        lines[2], "500",
        "the remaining budget is the stricter bound"
    );
    // A replay produces its answer at once and must not invent fragments, so
    // it cannot accept an answer the recording could not have produced.
    assert_eq!(lines[3], "16000", "a replay does not stream");
}
