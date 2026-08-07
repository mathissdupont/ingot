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

fn handle(mut stream: TcpStream, status: u16, response: &Value) -> Option<Captured> {
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

    let payload = serde_json::to_vec(response).ok()?;
    let head = format!(
        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        payload.len()
    );
    stream.write_all(head.as_bytes()).ok()?;
    stream.write_all(&payload).ok()?;
    stream.flush().ok()?;

    Some(Captured { headers, body })
}
