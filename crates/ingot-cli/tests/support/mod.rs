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
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock is before the epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("ingot-run-{tag}-{unique}"));
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

    let payload = serde_json::to_vec(reply)?;
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        payload.len()
    )?;
    stream.write_all(&payload)?;
    stream.flush()
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

pub fn run(args: &[&str], base_url: Option<&str>) -> Output {
    let mut command = Command::new(binary());
    command.args(args).arg("--color").arg("never");
    match base_url {
        Some(url) => {
            command.env("ANTHROPIC_API_KEY", "stub-key");
            command.env("INGOT_ANTHROPIC_BASE_URL", url);
        }
        None => {
            command.env_remove("ANTHROPIC_API_KEY");
            command.env_remove("INGOT_ANTHROPIC_BASE_URL");
        }
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
