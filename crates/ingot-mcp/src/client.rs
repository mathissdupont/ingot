//! An MCP client: handshake, tool discovery, tool invocation.
//!
//! Only the tools half of the protocol is implemented. Ingot has no use for
//! prompts or sampling — an agent's prompts are compiled into its artifact —
//! and resources are a capability the language does not yet express. Anything
//! not implemented is not silently ignored: an unexpected reply is an error.

use std::fmt;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{json, Value};

use crate::jsonrpc::{self, Incoming};
use crate::transport::{Transport, TransportError};

/// Protocol revisions this client understands, newest first.
pub const SUPPORTED_PROTOCOL_VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

/// What the client asks for. A server may answer with an older revision from
/// the list above, which is the negotiation the specification describes.
pub const PREFERRED_PROTOCOL_VERSION: &str = "2025-06-18";

const CLIENT_NAME: &str = "ingot";

/// How many unrelated messages may arrive while waiting for a reply before the
/// client decides the peer is not talking sense. Notifications are normal; an
/// unbounded stream of them is not.
const MAX_INTERLEAVED: usize = 64;

#[derive(Debug, Clone, PartialEq)]
pub enum McpError {
    /// The connection itself failed.
    Transport {
        server: String,
        reason: String,
        stderr: String,
    },
    /// The server answered, but not with anything the protocol allows.
    Protocol { server: String, reason: String },
    /// The server returned a JSON-RPC error.
    Rpc {
        server: String,
        method: String,
        code: i64,
        message: String,
    },
    /// The server did not answer within the configured timeout.
    Timeout {
        server: String,
        method: String,
        timeout: Duration,
    },
    /// No protocol revision in common.
    UnsupportedProtocol { server: String, offered: String },
    /// The operator's configuration cannot be satisfied.
    Configuration(String),
}

impl fmt::Display for McpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            McpError::Transport {
                server,
                reason,
                stderr,
            } => {
                write!(f, "MCP server `{server}`: {reason}")?;
                if !stderr.is_empty() {
                    write!(f, "\n  its standard error said:\n{}", indent(stderr))?;
                }
                Ok(())
            }
            McpError::Protocol { server, reason } => {
                write!(f, "MCP server `{server}` broke the protocol: {reason}")
            }
            McpError::Rpc {
                server,
                method,
                code,
                message,
            } => write!(
                f,
                "MCP server `{server}` refused `{method}`: {message} (code {code})"
            ),
            McpError::Timeout {
                server,
                method,
                timeout,
            } => write!(
                f,
                "MCP server `{server}` did not answer `{method}` within {}s",
                timeout.as_secs()
            ),
            McpError::UnsupportedProtocol { server, offered } => write!(
                f,
                "MCP server `{server}` speaks protocol `{offered}`; this client implements {}",
                SUPPORTED_PROTOCOL_VERSIONS.join(", ")
            ),
            McpError::Configuration(reason) => f.write_str(reason),
        }
    }
}

impl std::error::Error for McpError {}

fn indent(text: &str) -> String {
    text.lines()
        .map(|line| format!("    {line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// What a server said about itself during the handshake.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerInfo {
    pub name: String,
    pub version: String,
    pub protocol_version: String,
    /// Whether the server declared the `tools` capability.
    pub serves_tools: bool,
}

/// One tool a server publishes.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolDescriptor {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub input_schema: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<Value>,
}

/// A block of a tool result.
#[derive(Debug, Clone, PartialEq)]
pub enum ContentBlock {
    Text(String),
    /// Images, audio and embedded resources. Ingot has no type that can carry
    /// them yet, so they are kept only to make the error message specific.
    Other(String),
}

/// What `tools/call` returned.
#[derive(Debug, Clone, PartialEq)]
pub struct CallOutcome {
    pub content: Vec<ContentBlock>,
    /// The `structuredContent` field, when the server produced one.
    pub structured: Option<Value>,
    /// The tool ran and reported failure. Distinct from a JSON-RPC error, which
    /// means the call itself was malformed.
    pub is_error: bool,
}

impl CallOutcome {
    /// Every text block, joined. The conventional way to read a prose result.
    pub fn text(&self) -> String {
        self.content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text(text) => Some(text.as_str()),
                ContentBlock::Other(_) => None,
            })
            .collect::<Vec<_>>()
            .join("")
    }

    /// The kinds of non-text block present, for an error message.
    pub fn non_text_kinds(&self) -> Vec<String> {
        let mut kinds: Vec<String> = self
            .content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Other(kind) => Some(kind.clone()),
                ContentBlock::Text(_) => None,
            })
            .collect();
        kinds.sort();
        kinds.dedup();
        kinds
    }
}

