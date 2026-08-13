//! Shared scaffolding for the `ingot` end-to-end tests.
//!
//! A stub HTTP server stands in for the model provider, so the tests exercise
//! the real HTTP path with no API key and no network. Each test binary uses a
//! different subset of this, hence the blanket allow.

#![allow(dead_code)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;

use serde_json::{json, Value};

pub const EXIT_OK: i32 = 0;
pub const EXIT_DIAGNOSTICS: i32 = 1;
/// An operational failure: something outside the source went wrong.
pub const EXIT_FAILURE: i32 = 2;

pub fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_ingot")
}

pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("the crate must live two levels below the repository root")
        .to_path_buf()
}

pub struct TempDir(PathBuf);

impl TempDir {
    pub fn new(tag: &str) -> TempDir {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock is before the epoch")
            .as_nanos();
        // The clock alone is not enough. `as_nanos` reports whatever resolution
        // the platform has, and on macOS that is microseconds — so two tests
        // starting in the same microsecond got the same directory and quietly
        // overwrote each other's fixtures. Diagnosing that from the failure is
        // very hard: the symptom is one test reading another test's file.
        let counter = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "ingot-run-{tag}-{unique}-{}-{counter}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).expect("creating the scratch directory");
        TempDir(path)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A stub provider that answers `replies` in order, then stops accepting.
pub struct StubProvider {
    pub url: String,
    pub served: Arc<AtomicUsize>,
}

pub fn stub_provider(replies: Vec<Value>) -> StubProvider {
    let listener = TcpListener::bind("127.0.0.1:0").expect("binding a local port");
    let port = listener.local_addr().unwrap().port();
    let served = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&served);

    thread::spawn(move || {
        for stream in listener.incoming().take(replies.len()) {
            let Ok(stream) = stream else { break };
            let index = counter.fetch_add(1, Ordering::SeqCst);
            let reply = replies.get(index).cloned().unwrap_or(Value::Null);
            let _ = answer(stream, &reply);
        }
    });

    StubProvider {
        url: format!("http://127.0.0.1:{port}/v1/messages"),
        served,
    }
}

fn answer(mut stream: TcpStream, reply: &Value) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;

    let mut content_length = 0usize;
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header)? == 0 {
            break;
        }
        let header = header.trim_end();
        if header.is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':') {
            if name.trim().eq_ignore_ascii_case("content-length") {
                content_length = value.trim().parse().unwrap_or(0);
            }
        }
    }
    let mut body = vec![0u8; content_length];
    reader.read_exact(&mut body)?;
    let request: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);

    // The provider streams, so the stub has to. Rather than keep a second set
    // of fixtures, the same reply is re-framed as the event stream that would
    // have produced it — which is also the sharpest test of the property the
    // providers are built on: one parser, two transports, same answer.
    if request.get("stream") == Some(&Value::Bool(true)) {
        let (content_type, payload) = (String::from("text/event-stream"), as_event_stream(reply));
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            payload.len()
        )?;
        stream.write_all(payload.as_bytes())?;
        return stream.flush();
    }

    let payload = serde_json::to_vec(reply)?;
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        payload.len()
    )?;
    stream.write_all(&payload)?;
    stream.flush()
}

/// Re-frame a whole reply as the event stream that would have produced it.
///
/// The vendor is read off the reply's own shape, so a test writes one fixture
/// and gets both transports. A reply in neither shape is served as-is, which is
/// how an error payload reaches the provider unchanged.
fn as_event_stream(reply: &Value) -> String {
    let mut out = String::new();
    let mut push = |name: &str, data: Value| {
        if !name.is_empty() {
            out.push_str(&format!("event: {name}\n"));
        }
        out.push_str(&format!("data: {data}\n\n"));
    };

    if let Some(content) = reply.get("content") {
        let text: String = content
            .as_array()
            .map(|blocks| {
                blocks
                    .iter()
                    .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
                    .filter_map(|block| block.get("text").and_then(Value::as_str))
                    .collect::<Vec<_>>()
                    .join("")
            })
            .unwrap_or_default();
        push(
            "message_start",
            json!({ "type": "message_start", "message": {
                "id": reply.get("id").cloned().unwrap_or(Value::Null),
                "model": reply.get("model").cloned().unwrap_or(Value::Null),
                "usage": reply.get("usage").cloned().unwrap_or(json!({})),
            }}),
        );
        for fragment in fragments(&text) {
            push(
                "content_block_delta",
                json!({ "type": "content_block_delta",
                        "delta": { "type": "text_delta", "text": fragment } }),
            );
        }
        push(
            "message_delta",
            json!({ "type": "message_delta", "delta": {
                "stop_reason": reply.get("stop_reason").cloned().unwrap_or(Value::Null),
                "stop_details": reply.get("stop_details").cloned().unwrap_or(Value::Null),
            }, "usage": reply.get("usage").cloned().unwrap_or(json!({})) }),
        );
        push("message_stop", json!({ "type": "message_stop" }));
        return out;
    }

    if let Some(choice) = reply
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
    {
        let text = choice
            .get("message")
            .and_then(|message| message.get("content"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        let model = reply.get("model").cloned().unwrap_or(Value::Null);
        for fragment in fragments(text) {
            push(
                "",
                json!({ "model": model, "choices": [
                    { "index": 0, "delta": { "content": fragment }, "finish_reason": Value::Null }
                ]}),
            );
        }
        push(
            "",
            json!({ "model": model, "choices": [{
                "index": 0,
                "delta": {},
                "finish_reason": choice.get("finish_reason").cloned().unwrap_or(Value::Null),
            }], "usage": reply.get("usage").cloned().unwrap_or(Value::Null) }),
        );
        push("", json!("[DONE]"));
        // `[DONE]` is framing rather than JSON, and quoting it would make it a
        // string the provider tries to parse as a chunk.
        return out.replace("data: \"[DONE]\"", "data: [DONE]");
    }

    format!("data: {reply}\n\n")
}

/// A few pieces rather than one, so the streaming path is actually exercised.
fn fragments(text: &str) -> Vec<&str> {
    if text.is_empty() {
        return Vec::new();
    }
    let mut pieces = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        let mut cut = rest.len().min(16);
        while cut > 0 && !rest.is_char_boundary(cut) {
            cut -= 1;
        }
        let (piece, tail) = rest.split_at(cut.max(1).min(rest.len()));
        pieces.push(piece);
        rest = tail;
    }
    pieces
}

pub fn text_reply(text: &str) -> Value {
    json!({
        "id": "msg_stub",
        "model": "claude-opus-5",
        "stop_reason": "end_turn",
        "content": [{ "type": "text", "text": text }],
        "usage": { "input_tokens": 120, "output_tokens": 40 },
    })
}

/// A Chat Completions reply, for the OpenAI-compatible provider.
pub fn openai_reply(text: &str) -> Value {
    json!({
        "id": "chatcmpl-stub",
        "model": "gpt-test",
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": text },
            "finish_reason": "stop",
        }],
        "usage": { "prompt_tokens": 120, "completion_tokens": 40 },
    })
}

