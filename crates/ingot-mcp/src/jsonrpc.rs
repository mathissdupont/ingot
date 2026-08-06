//! JSON-RPC 2.0, framed the way MCP frames it on stdio.
//!
//! One JSON value per line, UTF-8, no embedded newlines. That framing is the
//! whole of the wire format, which is why this module is small. Both halves
//! live here — the client sends requests and the reference server answers
//! them — so the two can never drift apart.

use serde::Deserialize;
use serde_json::{json, Value};

pub const VERSION: &str = "2.0";

// The standard JSON-RPC codes. A server reports a malformed call with one of
// these; a tool that ran and failed reports itself with `isError` instead, and
// the distinction matters: the first is the caller's bug, the second is a
// result the agent may reasonably see.
pub const PARSE_ERROR: i64 = -32700;
pub const INVALID_REQUEST: i64 = -32600;
pub const METHOD_NOT_FOUND: i64 = -32601;
pub const INVALID_PARAMS: i64 = -32602;
pub const INTERNAL_ERROR: i64 = -32603;

static NULL: Value = Value::Null;

/// Any message read off the wire.
///
/// One permissive struct rather than an enum. A peer may interleave
/// notifications — progress, logging — with the response we are waiting for,
/// and a strict enum would turn "unexpected but harmless" into a protocol
/// failure.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Incoming {
    #[serde(default)]
    pub id: Option<Value>,
    #[serde(default)]
    pub method: Option<String>,
    #[serde(default)]
    pub params: Option<Value>,
    #[serde(default)]
    pub result: Option<Value>,
    #[serde(default)]
    pub error: Option<RpcError>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    #[serde(default)]
    pub data: Option<Value>,
}

impl Incoming {
    pub fn parse(line: &str) -> Result<Incoming, String> {
        serde_json::from_str(line).map_err(|error| error.to_string())
    }

    /// A call that expects a reply.
    pub fn is_request(&self) -> bool {
        self.method.is_some() && self.id.is_some()
    }

    /// A call that expects no reply. Answering one is a protocol violation.
    pub fn is_notification(&self) -> bool {
        self.method.is_some() && self.id.is_none()
    }

    /// A reply to something we sent.
    pub fn is_response(&self) -> bool {
        self.method.is_none() && self.id.is_some()
    }

    pub fn method(&self) -> &str {
        self.method.as_deref().unwrap_or_default()
    }

    pub fn params(&self) -> &Value {
        self.params.as_ref().unwrap_or(&NULL)
    }
}

fn line(value: Value) -> String {
    serde_json::to_string(&value).expect("JSON-RPC envelopes are always serializable")
}

pub fn request_line(id: u64, method: &str, params: Value) -> String {
    line(json!({ "jsonrpc": VERSION, "id": id, "method": method, "params": params }))
}

pub fn notification_line(method: &str, params: Value) -> String {
    line(json!({ "jsonrpc": VERSION, "method": method, "params": params }))
}

pub fn result_line(id: &Value, result: Value) -> String {
    line(json!({ "jsonrpc": VERSION, "id": id, "result": result }))
}

pub fn error_line(id: Option<&Value>, code: i64, message: &str) -> String {
    line(json!({
        "jsonrpc": VERSION,
        "id": id.cloned().unwrap_or(Value::Null),
        "error": { "code": code, "message": message },
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_response_is_told_apart_from_a_notification() {
        let response = Incoming::parse(r#"{"jsonrpc":"2.0","id":1,"result":{}}"#).unwrap();
        assert!(response.is_response());
        assert!(!response.is_notification());

        let note =
            Incoming::parse(r#"{"jsonrpc":"2.0","method":"notifications/message"}"#).unwrap();
        assert!(note.is_notification());
        assert!(!note.is_response());
        assert!(!note.is_request());

        let request = Incoming::parse(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#).unwrap();
        assert!(request.is_request());
    }

    #[test]
    fn an_error_reply_carries_its_code() {
        let message =
            Incoming::parse(r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"nope"}}"#)
                .unwrap();
        let error = message.error.expect("an error reply");
        assert_eq!(error.code, METHOD_NOT_FOUND);
        assert_eq!(error.message, "nope");
    }

    #[test]
    fn envelopes_are_single_line() {
        let text = request_line(7, "tools/call", json!({"name": "a\nb"}));
        assert_eq!(text.lines().count(), 1, "{text}");
        assert!(text.contains("\\n"), "the newline must be escaped: {text}");
        let parsed = Incoming::parse(&text).unwrap();
        assert_eq!(parsed.id, Some(json!(7)));
        assert_eq!(parsed.method(), "tools/call");
    }

    #[test]
    fn a_missing_params_object_reads_as_null_rather_than_panicking() {
        let message = Incoming::parse(r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#).unwrap();
        assert!(message.params().is_null());
    }

    #[test]
    fn garbage_is_reported_rather_than_swallowed() {
        assert!(Incoming::parse("not json").is_err());
    }
}
