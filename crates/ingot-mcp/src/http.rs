//! MCP over Streamable HTTP.
//!
//! One endpoint, one `POST` per outgoing JSON-RPC message. The reply is either
//! `application/json` — one message — or `text/event-stream` — a sequence of
//! them, one per event's `data`. A notification is acknowledged with `202` and
//! no body, and yields nothing to read.
//!
//! # Nothing above this changes
//!
//! [`McpClient`](crate::McpClient) still writes a line and reads a line. The
//! handshake, the routing, the result conversion and the timeout rule are the
//! code that already existed for a child process, which is why [`Transport`]
//! was a trait from the first commit. The same discipline the streaming work
//! settled on: one parser, two transports.
//!
//! # A tool call is not retried
//!
//! An HTTP request *can* be retried where a write to a child's pipe cannot, and
//! this does not. `tools/call` is not idempotent — a server that sent mail and
//! then failed to answer must not be asked twice — so a failure is a failure.
//!
//! See [RFC-0019](../../../rfcs/0019-a-tool-server-that-is-not-a-child-process.md).

use std::collections::VecDeque;
use std::time::Duration;

use crate::transport::{Transport, TransportError};

/// Sent so a server can tell one client from another in its logs.
const USER_AGENT: &str = concat!("ingot-mcp/", env!("CARGO_PKG_VERSION"));

/// The header a server uses to bind requests into one session.
const SESSION_HEADER: &str = "mcp-session-id";

/// How much of a failing response body to keep for a diagnostic.
///
/// Enough to show a JSON error object or an HTML error page's title, and little
/// enough that a server returning a megabyte of markup does not fill a terminal.
const DIAGNOSTIC_BYTES: usize = 400;

pub struct HttpTransport {
    url: String,
    /// The value of an `authorization` header, when the operator named a
    /// variable holding one.
    ///
    /// Read from the environment at connect time and never written anywhere: it
    /// does not reach a diagnostic, a run record, or `ingot tools`.
    authorization: Option<String>,
    timeout: Duration,
    /// Handed back by `initialize` and echoed on every later request, when the
    /// server uses one.
    session: Option<String>,
    /// Messages the last `send` produced, in arrival order.
    pending: VecDeque<String>,
    /// The last failure, for [`Transport::diagnostics`]. A remote server has no
    /// standard error, so this stands in for one.
    last_error: String,
    closed: bool,
}

impl HttpTransport {
    /// Connect nothing yet — the first `send` is the first request.
    ///
    /// `auth` is a value, not a variable name: reading the environment is the
    /// caller's job, so this module cannot be the place a credential is looked
    /// up by accident.
    pub fn new(url: &str, auth: Option<String>, timeout: Duration) -> HttpTransport {
        HttpTransport {
            url: url.to_string(),
            authorization: auth,
            timeout,
            session: None,
            pending: VecDeque::new(),
            last_error: String::new(),
            closed: false,
        }
    }

    fn post(&mut self, line: &str) -> Result<(), TransportError> {
        let mut request = ureq::post(&self.url)
            .config()
            .timeout_global(Some(self.timeout))
            .build()
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .header("user-agent", USER_AGENT);
        if let Some(value) = &self.authorization {
            request = request.header("authorization", value);
        }
        if let Some(session) = &self.session {
            request = request.header("mcp-session-id", session);
        }

        let mut response = match request.send(line) {
            Ok(response) => response,
            Err(ureq::Error::StatusCode(status)) => {
                self.last_error = describe_status(status);
                return Err(TransportError::Io(self.last_error.clone()));
            }
            Err(error) => {
                self.last_error = error.to_string();
                return Err(TransportError::Io(self.last_error.clone()));
            }
        };

        // A server assigns a session on the `initialize` reply and expects it
        // back on everything after. Kept on the first response that carries
        // one, and never overwritten by a later blank.
        if self.session.is_none() {
            if let Some(value) = response.headers().get(SESSION_HEADER) {
                if let Ok(value) = value.to_str() {
                    if !value.is_empty() {
                        self.session = Some(value.to_string());
                    }
                }
            }
        }

        let status = response.status().as_u16();
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .unwrap_or("")
            .to_ascii_lowercase();

        // 202 with no body is how a notification is acknowledged. There is
        // nothing to read, and waiting for something would hang the handshake.
        if status == 202 {
            return Ok(());
        }

        let body = response
            .body_mut()
            .read_to_string()
            .map_err(|error| TransportError::Io(error.to_string()))?;

        if content_type.starts_with("text/event-stream") {
            for message in event_stream_messages(&body) {
                self.pending.push_back(message);
            }
        } else if !body.trim().is_empty() {
            // One whole-body message. Newlines inside it would break the
            // line-oriented contract above, and a JSON-RPC message is one
            // object, so the whitespace goes.
            self.pending.push_back(compact(&body));
        }
        Ok(())
    }
}

