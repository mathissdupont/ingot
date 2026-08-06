//! `ingot-mcp-fs` — a sandboxed filesystem MCP server.
//!
//! It exists for two reasons. A fresh checkout needs something to point
//! `[[mcp.server]]` at before installing anything from anywhere else, and
//! Ingot's own integration tests need a real subprocess to talk to, so that the
//! path under test is the path that ships.
//!
//! It is not a general-purpose file server. Everything is confined to one root:
//!
//! * paths are relative, and a `..` component is refused outright;
//! * the resolved path is compared against the canonicalised root, so a symlink
//!   pointing outside is refused too;
//! * writing is off unless `--allow-write` is passed;
//! * reads are capped, so one enormous file cannot exhaust memory.
//!
//! ```text
//! ingot-mcp-fs --root . [--allow-write] [--max-bytes N]
//! ```

use std::io::{BufRead, BufReader, Write};
use std::path::{Component, Path, PathBuf};

use ingot_mcp::jsonrpc::{self, Incoming};
use serde_json::{json, Value};

const SERVER_NAME: &str = "ingot-mcp-fs";
const DEFAULT_MAX_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Debug)]
struct Options {
    root: PathBuf,
    allow_write: bool,
    max_bytes: u64,
}

fn main() -> std::process::ExitCode {
    let options = match parse_arguments(std::env::args().skip(1).collect()) {
        Ok(Some(options)) => options,
        Ok(None) => {
            print!("{USAGE}");
            return std::process::ExitCode::SUCCESS;
        }
        Err(message) => {
            eprintln!("{SERVER_NAME}: {message}");
            eprint!("{USAGE}");
            return std::process::ExitCode::FAILURE;
        }
    };

    eprintln!(
        "{SERVER_NAME}: serving {} ({})",
        options.root.display(),
        if options.allow_write {
            "read and write"
        } else {
            "read only"
        }
    );

    serve(&options);
    std::process::ExitCode::SUCCESS
}

const USAGE: &str = "\
usage: ingot-mcp-fs --root <DIR> [--allow-write] [--max-bytes <N>]

A Model Context Protocol server exposing one directory over stdio.

  --root <DIR>       the only directory this server will touch (required)
  --allow-write      permit fs.write_file; off by default
  --max-bytes <N>    refuse to read a file larger than this (default 4194304)
  -h, --help         print this message
";

fn parse_arguments(arguments: Vec<String>) -> Result<Option<Options>, String> {
    let mut root: Option<PathBuf> = None;
    let mut allow_write = false;
    let mut max_bytes = DEFAULT_MAX_BYTES;

    let mut rest = arguments.into_iter();
    while let Some(argument) = rest.next() {
        match argument.as_str() {
            "-h" | "--help" => return Ok(None),
            "--allow-write" => allow_write = true,
            "--root" => {
                let value = rest.next().ok_or("`--root` needs a directory")?;
                root = Some(PathBuf::from(value));
            }
            "--max-bytes" => {
                let value = rest.next().ok_or("`--max-bytes` needs a number")?;
                max_bytes = value
                    .parse()
                    .map_err(|_| format!("`--max-bytes {value}` is not a number"))?;
            }
            other => return Err(format!("unrecognised argument `{other}`")),
        }
    }

    let root = root.ok_or("`--root` is required; this server refuses to guess what to expose")?;
    let root = root
        .canonicalize()
        .map_err(|error| format!("--root {}: {error}", root.display()))?;
    if !root.is_dir() {
        return Err(format!("--root {} is not a directory", root.display()));
    }

    Ok(Some(Options {
        root,
        allow_write,
        max_bytes,
    }))
}

fn serve(options: &Options) {
    let stdin = BufReader::new(std::io::stdin());
    let mut stdout = std::io::stdout();

    for line in stdin.lines() {
        let Ok(line) = line else { return };
        if line.trim().is_empty() {
            continue;
        }

        let reply = match Incoming::parse(&line) {
            Ok(message) => handle(options, &message),
            Err(reason) => Some(jsonrpc::error_line(
                None,
                jsonrpc::PARSE_ERROR,
                &format!("not valid JSON: {reason}"),
            )),
        };

        if let Some(reply) = reply {
            if writeln!(stdout, "{reply}").is_err() || stdout.flush().is_err() {
                return;
            }
        }
    }
}

