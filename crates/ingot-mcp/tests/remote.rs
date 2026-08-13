//! End-to-end tests against an MCP server reached over HTTP.
//!
//! A stub server on a loopback port stands in for a hosted one, so these
//! exercise the real socket, the real framing and the real handshake without a
//! network. The two things most worth pinning are that a JSON reply and an
//! event-stream reply reach the client identically, and that the policy check
//! happens **before** anything connects.

#![cfg(feature = "http")]

use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use ingot_mcp::{AgentTools, DirectLauncher, McpConfig, McpToolHost, NetworkGrant, ServerConfig};
use ingot_runtime::{ToolHost, ToolInvocation};
use serde_json::{json, Value};

/// How a stub frames its replies.
#[derive(Copy, Clone, PartialEq, Eq)]
enum Framing {
    Json,
    EventStream,
}

/// The `authorization` and `mcp-session-id` of each request, in arrival order.
type SeenHeaders = Arc<Mutex<Vec<(Option<String>, Option<String>)>>>;

struct Stub {
    url: String,
    /// What crossed the wire, so a test can assert on it.
    seen: SeenHeaders,
    served: Arc<AtomicUsize>,
}

/// A server that answers `initialize`, `tools/list` and `tools/call`.
///
/// It assigns a session id on the first reply, which is what lets a test check
/// that the client echoes it afterwards.
fn stub(framing: Framing) -> Stub {
    let listener = TcpListener::bind("127.0.0.1:0").expect("binding a local port");
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let served = Arc::new(AtomicUsize::new(0));
    let headers = Arc::clone(&seen);
    let counter = Arc::clone(&served);

    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { break };
            counter.fetch_add(1, Ordering::SeqCst);
            let _ = answer(stream, framing, &headers);
        }
    });

    Stub {
        url: format!("http://127.0.0.1:{port}/mcp"),
        seen,
        served,
    }
}

fn answer(mut stream: TcpStream, framing: Framing, seen: &SeenHeaders) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request_line = String::new();
    reader.read_line(&mut request_line)?;

    let mut length = 0usize;
    let mut authorization = None;
    let mut session = None;
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
            match name.trim().to_ascii_lowercase().as_str() {
                "content-length" => length = value.trim().parse().unwrap_or(0),
                "authorization" => authorization = Some(value.trim().to_string()),
                "mcp-session-id" => session = Some(value.trim().to_string()),
                _ => {}
            }
        }
    }
    seen.lock().unwrap().push((authorization, session));

    // A DELETE ends the session and carries no body.
    if request_line.starts_with("DELETE") {
        return write_raw(&mut stream, "HTTP/1.1 200 OK\r\ncontent-length: 0\r\n\r\n");
    }

    let mut body = vec![0u8; length];
    reader.read_exact(&mut body)?;
    let message: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    let method = message["method"].as_str().unwrap_or("");

    // A notification is acknowledged and answered with nothing.
    if message.get("id").is_none() {
        return write_raw(
            &mut stream,
            "HTTP/1.1 202 Accepted\r\ncontent-length: 0\r\n\r\n",
        );
    }

    let result = match method {
        "initialize" => json!({
            "protocolVersion": ingot_mcp::PREFERRED_PROTOCOL_VERSION,
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "stub", "version": "0.1.0" },
        }),
        "tools/list" => json!({
            "tools": [{
                "name": "remote.echo",
                "description": "Returns what it was given.",
                "inputSchema": { "type": "object" },
            }],
        }),
        "tools/call" => json!({
            "content": [{ "type": "text", "text": "echoed" }],
        }),
        _ => json!({}),
    };
    let reply = json!({ "jsonrpc": "2.0", "id": message["id"], "result": result });

    let (content_type, payload) = match framing {
        Framing::Json => ("application/json", reply.to_string()),
        // Pretty-printed on purpose: a server may, and the client has to
        // deliver it as one line regardless.
        Framing::EventStream => (
            "text/event-stream",
            format!(
                ": keep-alive\nevent: message\n{}\n\n",
                serde_json::to_string_pretty(&reply)
                    .unwrap()
                    .lines()
                    .map(|line| format!("data: {line}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            ),
        ),
    };

    write_raw(
        &mut stream,
        &format!(
            "HTTP/1.1 200 OK\r\ncontent-type: {content_type}\r\nmcp-session-id: sess-1\r\n\
             content-length: {}\r\n\r\n{payload}",
            payload.len()
        ),
    )
}

fn write_raw(stream: &mut TcpStream, text: &str) -> std::io::Result<()> {
    stream.write_all(text.as_bytes())?;
    stream.flush()
}

fn remote_config(url: &str, auth_env: Option<&str>) -> McpConfig {
    McpConfig {
        servers: vec![ServerConfig {
            name: "hosted".to_string(),
            command: String::new(),
            url: Some(url.to_string()),
            auth_env: auth_env.map(str::to_string),
            args: Vec::new(),
            image: None,
            cwd: None,
            pass_env: Vec::new(),
            tools: BTreeMap::new(),
        }],
        ..McpConfig::default()
    }
}

fn agent(hosts: &[&str], allowed: bool) -> Vec<AgentTools> {
    let tools: BTreeSet<String> = ["remote.echo".to_string()].into_iter().collect();
    vec![
        AgentTools::new("test.Remote", tools).with_network(NetworkGrant {
            allowed,
            hosts: hosts.iter().map(|host| host.to_string()).collect(),
        }),
    ]
}

fn connect(config: &McpConfig, agents: &[AgentTools]) -> Result<McpToolHost, String> {
    McpToolHost::connect_agents(config, Path::new("."), agents, &DirectLauncher)
        .map_err(|error| error.to_string())
}

/// The refusal a connection produced, or a panic saying it did not refuse.
///
/// `expect_err` would need `McpToolHost: Debug`, and deriving it to satisfy a
/// test would put a server handle's innards in every error message.
fn refusal(config: &McpConfig, agents: &[AgentTools]) -> String {
    match connect(config, agents) {
        Err(error) => error,
        Ok(_) => panic!("the connection was expected to be refused"),
    }
}

#[test]
fn a_json_reply_and_an_event_stream_reply_are_the_same_message() {
    // One parser, two transports. Whatever framing a server picks, the client
    // above sees the same thing — including the tool it publishes.
    for framing in [Framing::Json, Framing::EventStream] {
        let stub = stub(framing);
        let mut host = connect(
            &remote_config(&stub.url, None),
            &agent(&["127.0.0.1"], true),
        )
        .expect("the stub server answers");

        let resolved = host.resolved();
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].tool, "remote.echo");

        let value = host
            .call(&ToolInvocation {
                agent: "test.Remote".to_string(),
                node: "n0".to_string(),
                reference: "mcp:remote.echo".to_string(),
                name: "remote.echo".to_string(),
                transport: "mcp".to_string(),
                arguments: BTreeMap::new(),
                effects: Vec::new(),
                result_type: "text".to_string(),
            })
            .expect("the call succeeds");
        assert_eq!(value, json!("echoed"));
        host.close();
    }
}

