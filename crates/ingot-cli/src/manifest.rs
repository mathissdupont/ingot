//! `ingot.toml`, the project manifest.
//!
//! The manifest is optional: every command also accepts a `.ing` file directly.
//! It exists so that `ingot check` with no arguments does the obvious thing
//! inside a project, and so that build output has a declared home.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use ingot_mcp::McpConfig;
use serde::{Deserialize, Serialize};

pub const MANIFEST_NAME: &str = "ingot.toml";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub project: Project,
    #[serde(default)]
    pub build: Build,
    /// Where the agent's tools come from on this machine.
    ///
    /// Deployment configuration rather than part of the program: the artifact
    /// names the tools it needs, this says which servers provide them. An
    /// untouched manifest has no `[mcp]` table at all.
    #[serde(default, skip_serializing_if = "McpConfig::is_empty")]
    pub mcp: McpConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub name: String,
    #[serde(default = "default_version")]
    pub version: String,
    /// Language version the sources are written against.
    #[serde(default = "default_language")]
    pub language: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Build {
    /// Entry source file, relative to the manifest.
    pub entry: String,
    /// Directory build output is written to, relative to the manifest.
    #[serde(rename = "out-dir")]
    pub out_dir: String,
}

impl Default for Build {
    fn default() -> Self {
        Build {
            entry: "main.ing".to_string(),
            out_dir: "target/ingot".to_string(),
        }
    }
}

fn default_version() -> String {
    "0.1.0".to_string()
}

fn default_language() -> String {
    "0.1".to_string()
}

impl Manifest {
    pub fn new(name: &str) -> Manifest {
        Manifest {
            project: Project {
                name: name.to_string(),
                version: default_version(),
                language: default_language(),
                description: None,
            },
            build: Build::default(),
            mcp: McpConfig::default(),
        }
    }

    pub fn load(path: &Path) -> Result<Manifest> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
    }

    pub fn to_toml(&self) -> String {
        toml::to_string_pretty(self).expect("the manifest model is always serializable")
    }
}

/// What a command decided to operate on.
#[derive(Debug, Clone)]
pub struct Target {
    /// The source file to compile.
    pub entry: PathBuf,
    /// The project directory. Relative paths in the manifest — a tool server's
    /// working directory, for instance — resolve against this rather than
    /// against wherever the operator happened to be standing.
    pub root: PathBuf,
    /// Where build output belongs.
    pub out_dir: PathBuf,
    /// The project this came from, when a manifest was found.
    pub manifest: Option<Manifest>,
}

impl Target {
    /// The tool servers configured for this project. Empty without a manifest:
    /// compiling a loose `.ing` file has no project to read configuration from.
    pub fn mcp(&self) -> McpConfig {
        self.manifest
            .as_ref()
            .map(|manifest| manifest.mcp.clone())
            .unwrap_or_default()
    }
}

/// Work out what to compile from an optional path argument.
///
/// * a `.ing` file  -> compile it, output next to it under `target/ingot`
/// * a directory    -> read its `ingot.toml`
/// * nothing given  -> search upwards from the working directory for `ingot.toml`
pub fn resolve_target(path: Option<&Path>) -> Result<Target> {
    match path {
        Some(path) if path.is_file() => {
            let parent = path.parent().unwrap_or(Path::new(".")).to_path_buf();
            Ok(Target {
                entry: path.to_path_buf(),
                out_dir: parent.join("target").join("ingot"),
                root: parent,
                manifest: None,
            })
        }
        Some(path) if path.is_dir() => from_project_dir(path),
        Some(path) => bail!("{} does not exist", path.display()),
        None => {
            let current = std::env::current_dir().context("reading the working directory")?;
            match find_manifest(&current) {
                Some(dir) => from_project_dir(&dir),
                None => bail!(
                    "no {MANIFEST_NAME} found in {} or any parent directory\n\
                     pass a source file explicitly, or run `ingot init <name>` to create a project",
                    current.display()
                ),
            }
        }
    }
}

