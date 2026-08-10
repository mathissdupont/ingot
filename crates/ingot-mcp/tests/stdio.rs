//! End-to-end tests against a real MCP server in a real subprocess.
//!
//! The unit tests drive the protocol through an in-process transport, which
//! covers the message handling but not the part most likely to be wrong: pipes,
//! framing, process lifetime and the handshake between two independently
//! compiled programs. These start `ingot-mcp-fs` and talk to it the way
//! `ingot run` does.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use ingot_mcp::{McpConfig, McpToolHost, ServerConfig};
use ingot_runtime::{ToolError, ToolHost, ToolInvocation};
use serde_json::{json, Value};

fn server() -> String {
    let exe_name = format!("ingot-mcp-fs{}", std::env::consts::EXE_SUFFIX);
    if let Ok(current) = std::env::current_exe() {
        if let Some(debug_dir) = current.parent().and_then(Path::parent) {
            let sibling = debug_dir.join(&exe_name);
            if sibling.exists() {
                return sibling.display().to_string();
            }
        }
    }
    env!("CARGO_BIN_EXE_ingot-mcp-fs").to_string()
}

fn workspace(label: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("ingot-mcp-stdio-{label}"));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("creating a temporary workspace");
    root.canonicalize().expect("canonicalising it")
}

fn config(root: &Path, allow_write: bool, tools: &[(&str, &str)]) -> McpConfig {
    let mut args = vec!["--root".to_string(), root.display().to_string()];
    if allow_write {
        args.push("--allow-write".to_string());
    }

    McpConfig {
        servers: vec![ServerConfig {
            name: "files".to_string(),
            command: server(),
            args,
            image: None,
            cwd: None,
            pass_env: Vec::new(),
            tools: tools
                .iter()
                .map(|(tool, remote)| (tool.to_string(), remote.to_string()))
                .collect(),
        }],
        // Short: a hung server should fail a test quickly, not hold the suite.
        timeout_seconds: 10,
    }
}

fn required(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|name| name.to_string()).collect()
}

fn invocation(name: &str, result_type: &str, arguments: &[(&str, Value)]) -> ToolInvocation {
    ToolInvocation {
        node: "n0".to_string(),
        agent: "test.Agent".to_string(),
        reference: format!("mcp:{name}"),
        name: name.to_string(),
        transport: "mcp".to_string(),
        arguments: arguments
            .iter()
            .map(|(name, value)| ((*name).to_string(), value.clone()))
            .collect::<BTreeMap<String, Value>>(),
        effects: vec!["filesystem_read".to_string()],
        result_type: result_type.to_string(),
    }
}

#[test]
fn a_real_server_is_discovered_and_its_tools_are_routed() {
    let root = workspace("discover");
    let config = config(&root, false, &[]);

    let host = McpToolHost::connect(
        &config,
        &root,
        &required(&["fs.read_file", "fs.list_dir", "fs.write_file"]),
    )
    .expect("the server must start");

    let resolved: Vec<String> = host.resolved().into_iter().map(|tool| tool.tool).collect();
    assert_eq!(
        resolved,
        vec!["fs.list_dir".to_string(), "fs.read_file".to_string()],
        "a read-only server publishes no write tool"
    );
    assert_eq!(
        host.unresolved(&required(&["fs.write_file"])),
        vec!["fs.write_file".to_string()]
    );

    let (name, info, published) = host.inventory().into_iter().next().expect("one server");
    assert_eq!(name, "files");
    let info = info.expect("the handshake completed");
    assert_eq!(info.name, "ingot-mcp-fs");
    assert!(info.serves_tools);
    assert_eq!(published.len(), 2, "{published:?}");
}

