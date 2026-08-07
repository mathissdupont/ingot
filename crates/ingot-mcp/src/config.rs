//! How an operator declares which MCP servers exist.
//!
//! This is deployment configuration, not part of the agent. The artifact says
//! *which tools it needs and what they are allowed to do*; this says *where
//! those tools come from on this machine*. Keeping the two apart is what lets
//! the same artifact run against a different set of servers without being
//! recompiled.
//!
//! ```toml
//! [mcp]
//! timeout-seconds = 30
//!
//! [[mcp.server]]
//! name = "files"
//! command = "ingot-mcp-fs"
//! args = ["--root", ".", "--allow-write"]
//! # Names only. Values are read from the operator's environment at spawn time,
//! # so no credential is ever written into a manifest that gets committed.
//! pass-env = ["BRAVE_API_KEY"]
//!
//! # Ingot tool name -> the name this server publishes it under.
//! [mcp.server.tools]
//! "repo.read_file" = "fs.read_file"
//! ```

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The default per-request deadline. Generous enough for a tool that talks to a
/// network service, short enough that a wedged server does not stall a run
/// until someone notices.
pub const DEFAULT_TIMEOUT_SECONDS: u64 = 30;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpConfig {
    /// Configured servers, in the order they are consulted.
    #[serde(default, rename = "server", skip_serializing_if = "Vec::is_empty")]
    pub servers: Vec<ServerConfig>,

    /// Seconds to wait for any single request before giving up.
    #[serde(default = "default_timeout", rename = "timeout-seconds")]
    pub timeout_seconds: u64,
}

impl Default for McpConfig {
    fn default() -> McpConfig {
        McpConfig {
            servers: Vec::new(),
            timeout_seconds: DEFAULT_TIMEOUT_SECONDS,
        }
    }
}

impl McpConfig {
    /// Whether anything is configured at all. Used to keep an untouched
    /// manifest free of an empty `[mcp]` table.
    pub fn is_empty(&self) -> bool {
        self.servers.is_empty()
    }

    pub fn timeout(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.timeout_seconds.max(1))
    }

    /// Reject configurations that cannot work, before anything is spawned.
    pub fn validate(&self) -> Result<(), String> {
        let mut seen: BTreeMap<&str, ()> = BTreeMap::new();
        for server in &self.servers {
            if server.name.trim().is_empty() {
                return Err("an [[mcp.server]] has an empty `name`".to_string());
            }
            if server.command.trim().is_empty() {
                return Err(format!(
                    "MCP server `{}` has an empty `command`",
                    server.name
                ));
            }
            if seen.insert(server.name.as_str(), ()).is_some() {
                return Err(format!(
                    "two [[mcp.server]] entries are both named `{}`; names must be unique",
                    server.name
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerConfig {
    /// How this server is referred to in messages. Unique within a manifest.
    pub name: String,

    /// The program to run. Resolved through `PATH` like any other command.
    pub command: String,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,

    /// The container image to run `command` inside, under `--sandbox`.
    ///
    /// Required to contain a server and ignored otherwise. The image is the
    /// operator's choice because the server is the operator's program: Ingot
    /// cannot know what a third-party MCP server needs installed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,

    /// Working directory, relative to the manifest. Defaults to the project
    /// root, so `args = ["--root", "."]` means the project, not wherever the
    /// operator happened to be standing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,

    /// Environment variables to forward, **by name**. A server otherwise starts
    /// with a minimal environment: nothing the operator exported reaches it
    /// unless it is named here.
    ///
    /// There is deliberately no way to write a value. A manifest is committed;
    /// a secret in one is a secret published.
    #[serde(default, rename = "pass-env", skip_serializing_if = "Vec::is_empty")]
    pub pass_env: Vec<String>,

    /// Ingot tool name to the name the server publishes. Tools not listed here
    /// are matched by name, so the map is only needed when the two differ.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub tools: BTreeMap<String, String>,
}

fn default_timeout() -> u64 {
    DEFAULT_TIMEOUT_SECONDS
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server(name: &str) -> ServerConfig {
        ServerConfig {
            name: name.to_string(),
            command: "ingot-mcp-fs".to_string(),
            args: Vec::new(),
            image: None,
            cwd: None,
            pass_env: Vec::new(),
            tools: BTreeMap::new(),
        }
    }

    #[test]
    fn an_absent_section_means_no_servers_and_the_default_timeout() {
        let config = McpConfig::default();
        assert!(config.is_empty());
        assert_eq!(config.timeout_seconds, DEFAULT_TIMEOUT_SECONDS);
    }

    #[test]
    fn duplicate_server_names_are_refused() {
        let config = McpConfig {
            servers: vec![server("files"), server("files")],
            ..McpConfig::default()
        };
        let error = config.validate().unwrap_err();
        assert!(error.contains("files"), "{error}");
    }

    #[test]
    fn an_empty_command_is_refused_before_anything_is_spawned() {
        let mut only = server("files");
        only.command = String::new();
        let config = McpConfig {
            servers: vec![only],
            ..McpConfig::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn a_zero_timeout_is_clamped_rather_than_meaning_no_wait() {
        let config = McpConfig {
            timeout_seconds: 0,
            ..McpConfig::default()
        };
        assert_eq!(config.timeout().as_secs(), 1);
    }
}
