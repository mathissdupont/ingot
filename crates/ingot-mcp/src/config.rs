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
            server.validate()?;
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
    ///
    /// Mutually exclusive with `url`, and exactly one of the two is required.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub command: String,

    /// A remote server's endpoint, spoken to over Streamable HTTP.
    ///
    /// Bytes leave the machine on the agent's behalf, so the host here is
    /// checked against the calling agent's own `network` policy grant before
    /// anything connects. See
    /// [RFC-0019](../../../rfcs/0019-a-tool-server-that-is-not-a-child-process.md).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,

    /// The environment variable holding a bearer token for `url`.
    ///
    /// A **name**, never a value, for the reason `pass-env` gives: a manifest is
    /// committed, and a secret in one is a secret published. The value is read
    /// at connect time and never reaches an error message, a run record or
    /// `ingot tools`.
    #[serde(default, rename = "auth-env", skip_serializing_if = "Option::is_none")]
    pub auth_env: Option<String>,

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

impl ServerConfig {
    /// Whether this server is reached over a network rather than started.
    pub fn is_remote(&self) -> bool {
        self.url.is_some()
    }

    /// The host a remote server lives at, for the policy check.
    ///
    /// Parsed by hand rather than with a URL crate: one scheme, one authority,
    /// and taking a dependency to find a substring would be the larger change.
    pub fn host(&self) -> Option<String> {
        let url = self.url.as_deref()?;
        let rest = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
        let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
        // Userinfo before the host, and a port after it, are neither the host.
        let authority = authority
            .rsplit_once('@')
            .map(|(_, host)| host)
            .unwrap_or(authority);
        let host = match authority.strip_prefix('[') {
            // An IPv6 literal, whose colons are not a port separator.
            Some(rest) => rest.split_once(']').map(|(host, _)| host).unwrap_or(rest),
            None => authority.split(':').next().unwrap_or(authority),
        };
        (!host.is_empty()).then(|| host.to_ascii_lowercase())
    }

    /// Whether this endpoint is one where plain HTTP is unremarkable.
    ///
    /// A server on the loopback interface is an ordinary deployment. Anything
    /// else over plain HTTP draws a warning on every run.
    pub fn is_loopback(&self) -> bool {
        matches!(
            self.host().as_deref(),
            Some("localhost" | "127.0.0.1" | "::1")
        )
    }

    /// Refuse an entry that cannot work, or that says something it will not
    /// honour.
    fn validate(&self) -> Result<(), String> {
        let named = &self.name;
        match (self.command.trim().is_empty(), &self.url) {
            (true, None) => {
                return Err(format!(
                    "MCP server `{named}` has neither a `command` nor a `url`\n  \
                     a server is a program to start or an endpoint to reach"
                ))
            }
            (false, Some(_)) => {
                return Err(format!(
                    "MCP server `{named}` has both a `command` and a `url`\n  \
                     a server is started or reached, not both"
                ))
            }
            (true, Some(url)) if url.trim().is_empty() => {
                return Err(format!("MCP server `{named}` has an empty `url`"))
            }
            _ => {}
        }

        if self.is_remote() {
            // Every one of these means something for a child process and
            // nothing for a URL. Silently accepting `pass-env` beside a `url`
            // would let an operator believe a credential had been supplied to a
            // server that never saw it.
            let ignored = [
                (!self.args.is_empty(), "args"),
                (self.cwd.is_some(), "cwd"),
                (!self.pass_env.is_empty(), "pass-env"),
                (self.image.is_some(), "image"),
            ];
            if let Some((_, field)) = ignored.into_iter().find(|(present, _)| *present) {
                return Err(format!(
                    "MCP server `{named}` has a `url` and a `{field}`\n  \
                     `{field}` configures a program this machine starts, and a remote \
                     server is not one"
                ));
            }
            if self.host().is_none() {
                return Err(format!(
                    "MCP server `{named}` has a `url` with no host: {}",
                    self.url.as_deref().unwrap_or("")
                ));
            }
        } else if self.auth_env.is_some() {
            return Err(format!(
                "MCP server `{named}` has an `auth-env` and a `command`\n  \
                 a bearer token is sent to an endpoint; a child process is given \
                 environment variables with `pass-env`"
            ));
        }

        Ok(())
    }
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
            url: None,
            auth_env: None,
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

    fn remote(name: &str, url: &str) -> ServerConfig {
        ServerConfig {
            command: String::new(),
            url: Some(url.to_string()),
            ..server(name)
        }
    }

    fn refused(entry: ServerConfig) -> String {
        McpConfig {
            servers: vec![entry],
            ..McpConfig::default()
        }
        .validate()
        .expect_err("this entry cannot work")
    }

    #[test]
    fn a_server_is_started_or_reached_and_not_both() {
        let mut both = server("files");
        both.url = Some("https://example.invalid/mcp".to_string());
        assert!(refused(both).contains("not both"));

        let mut neither = server("files");
        neither.command = String::new();
        assert!(refused(neither).contains("neither"));
    }

    #[test]
    fn a_field_that_configures_a_child_process_is_refused_beside_a_url() {
        // Refused rather than ignored: silently accepting `pass-env` would let
        // an operator believe a credential reached a server that never saw it.
        for (label, mutate) in [
            (
                "args",
                (|s: &mut ServerConfig| s.args = vec!["-x".into()]) as fn(&mut ServerConfig),
            ),
            ("cwd", |s: &mut ServerConfig| s.cwd = Some(".".into())),
            ("pass-env", |s: &mut ServerConfig| {
                s.pass_env = vec!["TOKEN".into()]
            }),
            ("image", |s: &mut ServerConfig| {
                s.image = Some("alpine".into())
            }),
        ] {
            let mut entry = remote("hosted", "https://example.invalid/mcp");
            mutate(&mut entry);
            let error = refused(entry);
            assert!(error.contains(label), "{label}: {error}");
        }
    }

    #[test]
    fn a_bearer_token_is_refused_beside_a_command() {
        let mut entry = server("files");
        entry.auth_env = Some("TOKEN".to_string());
        let error = refused(entry);
        assert!(error.contains("pass-env"), "{error}");
    }

    #[test]
    fn a_host_is_read_out_of_every_shape_of_url() {
        let cases = [
            ("https://mcp.example.com/mcp", "mcp.example.com"),
            ("https://mcp.example.com:8443/mcp?x=1", "mcp.example.com"),
            ("http://127.0.0.1:3000/mcp", "127.0.0.1"),
            ("https://user:pw@Host.EXAMPLE.com/mcp", "host.example.com"),
            ("https://[::1]:9000/mcp", "::1"),
        ];
        for (url, expected) in cases {
            assert_eq!(
                remote("hosted", url).host().as_deref(),
                Some(expected),
                "{url}"
            );
        }
        assert_eq!(remote("hosted", "https:///mcp").host(), None);
    }

    #[test]
    fn only_a_loopback_endpoint_is_an_ordinary_place_for_plain_http() {
        assert!(remote("hosted", "http://localhost:3000/mcp").is_loopback());
        assert!(remote("hosted", "http://127.0.0.1/mcp").is_loopback());
        assert!(!remote("hosted", "http://mcp.example.com/mcp").is_loopback());
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