/// Answer one message, or `None` if it was a notification.
fn handle(options: &Options, message: &Incoming) -> Option<String> {
    if message.is_notification() {
        return None;
    }
    if !message.is_request() {
        // A response to something we never sent.
        return Some(jsonrpc::error_line(
            message.id.as_ref(),
            jsonrpc::INVALID_REQUEST,
            "this server sends no requests, so it expects no responses",
        ));
    }

    let id = message.id.clone().unwrap_or(Value::Null);
    match message.method() {
        "initialize" => Some(jsonrpc::result_line(&id, initialize(message.params()))),
        "ping" => Some(jsonrpc::result_line(&id, json!({}))),
        "tools/list" => Some(jsonrpc::result_line(
            &id,
            json!({ "tools": tool_list(options) }),
        )),
        "tools/call" => Some(match call_tool(options, message.params()) {
            Ok(result) => jsonrpc::result_line(&id, result),
            Err(Refusal { code, message }) => jsonrpc::error_line(Some(&id), code, &message),
        }),
        other => Some(jsonrpc::error_line(
            Some(&id),
            jsonrpc::METHOD_NOT_FOUND,
            &format!("`{other}` is not implemented by {SERVER_NAME}"),
        )),
    }
}

fn initialize(params: &Value) -> Value {
    // Echo the client's revision when it is one we know, which is the
    // negotiation the specification describes. Otherwise state ours and let the
    // client decide whether it can live with it.
    let requested = params.get("protocolVersion").and_then(Value::as_str);
    let version = match requested {
        Some(version) if ingot_mcp::SUPPORTED_PROTOCOL_VERSIONS.contains(&version) => version,
        _ => ingot_mcp::PREFERRED_PROTOCOL_VERSION,
    };

    json!({
        "protocolVersion": version,
        "capabilities": { "tools": { "listChanged": false } },
        "serverInfo": { "name": SERVER_NAME, "version": env!("CARGO_PKG_VERSION") },
    })
}

fn tool_list(options: &Options) -> Vec<Value> {
    let mut tools = vec![
        json!({
            "name": "fs.read_file",
            "description": "Read a UTF-8 text file from the server root.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Path relative to the server root." },
                },
                "required": ["path"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "fs.list_dir",
            "description": "List the entries of a directory under the server root.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Path relative to the server root." },
                },
                "required": ["path"],
                "additionalProperties": false,
            },
            "outputSchema": {
                "type": "object",
                "properties": { "value": { "type": "array", "items": { "type": "string" } } },
                "required": ["value"],
            },
        }),
    ];

    if options.allow_write {
        tools.push(json!({
            "name": "fs.write_file",
            "description": "Write a UTF-8 text file under the server root, creating it if needed.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Path relative to the server root." },
                    "content": { "type": "string" },
                },
                "required": ["path", "content"],
                "additionalProperties": false,
            },
            "outputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "bytes": { "type": "integer" },
                },
                "required": ["path", "bytes"],
            },
        }));
    }
    tools
}

/// A malformed call. Distinct from a tool that ran and failed.
struct Refusal {
    code: i64,
    message: String,
}

fn invalid(message: impl Into<String>) -> Refusal {
    Refusal {
        code: jsonrpc::INVALID_PARAMS,
        message: message.into(),
    }
}

/// A tool that ran and failed. The agent may see this, so it is phrased for a
/// reader rather than for a log.
fn tool_failure(message: impl Into<String>) -> Value {
    json!({
        "content": [{ "type": "text", "text": message.into() }],
        "isError": true,
    })
}

fn text_result(text: String) -> Value {
    json!({ "content": [{ "type": "text", "text": text }], "isError": false })
}

fn structured_result(structured: Value) -> Value {
    // The text block repeats the structured content: a client that predates
    // `structuredContent` still gets an answer, which the specification asks
    // servers to provide.
    json!({
        "content": [{
            "type": "text",
            "text": serde_json::to_string(&structured).unwrap_or_default(),
        }],
        "structuredContent": structured,
        "isError": false,
    })
}

fn call_tool(options: &Options, params: &Value) -> Result<Value, Refusal> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("`tools/call` needs a `name`"))?;
    let arguments = params.get("arguments").unwrap_or(&Value::Null);

    match name {
        "fs.read_file" => {
            let path = string_argument(arguments, "path")?;
            Ok(read_file(options, path))
        }
        "fs.list_dir" => {
            let path = string_argument(arguments, "path")?;
            Ok(list_dir(options, path))
        }
        "fs.write_file" if options.allow_write => {
            let path = string_argument(arguments, "path")?;
            let content = string_argument(arguments, "content")?;
            Ok(write_file(options, path, content))
        }
        "fs.write_file" => Err(invalid(
            "this server was started read-only; restart it with `--allow-write` to permit writes",
        )),
        other => Err(Refusal {
            code: jsonrpc::METHOD_NOT_FOUND,
            message: format!("`{other}` is not a tool of {SERVER_NAME}"),
        }),
    }
}