pub struct McpClient {
    server: String,
    transport: Box<dyn Transport + Send>,
    timeout: Duration,
    next_id: u64,
    info: Option<ServerInfo>,
}

impl McpClient {
    pub fn new(
        server: impl Into<String>,
        transport: Box<dyn Transport + Send>,
        timeout: Duration,
    ) -> Self {
        McpClient {
            server: server.into(),
            transport,
            timeout,
            next_id: 1,
            info: None,
        }
    }

    pub fn server_name(&self) -> &str {
        &self.server
    }

    pub fn info(&self) -> Option<&ServerInfo> {
        self.info.as_ref()
    }

    /// Perform the handshake. Must be called before anything else.
    pub fn initialize(&mut self) -> Result<&ServerInfo, McpError> {
        let result = self.request(
            "initialize",
            json!({
                "protocolVersion": PREFERRED_PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": { "name": CLIENT_NAME, "version": env!("CARGO_PKG_VERSION") },
            }),
        )?;

        let protocol_version = result
            .get("protocolVersion")
            .and_then(Value::as_str)
            .ok_or_else(|| self.protocol_error("`initialize` returned no `protocolVersion`"))?
            .to_string();
        if !SUPPORTED_PROTOCOL_VERSIONS.contains(&protocol_version.as_str()) {
            return Err(McpError::UnsupportedProtocol {
                server: self.server.clone(),
                offered: protocol_version,
            });
        }

        let serves_tools = result
            .get("capabilities")
            .and_then(|capabilities| capabilities.get("tools"))
            .is_some();
        let info = result.get("serverInfo");
        let read = |field: &str| {
            info.and_then(|info| info.get(field))
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_string()
        };

        self.info = Some(ServerInfo {
            name: read("name"),
            version: read("version"),
            protocol_version,
            serves_tools,
        });

        // The specification requires this before any other request. Servers
        // that do not need it ignore it; servers that do need it hang without.
        self.notify("notifications/initialized", json!({}))?;
        Ok(self.info.as_ref().expect("just assigned"))
    }

    /// Every tool the server publishes, following pagination to the end.
    pub fn list_tools(&mut self) -> Result<Vec<ToolDescriptor>, McpError> {
        let mut tools = Vec::new();
        let mut cursor: Option<String> = None;
        // A server that returns the same cursor forever would otherwise loop.
        let mut pages = 0;

        loop {
            let params = match &cursor {
                Some(cursor) => json!({ "cursor": cursor }),
                None => json!({}),
            };
            let result = self.request("tools/list", params)?;
            let listed = result
                .get("tools")
                .and_then(Value::as_array)
                .ok_or_else(|| self.protocol_error("`tools/list` returned no `tools` array"))?;

            for entry in listed {
                let name = entry
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or_else(|| self.protocol_error("a listed tool has no `name`"))?;
                tools.push(ToolDescriptor {
                    name: name.to_string(),
                    description: entry
                        .get("description")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    input_schema: entry.get("inputSchema").cloned().unwrap_or(Value::Null),
                    output_schema: entry.get("outputSchema").cloned(),
                });
            }

            let next = result
                .get("nextCursor")
                .and_then(Value::as_str)
                .map(str::to_string);
            pages += 1;
            match next {
                Some(next) if Some(&next) != cursor.as_ref() && pages < 100 => {
                    cursor = Some(next);
                }
                Some(_) => {
                    return Err(
                        self.protocol_error("`tools/list` paginates without making progress")
                    )
                }
                None => return Ok(tools),
            }
        }
    }

    /// Invoke a tool by the name the server publishes it under.
    pub fn call_tool(&mut self, name: &str, arguments: Value) -> Result<CallOutcome, McpError> {
        let result = self.request(
            "tools/call",
            json!({ "name": name, "arguments": arguments }),
        )?;

        let mut content = Vec::new();
        if let Some(blocks) = result.get("content").and_then(Value::as_array) {
            for block in blocks {
                let kind = block.get("type").and_then(Value::as_str).unwrap_or("");
                match kind {
                    "text" => content.push(ContentBlock::Text(
                        block
                            .get("text")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                    )),
                    other => content.push(ContentBlock::Other(other.to_string())),
                }
            }
        }

        Ok(CallOutcome {
            content,
            structured: result.get("structuredContent").cloned(),
            is_error: result
                .get("isError")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        })
    }

    pub fn close(&mut self) {
        self.transport.shutdown();
    }

    // --- plumbing ----------------------------------------------------------