#[test]
fn reading_a_file_returns_its_text() {
    let root = workspace("read");
    std::fs::write(root.join("note.md"), "# Note\n\nBody.\n").unwrap();

    let mut host = McpToolHost::connect(
        &config(&root, false, &[]),
        &root,
        &required(&["fs.read_file"]),
    )
    .expect("the server must start");

    let value = host
        .call(&invocation(
            "fs.read_file",
            "text",
            &[("path", json!("note.md"))],
        ))
        .expect("the read must succeed");
    assert_eq!(value, json!("# Note\n\nBody.\n"));
}

#[test]
fn a_list_result_arrives_as_a_typed_array() {
    let root = workspace("list");
    for name in ["b.txt", "a.txt"] {
        std::fs::write(root.join(name), "x").unwrap();
    }

    let mut host = McpToolHost::connect(
        &config(&root, false, &[]),
        &root,
        &required(&["fs.list_dir"]),
    )
    .expect("the server must start");

    let value = host
        .call(&invocation(
            "fs.list_dir",
            "string[]",
            &[("path", json!("."))],
        ))
        .expect("the listing must succeed");
    assert_eq!(value, json!(["a.txt", "b.txt"]));
}

#[test]
fn a_write_returns_a_file_handle_and_the_bytes_land_on_disk() {
    let root = workspace("write");
    let mut host = McpToolHost::connect(
        &config(&root, true, &[]),
        &root,
        &required(&["fs.write_file"]),
    )
    .expect("the server must start");

    let value = host
        .call(&invocation(
            "fs.write_file",
            "file",
            &[
                ("path", json!("out/summary.md")),
                ("content", json!("# Summary\n")),
            ],
        ))
        .expect("the write must succeed");

    assert_eq!(value["path"], json!("out/summary.md"));
    assert_eq!(
        std::fs::read_to_string(root.join("out").join("summary.md")).unwrap(),
        "# Summary\n"
    );
}

#[test]
fn an_alias_maps_the_artifacts_name_onto_the_servers_name() {
    let root = workspace("alias");
    std::fs::write(root.join("main.rs").as_path(), "fn main() {}\n").unwrap();

    let config = config(&root, false, &[("repo.read_file", "fs.read_file")]);
    let mut host = McpToolHost::connect(&config, &root, &required(&["repo.read_file"]))
        .expect("the server must start");

    let resolved = host.resolved();
    assert_eq!(resolved.len(), 1, "{resolved:?}");
    assert_eq!(resolved[0].tool, "repo.read_file");
    assert_eq!(resolved[0].remote, "fs.read_file");
    assert!(resolved[0].aliased);

    let value = host
        .call(&invocation(
            "repo.read_file",
            "text",
            &[("path", json!("main.rs"))],
        ))
        .expect("the aliased read must succeed");
    assert_eq!(value, json!("fn main() {}\n"));
}

#[test]
fn an_alias_the_server_does_not_publish_is_refused_with_the_alternatives() {
    let root = workspace("bad-alias");
    let config = config(&root, false, &[("repo.read_file", "fs.slurp")]);

    let Err(error) = McpToolHost::connect(&config, &root, &required(&["repo.read_file"])) else {
        panic!("mapping onto a tool the server does not have must fail");
    };
    let message = error.to_string();
    assert!(message.contains("fs.slurp"), "{message}");
    assert!(message.contains("fs.read_file"), "{message}");
}

#[test]
fn two_servers_serving_the_same_tool_is_a_configuration_error() {
    let root = workspace("conflict");
    let mut config = config(&root, false, &[]);
    let mut second = config.servers[0].clone();
    second.name = "files-again".to_string();
    config.servers.push(second);

    let Err(error) = McpToolHost::connect(&config, &root, &required(&["fs.read_file"])) else {
        panic!("an ambiguous route must be refused rather than resolved by luck");
    };
    let message = error.to_string();
    assert!(message.contains("files"), "{message}");
    assert!(message.contains("files-again"), "{message}");
    assert!(message.contains("fs.read_file"), "{message}");
}