fn string_argument<'a>(arguments: &'a Value, name: &str) -> Result<&'a str, Refusal> {
    match arguments.get(name) {
        Some(Value::String(value)) => Ok(value),
        Some(_) => Err(invalid(format!("`{name}` must be a string"))),
        None => Err(invalid(format!("`{name}` is required"))),
    }
}

fn read_file(options: &Options, requested: &str) -> Value {
    let path = match safe_path(&options.root, requested) {
        Ok(path) => path,
        Err(reason) => return tool_failure(reason),
    };

    match std::fs::metadata(&path) {
        Ok(metadata) if metadata.is_dir() => {
            return tool_failure(format!("`{requested}` is a directory, not a file"))
        }
        Ok(metadata) if metadata.len() > options.max_bytes => {
            return tool_failure(format!(
                "`{requested}` is {} bytes; this server refuses to read more than {}",
                metadata.len(),
                options.max_bytes
            ))
        }
        Ok(_) => {}
        Err(error) => return tool_failure(format!("`{requested}`: {error}")),
    }

    match std::fs::read(&path) {
        Ok(bytes) => match String::from_utf8(bytes) {
            Ok(text) => text_result(text),
            Err(_) => tool_failure(format!(
                "`{requested}` is not valid UTF-8; this server serves text only"
            )),
        },
        Err(error) => tool_failure(format!("`{requested}`: {error}")),
    }
}

fn list_dir(options: &Options, requested: &str) -> Value {
    let path = match safe_path(&options.root, requested) {
        Ok(path) => path,
        Err(reason) => return tool_failure(reason),
    };

    let entries = match std::fs::read_dir(&path) {
        Ok(entries) => entries,
        Err(error) => return tool_failure(format!("`{requested}`: {error}")),
    };

    let mut names: Vec<String> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .collect();
    // Directory order is filesystem-dependent; sorting makes a run reproducible.
    names.sort();
    structured_result(json!({ "value": names }))
}

fn write_file(options: &Options, requested: &str, content: &str) -> Value {
    let path = match safe_path(&options.root, requested) {
        Ok(path) => path,
        Err(reason) => return tool_failure(reason),
    };
    if path.is_dir() {
        return tool_failure(format!("`{requested}` is a directory"));
    }
    if let Some(parent) = path.parent() {
        if let Err(error) = std::fs::create_dir_all(parent) {
            return tool_failure(format!("creating the directory for `{requested}`: {error}"));
        }
    }
    match std::fs::write(&path, content) {
        Ok(()) => structured_result(json!({ "path": requested, "bytes": content.len() })),
        Err(error) => tool_failure(format!("`{requested}`: {error}")),
    }
}

/// Resolve a client-supplied path inside the root, or explain why not.
///
/// `root` must already be canonical. Two checks, because either alone is
/// insufficient: the component scan rejects the obvious `../../etc/passwd`, and
/// the canonical-prefix check rejects the same escape smuggled through a
/// symlink.
fn safe_path(root: &Path, requested: &str) -> Result<PathBuf, String> {
    if requested.trim().is_empty() {
        return Err("the path is empty".to_string());
    }

    let candidate = Path::new(requested);
    for component in candidate.components() {
        match component {
            Component::Normal(_) | Component::CurDir => {}
            Component::ParentDir => {
                return Err(format!("`{requested}` contains `..`, which is refused"))
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(format!(
                    "`{requested}` is absolute; paths are relative to the server root"
                ))
            }
        }
    }

    let joined = root.join(candidate);

    // `symlink_metadata` rather than `exists`: a symlink that dangles must be
    // resolved and refused, not mistaken for a new file to create.
    if joined.symlink_metadata().is_ok() {
        let resolved = joined
            .canonicalize()
            .map_err(|error| format!("`{requested}`: {error}"))?;
        return confine(root, resolved, requested);
    }

    // The path does not exist yet, which is normal for a write. Canonicalise
    // the deepest ancestor that does exist and check *that*; the components
    // below it are all plain names, already scanned above, so they cannot climb
    // back out of what was just checked.
    let mut missing: Vec<std::ffi::OsString> = Vec::new();
    let mut existing = joined.as_path();
    while existing.symlink_metadata().is_err() {
        let name = existing
            .file_name()
            .ok_or_else(|| format!("`{requested}` names no file"))?;
        missing.push(name.to_os_string());
        existing = existing
            .parent()
            .ok_or_else(|| format!("`{requested}` has no parent directory"))?;
    }

    let anchor = existing
        .canonicalize()
        .map_err(|error| format!("`{requested}`: {error}"))?;
    let mut resolved = confine(root, anchor, requested)?;
    for name in missing.iter().rev() {
        resolved.push(name);
    }
    Ok(resolved)
}

