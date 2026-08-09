//! `ingot.lock` — what a package was built from, and what it expects.
//!
//! It records **identity, not content**: digests and names, never file bodies
//! and never values. There is deliberately no field anywhere in this document
//! that can hold an environment value, which is the same rule the project
//! manifest already enforces for `[[mcp.server]]` and for the same reason — a
//! lockfile is committed.
//!
//! This is not a resolver's output. Ingot has no dependency graph to solve; what
//! it has is a set of inputs whose identity decides whether a run elsewhere
//! means what a run here meant.

use serde::{Deserialize, Serialize};

/// The version of this lockfile shape. Moves independently of the language, the
/// Agent IR and the CLI.
pub const LOCK_VERSION: &str = "1";

/// Fields are declared alphabetically so the canonical encoding is sorted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Lockfile {
    /// Every agent, by name and Agent IR blob digest.
    pub agents: Vec<LockedAgent>,
    /// The image a contained run expects, when the project declares one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    /// The compiler version that produced the package.
    pub ingot: String,
    pub ir_version: String,
    pub language: String,
    pub lock_version: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub model_providers: Vec<LockedModelProvider>,
    pub project: Project,
    /// Every compilation unit, by project-relative path and content digest.
    pub sources: Vec<LockedSource>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_servers: Vec<LockedToolServer>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LockedAgent {
    pub agent: String,
    pub digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LockedSource {
    pub digest: String,
    /// Project-relative and slash-normalized. Never an absolute host path: two
    /// machines that built the same thing must agree on this string.
    pub path: String,
}

/// A declared MCP server, by identity.
///
/// `pass_env` holds variable **names**. There is no field here for a value, and
/// adding one would be the bug SECURITY.md calls tool host leakage.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LockedToolServer {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    pub name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pass_env: Vec<String>,
}

/// A declared model service, by identity.
///
/// `api_key_env` is the **name** of the variable holding the key. Absent means
/// the service needs no authentication, which is what a local server usually
/// wants.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LockedModelProvider {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_env: Option<String>,
    /// The endpoint, which is configuration rather than a secret. A key never
    /// appears here; a URL that embedded one would be caught by the scan.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    pub name: String,
}

impl Lockfile {
    /// Put every list in its documented order.
    ///
    /// Called once on the way out rather than trusted from the caller: a
    /// lockfile that sorted only when its producer remembered to would be
    /// reproducible only by luck.
    pub(crate) fn sorted(mut self) -> Lockfile {
        self.agents.sort();
        self.sources.sort();
        self.tool_servers.sort();
        self.model_providers.sort();
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lockfile() -> Lockfile {
        Lockfile {
            agents: vec![
                LockedAgent {
                    agent: "demo.Second".into(),
                    digest: "sha256:b".into(),
                },
                LockedAgent {
                    agent: "demo.First".into(),
                    digest: "sha256:a".into(),
                },
            ],
            image: None,
            ingot: "0.3.0".into(),
            ir_version: "0.2".into(),
            language: "0.1".into(),
            lock_version: LOCK_VERSION.into(),
            model_providers: Vec::new(),
            project: Project {
                name: "demo".into(),
                version: "0.1.0".into(),
            },
            sources: vec![
                LockedSource {
                    digest: "sha256:z".into(),
                    path: "second.ing".into(),
                },
                LockedSource {
                    digest: "sha256:y".into(),
                    path: "main.ing".into(),
                },
            ],
            tool_servers: Vec::new(),
        }
    }

    #[test]
    fn every_list_is_sorted_on_the_way_out() {
        let sorted = lockfile().sorted();
        assert_eq!(sorted.agents[0].agent, "demo.First");
        // Sources sort by digest first, because that is the first field — the
        // order only has to be *a* total order that both machines agree on.
        assert!(sorted.sources[0].digest <= sorted.sources[1].digest);
    }

    #[test]
    fn the_encoding_is_canonical_and_stable() {
        let first = crate::json::canonical(&lockfile().sorted());
        let second = crate::json::canonical(&lockfile().sorted());
        assert_eq!(first, second);
        assert!(first.ends_with("}\n"), "{first}");
        assert!(!first.ends_with("}\n\n"), "{first}");
        // Alphabetical field declaration means the encoding is key-sorted.
        let keys: Vec<&str> = first
            .lines()
            .filter(|line| line.starts_with("  \""))
            .map(|line| line.trim_start().split('"').nth(1).unwrap_or_default())
            .collect();
        let mut expected = keys.clone();
        expected.sort_unstable();
        assert_eq!(keys, expected, "{first}");
    }

    #[test]
    fn there_is_no_field_that_can_hold_an_environment_value() {
        // A regression guard with teeth: if someone adds `env` or `apiKey` to a
        // locked server or provider, this fails before it ships.
        let json = crate::json::canonical(&LockedToolServer {
            args: vec!["--root".into()],
            command: "server".into(),
            image: None,
            name: "workspace".into(),
            pass_env: vec!["GITHUB_TOKEN".into()],
        });
        assert!(json.contains("passEnv"), "{json}");
        assert!(!json.contains("\"env\""), "{json}");

        let json = crate::json::canonical(&LockedModelProvider {
            api_key_env: Some("OPENAI_API_KEY".into()),
            base_url: Some("https://api.example/v1/chat/completions".into()),
            name: "openai".into(),
        });
        assert!(json.contains("apiKeyEnv"), "{json}");
        assert!(!json.contains("\"apiKey\":"), "{json}");
    }
}