#[test]
fn a_tool_that_fails_stops_the_call_with_the_servers_explanation() {
    let root = workspace("missing");
    let mut host = McpToolHost::connect(
        &config(&root, false, &[]),
        &root,
        &required(&["fs.read_file"]),
    )
    .expect("the server must start");

    let error = host
        .call(&invocation(
            "fs.read_file",
            "text",
            &[("path", json!("absent.md"))],
        ))
        .expect_err("reading a file that is not there must fail");
    assert!(matches!(error, ToolError::Failed(_)), "{error}");
    assert!(error.to_string().contains("absent.md"), "{error}");
}

#[test]
fn escaping_the_root_is_refused_by_the_server_not_by_luck() {
    let root = workspace("escape");
    let mut host = McpToolHost::connect(
        &config(&root, false, &[]),
        &root,
        &required(&["fs.read_file"]),
    )
    .expect("the server must start");

    for path in ["../../secrets.txt", "/etc/passwd"] {
        let error = host
            .call(&invocation(
                "fs.read_file",
                "text",
                &[("path", json!(path))],
            ))
            .expect_err("a path outside the root must be refused");
        assert!(matches!(error, ToolError::Failed(_)), "{path}: {error}");
    }
}

#[test]
fn a_tool_nothing_serves_is_reported_rather_than_skipped() {
    let root = workspace("unserved");
    let mut host = McpToolHost::connect(
        &config(&root, false, &[]),
        &root,
        &required(&["fs.read_file"]),
    )
    .expect("the server must start");

    assert!(!host.provides("forge.comment"));
    let error = host
        .call(&invocation("forge.comment", "bool", &[]))
        .expect_err("an unrouted tool must fail");
    assert!(matches!(error, ToolError::NotAvailable(_)), "{error}");
}

#[test]
fn a_transport_the_host_does_not_serve_is_refused() {
    let root = workspace("transport");
    let mut host = McpToolHost::connect(
        &config(&root, false, &[]),
        &root,
        &required(&["fs.read_file"]),
    )
    .expect("the server must start");

    let mut call = invocation("fs.read_file", "text", &[("path", json!("x"))]);
    call.transport = "http".to_string();
    let error = host.call(&call).expect_err("only `mcp` is served");
    assert!(error.to_string().contains("http"), "{error}");
}

#[test]
fn a_server_that_refuses_to_start_reports_its_standard_error() {
    let root = workspace("broken");
    let mut config = config(&root, false, &[]);
    config.servers[0].args = vec!["--root".to_string(), "no/such/directory".to_string()];

    let Err(error) = McpToolHost::connect(&config, &root, &required(&["fs.read_file"])) else {
        panic!("a server that exits immediately cannot be connected to");
    };
    let message = error.to_string();
    assert!(message.contains("files"), "{message}");
    assert!(
        message.contains("no/such/directory") || message.contains("--root"),
        "the failure must carry the server's own words: {message}"
    );
}

#[test]
fn a_command_that_does_not_exist_says_which_command() {
    let root = workspace("nocommand");
    let mut config = config(&root, false, &[]);
    config.servers[0].command = "ingot-not-a-real-mcp-server".to_string();

    let Err(error) = McpToolHost::connect(&config, &root, &required(&["fs.read_file"])) else {
        panic!("spawning a command that does not exist must fail");
    };
    assert!(
        error.to_string().contains("ingot-not-a-real-mcp-server"),
        "{error}"
    );
}

#[test]
fn a_server_serving_nothing_this_run_needs_is_never_started() {
    // The command is deliberately nonsense: if the host tried to start it, the
    // test would fail, which is exactly the assertion.
    let root = workspace("unused");
    let mut config = config(&root, false, &[("mailer.send", "send")]);
    config.servers[0].command = "ingot-not-a-real-mcp-server".to_string();

    let host = McpToolHost::connect(&config, &root, &required(&["fs.read_file"]))
        .expect("an unneeded server must not be started");
    assert!(host.is_empty());
}