fn confine(root: &Path, resolved: PathBuf, requested: &str) -> Result<PathBuf, String> {
    if resolved.starts_with(root) {
        Ok(resolved)
    } else {
        Err(format!(
            "`{requested}` resolves outside the server root, which is refused"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(root: &Path, allow_write: bool) -> Options {
        Options {
            root: root.to_path_buf(),
            allow_write,
            max_bytes: DEFAULT_MAX_BYTES,
        }
    }

    fn temporary_root(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("ingot-mcp-fs-{label}"));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("creating a temporary root");
        root.canonicalize().expect("canonicalising it")
    }

    #[test]
    fn root_is_required_so_the_server_never_guesses() {
        let error = parse_arguments(vec![]).unwrap_err();
        assert!(error.contains("--root"), "{error}");
    }

    #[test]
    fn an_unknown_flag_is_refused_rather_than_ignored() {
        let error = parse_arguments(vec!["--delete-everything".to_string()]).unwrap_err();
        assert!(error.contains("--delete-everything"), "{error}");
    }

    #[test]
    fn help_asks_for_no_root() {
        assert!(parse_arguments(vec!["--help".to_string()])
            .unwrap()
            .is_none());
    }

    #[test]
    fn a_parent_component_is_refused() {
        let root = temporary_root("parent");
        let error = safe_path(&root, "../secrets.txt").unwrap_err();
        assert!(error.contains(".."), "{error}");
    }

    #[test]
    fn a_rooted_path_is_refused() {
        // `/etc/passwd` has a root component on Windows too, so this one is
        // refused everywhere.
        let root = temporary_root("absolute");
        let error = safe_path(&root, "/etc/passwd").unwrap_err();
        assert!(error.contains("absolute"), "{error}");
    }

    #[cfg(windows)]
    #[test]
    fn a_drive_letter_or_a_unc_share_is_refused() {
        let root = temporary_root("absolute-windows");
        for path in ["C:\\Windows\\win.ini", "\\\\server\\share\\file"] {
            let error = safe_path(&root, path).unwrap_err();
            assert!(error.contains("absolute"), "{path}: {error}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_windows_style_path_is_an_odd_filename_here_not_an_escape() {
        // On Unix a backslash is an ordinary character, so `C:\Windows\win.ini`
        // is one strangely named file inside the root. Refusing it would be
        // theatre; what matters is that it stays inside.
        let root = temporary_root("absolute-unix");
        for path in ["C:\\Windows\\win.ini", "\\\\server\\share\\file"] {
            let resolved = safe_path(&root, path)
                .unwrap_or_else(|error| panic!("{path} should resolve: {error}"));
            assert!(
                resolved.starts_with(&root),
                "{path}: {}",
                resolved.display()
            );
        }
    }

    #[test]
    fn a_path_inside_the_root_resolves() {
        let root = temporary_root("inside");
        std::fs::write(root.join("note.md"), "hello").unwrap();
        let resolved = safe_path(&root, "note.md").unwrap();
        assert!(resolved.starts_with(&root), "{}", resolved.display());
    }

    #[test]
    fn a_file_that_does_not_exist_yet_resolves_so_it_can_be_created() {
        let root = temporary_root("new");
        std::fs::create_dir_all(root.join("out")).unwrap();
        let resolved = safe_path(&root, "out/report.md").unwrap();
        assert!(resolved.starts_with(&root));
    }

    #[test]
    fn a_path_several_levels_below_anything_that_exists_still_resolves_inside_the_root() {
        // A write creates intermediate directories, so resolution must cope
        // with a path whose parent is not there yet — without losing the
        // guarantee that the result is under the root.
        let root = temporary_root("missing-dir");
        let resolved = safe_path(&root, "a/b/c/report.md").unwrap();
        assert!(resolved.starts_with(&root), "{}", resolved.display());
        assert!(resolved.ends_with("report.md"), "{}", resolved.display());
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_pointing_out_of_the_root_is_refused() {
        // The component scan cannot catch this one: every component is a plain
        // name. Only comparing the canonical result against the canonical root
        // does.
        let root = temporary_root("symlink");
        let outside = temporary_root("symlink-target");
        std::fs::write(outside.join("secret.txt"), "shh").unwrap();
        std::os::unix::fs::symlink(&outside, root.join("escape")).unwrap();

        let error = safe_path(&root, "escape/secret.txt").unwrap_err();
        assert!(error.contains("outside the server root"), "{error}");
    }

    #[test]
    fn reading_a_directory_reports_what_it_is() {
        let root = temporary_root("readdir");
        std::fs::create_dir_all(root.join("sub")).unwrap();
        let result = read_file(&options(&root, false), "sub");
        assert_eq!(result["isError"], json!(true));
        assert!(
            result["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("directory"),
            "{result}"
        );
    }

    #[test]
    fn a_file_over_the_cap_is_refused_before_it_is_read() {
        let root = temporary_root("cap");
        std::fs::write(root.join("big.txt"), "0123456789").unwrap();
        let mut options = options(&root, false);
        options.max_bytes = 4;
        let result = read_file(&options, "big.txt");
        assert_eq!(result["isError"], json!(true));
        assert!(
            result["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("10"),
            "{result}"
        );
    }

    #[test]
    fn read_and_write_round_trip() {
        let root = temporary_root("roundtrip");
        let options = options(&root, true);

        let written = write_file(&options, "out/summary.md", "# Summary\n");
        assert_eq!(written["isError"], json!(false));
        assert_eq!(
            written["structuredContent"]["path"],
            json!("out/summary.md")
        );
        assert_eq!(written["structuredContent"]["bytes"], json!(10));

        let read = read_file(&options, "out/summary.md");
        assert_eq!(read["content"][0]["text"], json!("# Summary\n"));
    }

    #[test]
    fn listing_is_sorted_so_a_run_is_reproducible() {
        let root = temporary_root("listing");
        for name in ["c.txt", "a.txt", "b.txt"] {
            std::fs::write(root.join(name), "x").unwrap();
        }
        let result = list_dir(&options(&root, false), ".");
        assert_eq!(
            result["structuredContent"]["value"],
            json!(["a.txt", "b.txt", "c.txt"])
        );
    }

    #[test]
    fn a_read_only_server_publishes_no_write_tool_and_refuses_the_call() {
        let root = temporary_root("readonly");
        let options = options(&root, false);

        let names: Vec<String> = tool_list(&options)
            .iter()
            .map(|tool| tool["name"].as_str().unwrap_or_default().to_string())
            .collect();
        assert!(!names.contains(&"fs.write_file".to_string()), "{names:?}");

        let refusal = call_tool(
            &options,
            &json!({"name": "fs.write_file", "arguments": {"path": "a", "content": "b"}}),
        )
        .expect_err("a read-only server must refuse");
        assert!(
            refusal.message.contains("--allow-write"),
            "{}",
            refusal.message
        );
    }

    #[test]
    fn a_missing_argument_is_a_protocol_error_not_a_tool_error() {
        let root = temporary_root("args");
        let refusal = call_tool(&options(&root, false), &json!({"name": "fs.read_file"}))
            .expect_err("a call without `path` is malformed");
        assert_eq!(refusal.code, jsonrpc::INVALID_PARAMS);
        assert!(refusal.message.contains("path"), "{}", refusal.message);
    }

    #[test]
    fn an_unknown_method_is_reported_by_name() {
        let root = temporary_root("method");
        let message =
            Incoming::parse(r#"{"jsonrpc":"2.0","id":1,"method":"resources/list"}"#).unwrap();
        let reply = handle(&options(&root, false), &message).expect("a request needs a reply");
        assert!(reply.contains("resources/list"), "{reply}");
        assert!(reply.contains("-32601"), "{reply}");
    }

    #[test]
    fn a_notification_gets_no_reply() {
        let root = temporary_root("notify");
        let message =
            Incoming::parse(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#).unwrap();
        assert!(handle(&options(&root, false), &message).is_none());
    }

    #[test]
    fn the_handshake_echoes_a_revision_the_client_knows() {
        let result = initialize(&json!({"protocolVersion": "2024-11-05"}));
        assert_eq!(result["protocolVersion"], json!("2024-11-05"));

        let unknown = initialize(&json!({"protocolVersion": "1999-01-01"}));
        assert_eq!(
            unknown["protocolVersion"],
            json!(ingot_mcp::PREFERRED_PROTOCOL_VERSION)
        );
    }
}