impl Transport for HttpTransport {
    fn send(&mut self, line: &str) -> Result<(), TransportError> {
        if self.closed {
            return Err(TransportError::Closed(String::new()));
        }
        self.post(line)
    }

    fn recv(&mut self, _timeout: Duration) -> Result<String, TransportError> {
        // The exchange already happened: `send` posted and read the whole
        // reply. Waiting here would be waiting for a message no one is going to
        // send, so an empty queue is reported at once rather than after the
        // deadline. The deadline that matters is the request's, and `send`
        // applied it.
        match self.pending.pop_front() {
            Some(line) => Ok(line),
            None if self.closed => Err(TransportError::Closed(String::new())),
            None => Err(TransportError::Timeout),
        }
    }

    fn diagnostics(&self) -> String {
        self.last_error.clone()
    }

    fn shutdown(&mut self) {
        if self.closed {
            return;
        }
        self.closed = true;

        // Ending a session is what closing stdin means over this transport. A
        // server that does not implement it answers 405, which is not a
        // failure: there was nothing to end.
        let Some(session) = self.session.clone() else {
            return;
        };
        let mut request = ureq::delete(&self.url)
            .config()
            .timeout_global(Some(self.timeout))
            .build()
            .header("mcp-session-id", &session)
            .header("user-agent", USER_AGENT);
        if let Some(value) = &self.authorization {
            request = request.header("authorization", value);
        }
        let _ = request.call();
    }
}

