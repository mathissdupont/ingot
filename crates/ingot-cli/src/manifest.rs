//! `ingot.toml`, the project manifest.
//!
//! The manifest is optional: every command also accepts a `.ing` file directly.
//! It exists so that `ingot check` with no arguments does the obvious thing
//! inside a project, and so that build output has a declared home.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

pub const MANIFEST_NAME: &str = "ingot.toml";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub project: Project,
    #[serde(default)]
    pub build: Build,
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
    /// Where build output belongs.
    pub out_dir: PathBuf,
    /// The project this came from, when a manifest was found.
    pub manifest: Option<Manifest>,
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
}