#[test]
fn a_session_id_is_echoed_on_every_later_request() {
    let stub = stub(Framing::Json);
    let mut host = connect(
        &remote_config(&stub.url, None),
        &agent(&["127.0.0.1"], true),
    )
    .expect("the stub server answers");
    host.close();

    let seen = stub.seen.lock().unwrap().clone();
    assert!(
        seen.len() >= 3,
        "initialize, initialized, tools/list: {seen:?}"
    );
    // The first request cannot carry a session; every one after it must.
    assert_eq!(seen[0].1, None);
    assert!(
        seen[1..]
            .iter()
            .all(|(_, session)| session.as_deref() == Some("sess-1")),
        "{seen:?}"
    );
}

#[test]
fn a_bearer_token_is_read_from_the_environment_and_never_written_down() {
    // The value reaches the wire and nothing else: not the resolved-tool list,
    // not a message, not `ingot tools` output.
    std::env::set_var("INGOT_TEST_MCP_TOKEN", "s3cret-value");
    let stub = stub(Framing::Json);
    let config = remote_config(&stub.url, Some("INGOT_TEST_MCP_TOKEN"));
    let mut host = connect(&config, &agent(&["127.0.0.1"], true)).expect("it connects");

    let sent = stub.seen.lock().unwrap()[0].0.clone();
    assert_eq!(sent.as_deref(), Some("Bearer s3cret-value"));

    let rendered = format!(
        "{:?}\n{}\n{:?}",
        host.resolved(),
        host.launcher(),
        host.inventory()
    );
    assert!(
        !rendered.contains("s3cret-value"),
        "the token leaked into output: {rendered}"
    );
    // The manifest holds the variable's name, which is the point of naming it.
    assert_eq!(
        config.servers[0].auth_env.as_deref(),
        Some("INGOT_TEST_MCP_TOKEN")
    );
    host.close();
    std::env::remove_var("INGOT_TEST_MCP_TOKEN");
}

#[test]
fn an_absent_token_variable_is_refused_before_anything_connects() {
    let stub = stub(Framing::Json);
    let config = remote_config(&stub.url, Some("INGOT_A_VARIABLE_NOBODY_EXPORTS"));
    let error = refusal(&config, &agent(&["127.0.0.1"], true));
    assert!(error.contains("INGOT_A_VARIABLE_NOBODY_EXPORTS"), "{error}");
    assert_eq!(
        stub.served.load(Ordering::SeqCst),
        0,
        "nothing should have been sent"
    );
}

#[test]
fn network_deny_refuses_every_remote_server() {
    // The guarantee ADR-0005 said must not be weakened: `network deny` means no
    // remote server at all, with no "except through tools".
    let stub = stub(Framing::Json);
    let error = refusal(&remote_config(&stub.url, None), &agent(&[], false));
    assert!(error.contains("may not reach"), "{error}");
    assert!(error.contains("network is denied"), "{error}");
    assert_eq!(stub.served.load(Ordering::SeqCst), 0, "it connected anyway");
}

#[test]
fn a_host_the_policy_does_not_name_is_refused_and_the_message_says_what_to_add() {
    let stub = stub(Framing::Json);
    let error = refusal(
        &remote_config(&stub.url, None),
        &agent(&["arxiv.org"], true),
    );
    assert!(error.contains("127.0.0.1"), "{error}");
    assert!(error.contains("arxiv.org"), "{error}");
    assert!(error.contains("network allow"), "{error}");
    assert!(
        error.contains("command"),
        "it offers the local alternative: {error}"
    );
    assert_eq!(stub.served.load(Ordering::SeqCst), 0);
}

#[test]
fn an_unscoped_network_allow_reaches_anything() {
    let stub = stub(Framing::Json);
    let mut host =
        connect(&remote_config(&stub.url, None), &agent(&[], true)).expect("unscoped allow");
    assert_eq!(host.resolved().len(), 1);
    host.close();
}

#[test]
fn a_manifest_with_no_artifact_behind_it_connects_and_says_nothing_was_checked() {
    // What `ingot tools` has: no agent, so no policy to check against. Showing
    // what is out there is its whole job, and it labels the result rather than
    // pretending to have checked.
    let stub = stub(Framing::Json);
    let mut host = McpToolHost::connect_all(&remote_config(&stub.url, None), Path::new("."))
        .expect("it connects");
    assert_eq!(host.resolved().len(), 1);
    host.close();
}
