//! A stub HTTP server, shared by the provider wire tests.
//!
//! These tests exercise the real HTTP path — request construction, headers,
//! response parsing, error mapping — against localhost. No API key and no
//! internet, so they run in CI like any other test.
//!
//! Mocking at the trait boundary instead would test everything except the part
//! most likely to be wrong.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc;
use std::thread;

use serde_json::Value;

/// What the stub server saw.
pub struct Captured {
    pub headers: BTreeMap<String, String>,
    pub body: Value,
    /// The request target, including any query. Recorded because one protocol
    /// puts the model and the method in the path, and because a test needs to
    /// be able to prove a credential is *not* in there.
    pub target: String,
}

/// Serve one request with `status` and `response`, and report what arrived.
///
/// `path` is appended to the URL so each provider's endpoint reads the way it
/// does in production.
pub fn serve_once(path: &str, status: u16, response: Value) -> (String, mpsc::Receiver<Captured>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("binding a local port");
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = mpsc::channel();

    thread::spawn(move || {
        let Ok((stream, _)) = listener.accept() else {
            return;
        };
        if let Some(captured) = handle(stream, status, &response) {
            let _ = tx.send(captured);
        }
    });

    (format!("http://127.0.0.1:{port}{path}"), rx)
}

/// Serve one request as `text/event-stream`, and report what arrived.
///
/// Each entry is written as `event: <name>\ndata: <data>\n\n`, with the `event:`
/// line omitted when the name is empty — which is how an OpenAI-compatible
/// stream frames its chunks, and how it says `[DONE]`. `data` is raw text
/// rather than a `Value` so a test can serve something unparseable on purpose.
pub fn serve_stream(
    path: &str,
    events: Vec<(String, String)>,
) -> (String, mpsc::Receiver<Captured>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("binding a local port");
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = mpsc::channel();

    let mut payload = String::new();
    for (name, data) in events {
        if !name.is_empty() {
            payload.push_str(&format!("event: {name}\n"));
        }
        payload.push_str(&format!("data: {data}\n\n"));
    }

    thread::spawn(move || {
        let Ok((stream, _)) = listener.accept() else {
            return;
        };
        if let Some(captured) = handle_raw(stream, 200, "text/event-stream", payload.as_bytes()) {
            let _ = tx.send(captured);
        }
    });

    (format!("http://127.0.0.1:{port}{path}"), rx)
}

fn handle(stream: TcpStream, status: u16, response: &Value) -> Option<Captured> {
    let payload = serde_json::to_vec(response).ok()?;
    handle_raw(stream, status, "application/json", &payload)
}

fn handle_raw(
    mut stream: TcpStream,
    status: u16,
    content_type: &str,
    payload: &[u8],
) -> Option<Captured> {
    let mut reader = BufReader::new(stream.try_clone().ok()?);

    let mut request_line = String::new();
    reader.read_line(&mut request_line).ok()?;

    let mut headers = BTreeMap::new();
    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).ok()? == 0 {
            break;
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            let name = name.trim().to_ascii_lowercase();
            let value = value.trim().to_string();
            if name == "content-length" {
                content_length = value.parse().unwrap_or(0);
            }
            headers.insert(name, value);
        }
    }

    let mut body = vec![0u8; content_length];
    reader.read_exact(&mut body).ok()?;
    let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);

    let head = format!(
        "HTTP/1.1 {status} X\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        payload.len()
    );
    stream.write_all(head.as_bytes()).ok()?;
    stream.write_all(payload).ok()?;
    stream.flush().ok()?;

    // `POST /some/path HTTP/1.1` — the middle field.
    let target = request_line
        .split_whitespace()
        .nth(1)
        .unwrap_or_default()
        .to_string();

    Some(Captured {
        headers,
        body,
        target,
    })
}