impl Drop for HttpTransport {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// The `data` payload of each event in a `text/event-stream` body.
///
/// Split out from the request so the framing is testable without a socket, the
/// same way the provider layer does it.
pub(crate) fn event_stream_messages(body: &str) -> Vec<String> {
    let mut messages = Vec::new();
    let mut data = String::new();

    let flush = |data: &mut String, messages: &mut Vec<String>| {
        if !data.trim().is_empty() {
            messages.push(compact(data));
        }
        data.clear();
    };

    for line in body.lines() {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.is_empty() {
            flush(&mut data, &mut messages);
            continue;
        }
        // A comment is a keep-alive, not a message.
        if line.starts_with(':') {
            continue;
        }
        let (field, value) = match line.split_once(':') {
            Some((field, value)) => (field, value.strip_prefix(' ').unwrap_or(value)),
            None => (line, ""),
        };
        // Only `data` carries a message. `event`, `id` and `retry` are framing,
        // and an unknown field is ignored rather than guessed at.
        if field == "data" {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(value);
        }
    }
    // A stream that ends without a blank line still delivered its last event.
    flush(&mut data, &mut messages);
    messages
}

/// One JSON-RPC message on one line.
///
/// The transport contract above is line-oriented, and a server is free to send
/// pretty-printed JSON. Re-encoding is the only way to hold both; a body that
/// is not JSON is passed through so the client's own parser reports it.
fn compact(text: &str) -> String {
    match serde_json::from_str::<serde_json::Value>(text) {
        Ok(value) => value.to_string(),
        Err(_) => text.replace(['\r', '\n'], " "),
    }
}

fn describe_status(status: u16) -> String {
    let meaning = match status {
        400 => "the server rejected the message as malformed",
        401 => "the server refused the credentials",
        403 => "the credentials do not permit this",
        404 => "no MCP endpoint at this URL",
        405 => "the endpoint does not accept this method",
        406 => "the server cannot answer in JSON or an event stream",
        429 => "the server is rate-limiting this client",
        500..=599 => "the server failed",
        _ => "the server rejected the request",
    };
    format!("HTTP {status}: {meaning}")
}

/// Truncate a body for a diagnostic, on a character boundary.
#[allow(dead_code)]
fn snippet(body: &str) -> String {
    if body.len() <= DIAGNOSTIC_BYTES {
        return body.to_string();
    }
    let cut = body
        .char_indices()
        .map(|(index, _)| index)
        .take_while(|index| *index <= DIAGNOSTIC_BYTES)
        .last()
        .unwrap_or(0);
    format!("{}…", &body[..cut])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_json_reply_and_an_event_stream_reply_are_the_same_message() {
        // One parser, two transports: whatever framing a server chooses, the
        // client above sees one line.
        let whole = compact(r#"{"jsonrpc":"2.0","id":1,"result":{"ok":true}}"#);
        let streamed = event_stream_messages(
            "event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"ok\":true}}\n\n",
        );
        assert_eq!(streamed, vec![whole]);
    }

    #[test]
    fn a_stream_may_carry_several_messages() {
        let messages = event_stream_messages("data: {\"id\":1}\n\ndata: {\"id\":2}\n\n");
        assert_eq!(messages, vec![r#"{"id":1}"#, r#"{"id":2}"#]);
    }

    #[test]
    fn keep_alive_comments_and_crlf_are_tolerated() {
        let messages = event_stream_messages(": ping\r\ndata: {\"id\":1}\r\n\r\n");
        assert_eq!(messages, vec![r#"{"id":1}"#]);
    }

    #[test]
    fn a_multi_line_data_payload_is_one_message() {
        // The event-stream format joins repeated `data:` lines with newlines,
        // and a server may pretty-print. Both arrive as one line here.
        let messages = event_stream_messages("data: {\ndata:   \"id\": 1\ndata: }\n\n");
        assert_eq!(messages, vec![r#"{"id":1}"#]);
    }

    #[test]
    fn a_stream_that_ends_without_a_blank_line_still_delivers() {
        assert_eq!(
            event_stream_messages("data: {\"id\":1}"),
            vec![r#"{"id":1}"#]
        );
    }

    #[test]
    fn framing_fields_are_not_messages() {
        assert!(event_stream_messages("event: message\nid: 7\nretry: 100\n\n").is_empty());
    }

    #[test]
    fn a_body_that_is_not_json_is_passed_through_for_the_client_to_reject() {
        // Refusing here would report "not JSON" from the transport, which is
        // the wrong layer to say it: the client's parser has the message id and
        // the method name to put in the complaint.
        assert_eq!(compact("not json\nat all"), "not json at all");
    }

    #[test]
    fn every_status_gets_words_rather_than_a_number() {
        for status in [400, 401, 403, 404, 405, 406, 429, 503, 418] {
            let text = describe_status(status);
            assert!(text.contains(&status.to_string()), "{text}");
            assert!(text.len() > 10, "{text}");
        }
    }

    #[test]
    fn a_transport_with_no_reply_queued_says_so_at_once() {
        let mut transport =
            HttpTransport::new("http://127.0.0.1:1/mcp", None, Duration::from_secs(1));
        assert_eq!(
            transport.recv(Duration::from_secs(30)),
            Err(TransportError::Timeout)
        );
    }

    #[test]
    fn a_closed_transport_refuses_to_send() {
        let mut transport =
            HttpTransport::new("http://127.0.0.1:1/mcp", None, Duration::from_secs(1));
        transport.shutdown();
        assert!(matches!(
            transport.send("{}"),
            Err(TransportError::Closed(_))
        ));
    }
}