    fn request(&mut self, method: &str, params: Value) -> Result<Value, McpError> {
        let id = self.next_id;
        self.next_id += 1;

        self.transport
            .send(&jsonrpc::request_line(id, method, params))
            .map_err(|error| self.transport_error(error))?;

        let deadline = Instant::now() + self.timeout;
        for _ in 0..MAX_INTERLEAVED {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(self.timeout_error(method));
            }

            let line = match self.transport.recv(remaining) {
                Ok(line) => line,
                Err(TransportError::Timeout) => return Err(self.timeout_error(method)),
                Err(error) => return Err(self.transport_error(error)),
            };

            let message = Incoming::parse(&line).map_err(|reason| {
                self.protocol_error(&format!("sent something that is not JSON: {reason}"))
            })?;

            // A server may send notifications, and may send a request of its
            // own. Ingot advertises no capabilities, so any request it makes is
            // one we did not agree to serve: refuse it and carry on waiting.
            if message.is_notification() {
                continue;
            }
            if message.is_request() {
                let refusal = jsonrpc::error_line(
                    message.id.as_ref(),
                    jsonrpc::METHOD_NOT_FOUND,
                    "this client advertises no capabilities",
                );
                self.transport
                    .send(&refusal)
                    .map_err(|error| self.transport_error(error))?;
                continue;
            }
            if message.id.as_ref() != Some(&Value::from(id)) {
                // A reply to a request that already timed out. Skip it rather
                // than mistaking it for the answer to this one.
                continue;
            }

            if let Some(error) = message.error {
                return Err(McpError::Rpc {
                    server: self.server.clone(),
                    method: method.to_string(),
                    code: error.code,
                    message: error.message,
                });
            }
            return message.result.ok_or_else(|| {
                self.protocol_error(&format!(
                    "answered `{method}` with neither result nor error"
                ))
            });
        }

        Err(self.protocol_error(&format!(
            "sent {MAX_INTERLEAVED} unrelated messages while `{method}` was outstanding"
        )))
    }

    fn notify(&mut self, method: &str, params: Value) -> Result<(), McpError> {
        self.transport
            .send(&jsonrpc::notification_line(method, params))
            .map_err(|error| self.transport_error(error))
    }

    fn transport_error(&self, error: TransportError) -> McpError {
        McpError::Transport {
            server: self.server.clone(),
            reason: error.to_string(),
            stderr: self.transport.diagnostics(),
        }
    }

    fn protocol_error(&self, reason: &str) -> McpError {
        McpError::Protocol {
            server: self.server.clone(),
            reason: reason.to_string(),
        }
    }

    fn timeout_error(&self, method: &str) -> McpError {
        McpError::Timeout {
            server: self.server.clone(),
            method: method.to_string(),
            timeout: self.timeout,
        }
    }
}

impl Drop for McpClient {
    fn drop(&mut self) {
        self.transport.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::LoopbackTransport;

    fn id_of(line: &str) -> Value {
        Incoming::parse(line).unwrap().id.unwrap_or(Value::Null)
    }

    fn client(handler: impl FnMut(&str) -> Vec<String> + Send + 'static) -> McpClient {
        McpClient::new(
            "test",
            Box::new(LoopbackTransport::new(handler)),
            Duration::from_millis(50),
        )
    }

    fn handshake_reply(line: &str) -> String {
        jsonrpc::result_line(
            &id_of(line),
            json!({
                "protocolVersion": PREFERRED_PROTOCOL_VERSION,
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "stub", "version": "1.2.3" },
            }),
        )
    }

    #[test]
    fn the_handshake_records_what_the_server_said() {
        let mut client = client(|line| {
            let message = Incoming::parse(line).unwrap();
            match message.method() {
                "initialize" => vec![handshake_reply(line)],
                "notifications/initialized" => vec![],
                other => panic!("unexpected method {other}"),
            }
        });

        let info = client.initialize().unwrap().clone();
        assert_eq!(info.name, "stub");
        assert_eq!(info.version, "1.2.3");
        assert_eq!(info.protocol_version, PREFERRED_PROTOCOL_VERSION);
        assert!(info.serves_tools);
    }

    #[test]
    fn a_protocol_revision_we_do_not_implement_is_refused() {
        let mut client = client(|line| {
            vec![jsonrpc::result_line(
                &id_of(line),
                json!({ "protocolVersion": "1999-01-01", "capabilities": {} }),
            )]
        });
        let error = client.initialize().unwrap_err();
        assert!(
            matches!(error, McpError::UnsupportedProtocol { ref offered, .. } if offered == "1999-01-01"),
            "{error}"
        );
    }

    #[test]
    fn an_older_but_known_revision_is_accepted() {
        let mut client = client(|line| {
            let message = Incoming::parse(line).unwrap();
            if message.method() == "initialize" {
                vec![jsonrpc::result_line(
                    &id_of(line),
                    json!({ "protocolVersion": "2024-11-05", "capabilities": { "tools": {} } }),
                )]
            } else {
                vec![]
            }
        });
        assert_eq!(client.initialize().unwrap().protocol_version, "2024-11-05");
    }

