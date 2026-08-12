//! Model Context Protocol support for Ingot.
//!
//! Ingot does not define a tool protocol. An `.ing` file declares *what* a tool
//! is — its name, its parameter types, its result type and the effects calling
//! it has — and the compiler checks every call against that declaration and
//! against the agent's policy. This crate is the other half: it connects those
//! declarations to real [MCP] servers at run time.
//!
//! The split matters. The artifact is portable because it says nothing about
//! where `web.search` lives; the manifest says that, and it says it per machine.
//!
//! ```no_run
//! use std::collections::BTreeSet;
//! use std::path::Path;
//! use ingot_mcp::{McpConfig, McpToolHost};
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let config: McpConfig = toml::from_str(r#"
//!     [[server]]
//!     name = "files"
//!     command = "ingot-mcp-fs"
//!     args = ["--root", "."]
//! "#)?;
//!
//! let required: BTreeSet<String> = ["fs.read_file".to_string()].into();
//! let mut host = McpToolHost::connect(&config, Path::new("."), &required)?;
//! // `host` is a `ToolHost`; hand it to `ingot_runtime::run`.
//! # Ok(())
//! # }
//! ```
//!
//! # What is not here
//!
//! * **Prompts and sampling.** An Ingot agent's prompts are compiled into its
//!   artifact, and a tool server asking the agent's model to complete something
//!   would be an effect nothing declared.
//! * **Resources.** The language has no type for a subscribed resource yet.
//! * **The deprecated HTTP+SSE transport.** Two endpoints and a long-lived
//!   `GET`, for servers the protocol itself is moving off. Streamable HTTP is
//!   implemented; a server offering only the older shape is refused naming what
//!   it offered.
//!
//! None of these are silently ignored: a server that expects them gets an
//! explicit refusal.
//!
//! # Reaching a server over a network
//!
//! A `[[mcp.server]]` may carry a `url` instead of a `command`. Bytes then leave
//! the machine on the agent's behalf, so the server's host is checked against
//! that agent's own `network` policy grant **before anything connects**:
//! `network deny` means no remote server at all, and there is no "except through
//! tools". See
//! [RFC-0019](../../../rfcs/0019-a-tool-server-that-is-not-a-child-process.md).
//!
//! [MCP]: https://modelcontextprotocol.io

pub mod client;
pub mod config;
pub mod convert;
pub mod host;
#[cfg(feature = "http")]
pub mod http;
pub mod jsonrpc;
pub mod transport;

pub use client::{
    CallOutcome, ContentBlock, McpClient, McpError, ServerInfo, ToolDescriptor,
    PREFERRED_PROTOCOL_VERSION, SUPPORTED_PROTOCOL_VERSIONS,
};
pub use config::{McpConfig, ServerConfig, DEFAULT_TIMEOUT_SECONDS};
pub use host::{AgentTools, DirectLauncher, Launcher, McpToolHost, NetworkGrant, ResolvedTool};
pub use transport::{ChildTransport, LoopbackTransport, Transport, TransportError};

#[cfg(feature = "http")]
pub use http::HttpTransport;