fn from_project_dir(dir: &Path) -> Result<Target> {
    let manifest_path = dir.join(MANIFEST_NAME);
    if !manifest_path.is_file() {
        bail!("{} contains no {MANIFEST_NAME}", dir.display());
    }
    let manifest = Manifest::load(&manifest_path)?;
    let entry = dir.join(&manifest.build.entry);
    if !entry.is_file() {
        bail!(
            "entry file {} does not exist (declared as `build.entry` in {})",
            entry.display(),
            manifest_path.display()
        );
    }
    let out_dir = dir.join(&manifest.build.out_dir);
    Ok(Target {
        entry,
        root: dir.to_path_buf(),
        out_dir,
        manifest: Some(manifest),
    })
}

fn find_manifest(start: &Path) -> Option<PathBuf> {
    let mut current = Some(start);
    while let Some(dir) = current {
        if dir.join(MANIFEST_NAME).is_file() {
            return Some(dir.to_path_buf());
        }
        current = dir.parent();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_manifest_round_trips_through_toml() {
        let manifest = Manifest::new("research-agent");
        let parsed: Manifest = toml::from_str(&manifest.to_toml()).expect("must parse");
        assert_eq!(parsed.project.name, "research-agent");
        assert_eq!(parsed.project.language, "0.1");
        assert_eq!(parsed.build.entry, "main.ing");
        assert_eq!(parsed.build.out_dir, "target/ingot");
    }

    #[test]
    fn build_defaults_apply_when_the_section_is_absent() {
        let manifest: Manifest = toml::from_str("[project]\nname = \"x\"\n").expect("must parse");
        assert_eq!(manifest.build.entry, "main.ing");
        assert_eq!(manifest.project.version, "0.1.0");
    }

    #[test]
    fn a_new_manifest_writes_no_empty_mcp_table() {
        let toml = Manifest::new("brief").to_toml();
        assert!(!toml.contains("mcp"), "{toml}");
    }

    #[test]
    fn tool_servers_are_read_from_the_manifest() {
        let manifest: Manifest = toml::from_str(
            r#"
            [project]
            name = "review"

            [mcp]
            timeout-seconds = 5

            [[mcp.server]]
            name = "files"
            command = "ingot-mcp-fs"
            args = ["--root", ".", "--allow-write"]
            pass-env = ["GITHUB_TOKEN"]

            [mcp.server.tools]
            "repo.read_file" = "fs.read_file"
            "#,
        )
        .expect("must parse");

        assert_eq!(manifest.mcp.timeout_seconds, 5);
        let server = &manifest.mcp.servers[0];
        assert_eq!(server.command, "ingot-mcp-fs");
        assert_eq!(server.pass_env, vec!["GITHUB_TOKEN".to_string()]);
        assert_eq!(
            server.tools.get("repo.read_file").map(String::as_str),
            Some("fs.read_file")
        );
    }

    #[test]
    fn a_literal_secret_in_a_server_entry_is_refused_rather_than_ignored() {
        // `pass-env` names variables; there is no `env` key to write a value
        // into. An unknown key must fail loudly, or someone will commit a
        // credential and believe it took effect.
        let error = toml::from_str::<Manifest>(
            r#"
            [project]
            name = "review"

            [[mcp.server]]
            name = "files"
            command = "server"
            env = { BRAVE_API_KEY = "sk-live-secret" }
            "#,
        )
        .expect_err("an unknown key must not be silently dropped");
        assert!(error.to_string().contains("env"), "{error}");
    }

    #[test]
    fn a_project_without_a_manifest_configures_no_tool_servers() {
        let target = Target {
            entry: PathBuf::from("main.ing"),
            root: PathBuf::from("."),
            out_dir: PathBuf::from("target/ingot"),
            manifest: None,
        };
        assert!(target.mcp().is_empty());
    }
}