pub fn run(args: &[&str], base_url: Option<&str>) -> Output {
    match base_url {
        Some(url) => run_env(
            args,
            &[
                ("ANTHROPIC_API_KEY", "stub-key"),
                ("INGOT_ANTHROPIC_BASE_URL", url),
            ],
        ),
        None => run_env(args, &[]),
    }
}

/// Run with exactly these provider variables set, and no others.
///
/// Every key is cleared first: a run must never pick up a real credential from
/// the developer's shell and reach a real service.
pub fn run_env(args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut command = Command::new(binary());
    command.args(args).arg("--color").arg("never");
    for name in [
        "ANTHROPIC_API_KEY",
        "INGOT_ANTHROPIC_BASE_URL",
        "OPENAI_API_KEY",
        "INGOT_OPENAI_BASE_URL",
    ] {
        command.env_remove(name);
    }
    for (name, value) in env {
        command.env(name, value);
    }
    command.output().expect("the ingot binary must be runnable")
}

/// The same, from a chosen working directory.
///
/// For commands whose behaviour depends on where they were typed — a run
/// outside any checkout is the case `ingot conform` most needs to work.
pub fn run_in(cwd: &Path, args: &[&str]) -> Output {
    let mut command = Command::new(binary());
    command
        .args(args)
        .arg("--color")
        .arg("never")
        .current_dir(cwd);
    for name in [
        "ANTHROPIC_API_KEY",
        "INGOT_ANTHROPIC_BASE_URL",
        "OPENAI_API_KEY",
        "INGOT_OPENAI_BASE_URL",
    ] {
        command.env_remove(name);
    }
    command.output().expect("the ingot binary must be runnable")
}

pub fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

pub fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

pub fn code(output: &Output) -> i32 {
    output
        .status
        .code()
        .expect("the process must exit normally")
}

/// Where the reference MCP server binary is, building it if this test binary was
/// invoked in a way that did not.
///
/// `CARGO_BIN_EXE_*` covers this package's own binaries only, and the server
/// belongs to `ingot-mcp`, so it has to be found — and sometimes built — by hand.
pub fn fs_server() -> PathBuf {
    static BUILD: std::sync::Once = std::sync::Once::new();

    let mut dir = std::env::current_exe().expect("the test binary has a path");
    dir.pop();
    if dir.ends_with("deps") {
        dir.pop();
    }
    let path = dir.join(format!("ingot-mcp-fs{}", std::env::consts::EXE_SUFFIX));

    BUILD.call_once(|| {
        if path.is_file() {
            return;
        }
        // `cargo test -p ingot-cli` builds the ingot-mcp library but not its
        // binaries, so build it rather than failing with a puzzle.
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
        let mut command = Command::new(cargo);
        command.current_dir(repo_root()).args([
            "build",
            "-p",
            "ingot-mcp",
            "--bin",
            "ingot-mcp-fs",
        ]);
        if dir.ends_with("release") {
            command.arg("--release");
        }
        let status = command.status().expect("cargo must be runnable");
        assert!(status.success(), "building ingot-mcp-fs failed");
    });

    assert!(
        path.is_file(),
        "expected the reference MCP server at {}",
        path.display()
    );
    path
}

/// A TOML string literal. Windows paths are full of backslashes and a manifest
/// written without escaping them parses as something else entirely.
pub fn toml_string(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

/// A recorded authoring session: one reply per proposal, in order.
///
/// Authoring replays leniently, so a fixture stays valid when the authoring
/// prompt changes. What it pins is the source a model proposed — which the
/// compiler and the routing table then judge — not the prompt that asked.
pub fn authoring_cassette(dir: &Path, name: &str, replies: &[&str]) -> PathBuf {
    let interactions: Vec<Value> = replies
        .iter()
        .enumerate()
        .map(|(index, reply)| {
            json!({
                "index": index,
                "node": format!("authoring.{index}"),
                "requestDigest": "0".repeat(64),
                "responseType": "text",
                "value": format!("```ingot\n{reply}```"),
                "usage": { "inputTokens": 800, "outputTokens": 200 },
                "model": "test/authoring",
            })
        })
        .collect();
    let cassette = json!({
        "cassetteVersion": "0.1",
        "agent": "ingot.authoring",
        "interactions": interactions,
    });

    let path = dir.join(name);
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&cassette).expect("a cassette is serializable"),
    )
    .expect("writing the authoring cassette");
    path
}