    #[test]
    fn notifications_arriving_mid_request_are_skipped() {
        let mut client = client(|line| {
            let message = Incoming::parse(line).unwrap();
            match message.method() {
                "initialize" => vec![
                    jsonrpc::notification_line("notifications/message", json!({"level": "info"})),
                    handshake_reply(line),
                ],
                _ => vec![],
            }
        });
        assert_eq!(client.initialize().unwrap().name, "stub");
    }

    #[test]
    fn pagination_is_followed_to_the_end() {
        let mut page = 0;
        let mut client = client(move |line| {
            let message = Incoming::parse(line).unwrap();
            if message.method() != "tools/list" {
                return vec![];
            }
            page += 1;
            let result = if page == 1 {
                json!({
                    "tools": [{"name": "a", "inputSchema": {}}],
                    "nextCursor": "next",
                })
            } else {
                json!({ "tools": [{"name": "b", "inputSchema": {}}] })
            };
            vec![jsonrpc::result_line(&id_of(line), result)]
        });

        let tools = client.list_tools().unwrap();
        assert_eq!(
            tools
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>(),
            vec!["a", "b"]
        );
    }

    #[test]
    fn discovery_keeps_output_schemas_for_authoring_clients() {
        let mut client = client(|line| {
            vec![jsonrpc::result_line(
                &id_of(line),
                json!({
                    "tools": [{
                        "name": "count",
                        "inputSchema": {"type": "object"},
                        "outputSchema": {"type": "object", "properties": {"value": {"type": "integer"}}},
                    }],
                }),
            )]
        });

        let tools = client.list_tools().unwrap();
        assert_eq!(
            tools[0].output_schema,
            Some(json!({"type": "object", "properties": {"value": {"type": "integer"}}}))
        );
    }

    #[test]
    fn a_cursor_that_never_advances_is_an_error_rather_than_a_hang() {
        let mut client = client(|line| {
            vec![jsonrpc::result_line(
                &id_of(line),
                json!({ "tools": [], "nextCursor": "same" }),
            )]
        });
        let error = client.list_tools().unwrap_err();
        assert!(
            matches!(error, McpError::Protocol { ref reason, .. } if reason.contains("progress")),
            "{error}"
        );
    }

    #[test]
    fn a_json_rpc_error_names_the_method_and_the_code() {
        let mut client = client(|line| {
            vec![jsonrpc::error_line(
                Some(&id_of(line)),
                jsonrpc::INVALID_PARAMS,
                "`path` is required",
            )]
        });
        let error = client.call_tool("fs.read_file", json!({})).unwrap_err();
        assert!(error.to_string().contains("tools/call"), "{error}");
        assert!(error.to_string().contains("`path` is required"), "{error}");
    }

    #[test]
    fn a_tool_result_separates_text_from_structure() {
        let mut client = client(|line| {
            vec![jsonrpc::result_line(
                &id_of(line),
                json!({
                    "content": [
                        {"type": "text", "text": "hello"},
                        {"type": "image", "data": "..."},
                    ],
                    "structuredContent": {"value": 3},
                }),
            )]
        });
        let outcome = client.call_tool("x", json!({})).unwrap();
        assert_eq!(outcome.text(), "hello");
        assert_eq!(outcome.non_text_kinds(), vec!["image".to_string()]);
        assert_eq!(outcome.structured, Some(json!({"value": 3})));
        assert!(!outcome.is_error);
    }

    #[test]
    fn a_failed_tool_is_not_a_failed_call() {
        let mut client = client(|line| {
            vec![jsonrpc::result_line(
                &id_of(line),
                json!({ "content": [{"type": "text", "text": "no such file"}], "isError": true }),
            )]
        });
        let outcome = client.call_tool("fs.read_file", json!({})).unwrap();
        assert!(outcome.is_error);
        assert_eq!(outcome.text(), "no such file");
    }

    #[test]
    fn a_silent_server_times_out_with_the_method_named() {
        let mut client = client(|_| vec![]);
        let error = client.list_tools().unwrap_err();
        assert!(
            matches!(error, McpError::Timeout { ref method, .. } if method == "tools/list"),
            "{error}"
        );
    }

    #[test]
    fn a_dead_server_reports_its_standard_error() {
        let transport =
            LoopbackTransport::new(|_| vec![]).with_stderr("Traceback: ModuleNotFoundError");
        let mut transport = transport;
        transport.shutdown();
        let mut client = McpClient::new("broken", Box::new(transport), Duration::from_millis(10));

        let error = client.initialize().unwrap_err();
        let text = error.to_string();
        assert!(text.contains("broken"), "{text}");
        assert!(text.contains("ModuleNotFoundError"), "{text}");
    }
}
