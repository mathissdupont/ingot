//! The Ingot package: checked Agent IR and its identity, as an OCI artifact.
//!
//! A package is a standard [OCI image layout] directory holding one artifact
//! manifest, so existing tools move it and Ingot invents no transport. What is
//! Ingot-specific is only the media types and the two documents behind them: a
//! config that says what the package is, and a [lockfile](lock) that says what
//! it was built from and what it expects.
//!
//! Two properties are the point of the whole crate, and both are tested:
//!
//! * **The packaged Agent IR is the bytes the compiler wrote.** Not a
//!   re-encoding — the same bytes `ingot build` put on disk and `ingot test`
//!   replayed against. This crate never re-serialises an [`AgentIr`].
//! * **The digest is reproducible.** Same source, same manifest, same compiler,
//!   same digest, on every platform. That means no timestamps, no build-machine
//!   paths, no compression, and one canonical JSON encoding throughout.
//!
//! The normative rules are [Ingot Package 0.1](../../../specs/image/v0.1.md);
//! the reasoning is [RFC-0012](../../../rfcs/0012-the-ingot-package.md).
//!
//! [OCI image layout]: https://github.com/opencontainers/image-spec/blob/main/image-layout.md

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Path, PathBuf};

use ingot_ir::AgentIr;

mod json;
pub mod lock;
pub mod oci;
pub mod secrets;

pub use lock::{LockedModelProvider, LockedToolServer, Lockfile, Project};
pub use oci::{digest, Descriptor, Index, Manifest};

/// The version of the config blob's shape.
pub const CONFIG_VERSION: &str = "1";

/// The file name the lockfile is written under, in the project and in the
/// package.
pub const LOCK_NAME: &str = "ingot.lock";

#[derive(Debug)]
pub enum PackageError {
    /// A program with no agent has nothing to package. Refusing beats writing an
    /// artifact whose only content is metadata about an absence.
    NoAgents,
    /// The package states one `irVersion`; a mixed package could not.
    MixedIrVersions {
        first: String,
        second: String,
    },
    MixedLanguages {
        first: String,
        second: String,
    },
    /// A credential-shaped value reached the packager.
    Secret(secrets::Refusal),
    /// A layer title has to become a file name on somebody else's disk.
    BadTitle(String),
    Io {
        path: String,
        reason: String,
    },
    Malformed {
        path: String,
        reason: String,
    },
    /// A blob does not hash to the digest naming it.
    BlobMismatch {
        path: String,
        expected: String,
        actual: String,
    },
}

impl fmt::Display for PackageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PackageError::NoAgents => write!(f, "the program declares no agent to package"),
            PackageError::MixedIrVersions { first, second } => write!(
                f,
                "this program compiled to both Agent IR {first} and {second}; a package states one"
            ),
            PackageError::MixedLanguages { first, second } => write!(
                f,
                "this program mixes language {first} and {second}; a package states one"
            ),
            PackageError::Secret(refusal) => write!(f, "{refusal}"),
            PackageError::BadTitle(title) => write!(
                f,
                "`{title}` cannot be a layer title: a title becomes a file name, so it may not \
                 contain a path separator, a drive letter or `..`"
            ),
            PackageError::Io { path, reason } => write!(f, "{path}: {reason}"),
            PackageError::Malformed { path, reason } => write!(f, "{path} is not a valid package: {reason}"),
            PackageError::BlobMismatch {
                path,
                expected,
                actual,
            } => write!(
                f,
                "{path} does not match the digest naming it\n  named:    {expected}\n  contains: {actual}"
            ),
        }
    }
}

impl std::error::Error for PackageError {}

impl From<secrets::Refusal> for PackageError {
    fn from(refusal: secrets::Refusal) -> PackageError {
        PackageError::Secret(refusal)
    }
}

/// One compilation unit, by path and content.
///
/// The text is used to compute a digest and to run the secret scan. It is never
/// stored: the package carries identity, not source.
pub struct Source {
    pub path: String,
    pub text: String,
}

/// One target's portability report, already encoded.
pub struct Portability {
    pub target: String,
    pub json: String,
}

/// Everything a package is built from.
pub struct Inputs<'a> {
    pub project: Project,
    /// The producing toolchain, e.g. `ingot 0.3.0`.
    pub generator: String,
    pub agents: &'a [AgentIr],
    pub sources: Vec<Source>,
    pub tool_servers: Vec<LockedToolServer>,
    pub model_providers: Vec<LockedModelProvider>,
    /// The image a contained run expects, when the project declares one.
    pub image: Option<String>,
    pub portability: Vec<Portability>,
}

/// The config blob: what this package is.
///
/// Fields are declared alphabetically so the canonical encoding is sorted.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    pub agents: Vec<ConfigAgent>,
    pub config_version: String,
    pub generator: String,
    pub ir_version: String,
    pub language: String,
    pub project: Project,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigAgent {
    pub agent: String,
    pub digest: String,
    pub title: String,
}

/// A built package, in memory.
pub struct Package {
    manifest: Manifest,
    manifest_bytes: Vec<u8>,
    index: Index,
    config: Config,
    lockfile: Lockfile,
    /// Every blob, keyed by its own digest.
    blobs: BTreeMap<String, Vec<u8>>,
}

impl Package {
    /// Build a package from a checked compilation.
    ///
    /// Every refusal happens here rather than at write time: a package that
    /// cannot exist should not leave a half-written directory behind while
    /// finding that out.
    pub fn build(inputs: Inputs<'_>) -> Result<Package, PackageError> {
        let Some(first) = inputs.agents.first() else {
            return Err(PackageError::NoAgents);
        };
        let ir_version = first.ir_version.clone();
        let language = first.language.clone();
        for agent in inputs.agents {
            if agent.ir_version != ir_version {
                return Err(PackageError::MixedIrVersions {
                    first: ir_version,
                    second: agent.ir_version.clone(),
                });
            }
            if agent.language != language {
                return Err(PackageError::MixedLanguages {
                    first: language,
                    second: agent.language.clone(),
                });
            }
        }

        // Source never enters the package, but it is scanned: a credential in a
        // prompt is exactly what the scan exists for, and the moment to find it
        // is before anything is written.
        for source in &inputs.sources {
            secrets::check(&source.path, &source.text)?;
        }

        let titles = agent_titles(inputs.agents);
        let mut blobs: BTreeMap<String, Vec<u8>> = BTreeMap::new();
        let mut layers: Vec<Descriptor> = Vec::new();
        let mut config_agents: Vec<ConfigAgent> = Vec::new();
        let mut locked_agents: Vec<lock::LockedAgent> = Vec::new();

        for (agent, title) in inputs.agents.iter().zip(&titles) {
            if !oci::is_file_name(title) {
                return Err(PackageError::BadTitle(title.clone()));
            }
            // The bytes the compiler produced, carried verbatim. Re-encoding
            // here would make the packager a second encoder, and a second
            // encoder is how "the artifact you tested" stops being true.
            let bytes = agent.to_canonical_json().into_bytes();
            secrets::check(title, std::str::from_utf8(&bytes).unwrap_or_default())?;

            let descriptor = Descriptor::new(oci::AGENT_IR_LAYER, &bytes)
                .with_annotation(oci::TITLE, title)
                .with_annotation(oci::AGENT, &agent.agent);
            config_agents.push(ConfigAgent {
                agent: agent.agent.clone(),
                digest: descriptor.digest.clone(),
                title: title.clone(),
            });
            locked_agents.push(lock::LockedAgent {
                agent: agent.agent.clone(),
                digest: descriptor.digest.clone(),
            });
            blobs.insert(descriptor.digest.clone(), bytes);
            layers.push(descriptor);
        }
        config_agents.sort();

        let lockfile = Lockfile {
            agents: locked_agents,
            image: inputs.image,
            ingot: inputs
                .generator
                .rsplit(' ')
                .next()
                .unwrap_or(&inputs.generator)
                .to_string(),
            ir_version: ir_version.clone(),
            language: language.clone(),
            lock_version: lock::LOCK_VERSION.to_string(),
            model_providers: inputs.model_providers,
            project: inputs.project.clone(),
            sources: inputs
                .sources
                .iter()
                .map(|source| lock::LockedSource {
                    digest: oci::digest(source.text.as_bytes()),
                    path: source.path.clone(),
                })
                .collect(),
            tool_servers: inputs.tool_servers,
        }
        .sorted();

        let lock_bytes = json::canonical(&lockfile).into_bytes();
        secrets::check(
            LOCK_NAME,
            std::str::from_utf8(&lock_bytes).unwrap_or_default(),
        )?;
        let lock_descriptor =
            Descriptor::new(oci::LOCK_LAYER, &lock_bytes).with_annotation(oci::TITLE, LOCK_NAME);
        blobs.insert(lock_descriptor.digest.clone(), lock_bytes);
        layers.push(lock_descriptor);

        for report in &inputs.portability {
            let title = format!("portability.{}.json", report.target);
            if !oci::is_file_name(&title) {
                return Err(PackageError::BadTitle(title));
            }
            let bytes = report.json.clone().into_bytes();
            let descriptor =
                Descriptor::new(oci::PORTABILITY_LAYER, &bytes).with_annotation(oci::TITLE, &title);
            blobs.insert(descriptor.digest.clone(), bytes);
            layers.push(descriptor);
        }

        // Sorted by title, so two producers that assembled the layers in
        // different orders still agree on the manifest bytes.
        layers.sort_by(|left, right| left.title().cmp(&right.title()));

        let config = Config {
            agents: config_agents,
            config_version: CONFIG_VERSION.to_string(),
            generator: inputs.generator,
            ir_version,
            language,
            project: inputs.project.clone(),
        };
        let config_bytes = json::canonical(&config).into_bytes();
        let config_descriptor = Descriptor::new(oci::PACKAGE_CONFIG, &config_bytes);
        blobs.insert(config_descriptor.digest.clone(), config_bytes);

        let manifest = Manifest {
            annotations: [
                (oci::TITLE.to_string(), inputs.project.name.clone()),
                (oci::VERSION.to_string(), inputs.project.version.clone()),
            ]
            .into(),
            artifact_type: oci::PACKAGE_ARTIFACT.to_string(),
            config: config_descriptor,
            layers,
            media_type: oci::OCI_MANIFEST.to_string(),
            schema_version: oci::SCHEMA_VERSION,
        };
        // No `org.opencontainers.image.created`: a creation time turns a digest
        // from an identity into a serial number.
        let manifest_bytes = json::canonical(&manifest).into_bytes();
        let manifest_descriptor = Descriptor::new(oci::OCI_MANIFEST, &manifest_bytes);
        let index = Index {
            manifests: vec![manifest_descriptor],
            media_type: oci::OCI_INDEX.to_string(),
            schema_version: oci::SCHEMA_VERSION,
        };
        blobs.insert(oci::digest(&manifest_bytes), manifest_bytes.clone());

        Ok(Package {
            manifest,
            manifest_bytes,
            index,
            config,
            lockfile,
            blobs,
        })
    }

    /// The package's identity: `sha256` of the manifest bytes, which is what OCI
    /// already means by the digest of a thing.
    pub fn digest(&self) -> String {
        oci::digest(&self.manifest_bytes)
    }

    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    pub fn lockfile(&self) -> &Lockfile {
        &self.lockfile
    }

    /// The lockfile as it is written, in both places it is written.
    pub fn lockfile_json(&self) -> String {
        json::canonical(&self.lockfile)
    }

    /// Every layer, as `(title, bytes)`.
    pub fn layers(&self) -> Vec<(&str, &[u8])> {
        self.manifest
            .layers
            .iter()
            .filter_map(|layer| {
                Some((
                    layer.title()?,
                    self.blobs.get(&layer.digest).map(Vec::as_slice)?,
                ))
            })
            .collect()
    }

    /// Write the layout to `dir`.
    ///
    /// Blobs are content-addressed, so writing over an existing package is
    /// idempotent for everything still referenced. What is no longer referenced
    /// is removed, because the specification requires that a blob which is
    /// present is also referenced — and a stale blob is how a package quietly
    /// keeps a copy of something a rebuild removed.
    pub fn write(&self, dir: &Path) -> Result<(), PackageError> {
        if dir.exists() && !dir.join("oci-layout").is_file() && !is_empty_dir(dir) {
            return Err(PackageError::Malformed {
                path: dir.display().to_string(),
                reason: "the directory already exists and is not a package; refusing to write \
                         into it"
                    .to_string(),
            });
        }

        let blobs = dir.join("blobs").join("sha256");
        create_dir_all(&blobs)?;
        write_file(
            &dir.join("oci-layout"),
            json::canonical(&oci::Layout::default()).as_bytes(),
        )?;
        write_file(
            &dir.join("index.json"),
            json::canonical(&self.index).as_bytes(),
        )?;

        let mut written: BTreeSet<String> = BTreeSet::new();
        for (digest, bytes) in &self.blobs {
            let Some(relative) = oci::blob_path(digest) else {
                return Err(PackageError::Malformed {
                    path: digest.clone(),
                    reason: "not a well-formed sha256 digest".to_string(),
                });
            };
            write_file(&dir.join(&relative), bytes)?;
            written.insert(relative.rsplit('/').next().unwrap_or_default().to_string());
        }
        prune_unreferenced(&blobs, &written)?;
        Ok(())
    }

    /// Read a package from a layout directory, verifying it against itself.
    ///
    /// Every blob is checked against the digest naming it and the size its
    /// descriptor claims. A package that fails here is corrupt, whatever the
    /// project it came from looks like.
    pub fn read(dir: &Path) -> Result<Package, PackageError> {
        let layout: oci::Layout = read_json(&dir.join("oci-layout"))?;
        if layout.image_layout_version != oci::LAYOUT_VERSION {
            return Err(PackageError::Malformed {
                path: dir.display().to_string(),
                reason: format!(
                    "image layout version `{}` is not supported (expected `{}`)",
                    layout.image_layout_version,
                    oci::LAYOUT_VERSION
                ),
            });
        }

        let index: Index = read_json(&dir.join("index.json"))?;
        let [manifest_descriptor] = index.manifests.as_slice() else {
            return Err(PackageError::Malformed {
                path: dir.display().to_string(),
                reason: format!(
                    "index.json holds {} manifests; a package describes one project",
                    index.manifests.len()
                ),
            });
        };

        let mut blobs: BTreeMap<String, Vec<u8>> = BTreeMap::new();
        let manifest_bytes = read_blob(dir, manifest_descriptor, &mut blobs)?;
        let manifest: Manifest = parse_json(&manifest_bytes, "the manifest")?;

        let config_bytes = read_blob(dir, &manifest.config, &mut blobs)?;
        let config: Config = parse_json(&config_bytes, "the config")?;

        let mut lockfile: Option<Lockfile> = None;
        for layer in &manifest.layers {
            let bytes = read_blob(dir, layer, &mut blobs)?;
            if layer.media_type == oci::LOCK_LAYER {
                lockfile = Some(parse_json(&bytes, LOCK_NAME)?);
            }
        }
        let Some(lockfile) = lockfile else {
            return Err(PackageError::Malformed {
                path: dir.display().to_string(),
                reason: "no lockfile layer".to_string(),
            });
        };

        Ok(Package {
            manifest,
            manifest_bytes,
            index,
            config,
            lockfile,
            blobs,
        })
    }

    /// Everything that moved between a stored package and a freshly built one.
    ///
    /// Nothing is repaired. What this is for is saying that the artifact and the
    /// working tree have diverged, and which way.
    pub fn differences(stored: &Package, current: &Package) -> Vec<Difference> {
        let mut differences = Vec::new();
        if stored.digest() == current.digest() {
            return differences;
        }
        differences.push(Difference::Package {
            stored: stored.digest(),
            current: current.digest(),
        });

        let stored_sources: BTreeMap<&str, &str> = stored
            .lockfile
            .sources
            .iter()
            .map(|source| (source.path.as_str(), source.digest.as_str()))
            .collect();
        for source in &current.lockfile.sources {
            match stored_sources.get(source.path.as_str()) {
                Some(digest) if *digest == source.digest => {}
                Some(digest) => differences.push(Difference::Source {
                    path: source.path.clone(),
                    stored: (*digest).to_string(),
                    current: source.digest.clone(),
                }),
                None => differences.push(Difference::SourceAdded {
                    path: source.path.clone(),
                }),
            }
        }
        let current_paths: BTreeSet<&str> = current
            .lockfile
            .sources
            .iter()
            .map(|source| source.path.as_str())
            .collect();
        for path in stored_sources.keys() {
            if !current_paths.contains(path) {
                differences.push(Difference::SourceRemoved {
                    path: (*path).to_string(),
                });
            }
        }

        let stored_agents: BTreeMap<&str, &str> = stored
            .lockfile
            .agents
            .iter()
            .map(|agent| (agent.agent.as_str(), agent.digest.as_str()))
            .collect();
        for agent in &current.lockfile.agents {
            match stored_agents.get(agent.agent.as_str()) {
                Some(digest) if *digest == agent.digest => {}
                Some(digest) => differences.push(Difference::Agent {
                    agent: agent.agent.clone(),
                    stored: (*digest).to_string(),
                    current: agent.digest.clone(),
                }),
                None => differences.push(Difference::AgentAdded {
                    agent: agent.agent.clone(),
                }),
            }
        }
        let current_agents: BTreeSet<&str> = current
            .lockfile
            .agents
            .iter()
            .map(|agent| agent.agent.as_str())
            .collect();
        for agent in stored_agents.keys() {
            if !current_agents.contains(agent) {
                differences.push(Difference::AgentRemoved {
                    agent: (*agent).to_string(),
                });
            }
        }

        for (field, stored, current) in [
            (
                "generator",
                &stored.config.generator,
                &current.config.generator,
            ),
            (
                "language",
                &stored.config.language,
                &current.config.language,
            ),
            (
                "irVersion",
                &stored.config.ir_version,
                &current.config.ir_version,
            ),
            (
                "project.version",
                &stored.config.project.version,
                &current.config.project.version,
            ),
            (
                "project.name",
                &stored.config.project.name,
                &current.config.project.name,
            ),
        ] {
            if stored != current {
                differences.push(Difference::Metadata {
                    field,
                    stored: stored.clone(),
                    current: current.clone(),
                });
            }
        }

        differences
    }
}

/// One way a stored package and the working tree disagree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Difference {
    Package {
        stored: String,
        current: String,
    },
    Source {
        path: String,
        stored: String,
        current: String,
    },
    SourceAdded {
        path: String,
    },
    SourceRemoved {
        path: String,
    },
    Agent {
        agent: String,
        stored: String,
        current: String,
    },
    AgentAdded {
        agent: String,
    },
    AgentRemoved {
        agent: String,
    },
    Metadata {
        field: &'static str,
        stored: String,
        current: String,
    },
}

impl fmt::Display for Difference {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Difference::Package { stored, current } => {
                write!(
                    f,
                    "package digest\n  stored:  {stored}\n  current: {current}"
                )
            }
            Difference::Source {
                path,
                stored,
                current,
            } => write!(
                f,
                "source {path} changed\n  stored:  {stored}\n  current: {current}"
            ),
            Difference::SourceAdded { path } => write!(f, "source {path} is new since the package"),
            Difference::SourceRemoved { path } => {
                write!(
                    f,
                    "source {path} is in the package and no longer in the project"
                )
            }
            Difference::Agent {
                agent,
                stored,
                current,
            } => write!(
                f,
                "agent {agent} recompiled differently\n  stored:  {stored}\n  current: {current}"
            ),
            Difference::AgentAdded { agent } => write!(f, "agent {agent} is new since the package"),
            Difference::AgentRemoved { agent } => {
                write!(
                    f,
                    "agent {agent} is in the package and no longer in the project"
                )
            }
            Difference::Metadata {
                field,
                stored,
                current,
            } => write!(
                f,
                "{field} changed\n  stored:  {stored}\n  current: {current}"
            ),
        }
    }
}

/// The file name each agent's IR is carried under.
///
/// The short name, which is what `ingot build` writes. Two agents whose short
/// names collide would silently share a title, so when that happens every agent
/// in the package uses its fully qualified name instead — the same rule for all
/// of them, rather than a suffix on whichever sorted second.
fn agent_titles(agents: &[AgentIr]) -> Vec<String> {
    let short: Vec<&str> = agents
        .iter()
        .map(|agent| agent.agent.rsplit('.').next().unwrap_or(&agent.agent))
        .collect();
    let unique: BTreeSet<&str> = short.iter().copied().collect();
    if unique.len() == short.len() {
        short.iter().map(|name| format!("{name}.ir.json")).collect()
    } else {
        agents
            .iter()
            .map(|agent| format!("{}.ir.json", agent.agent))
            .collect()
    }
}

fn is_empty_dir(dir: &Path) -> bool {
    std::fs::read_dir(dir)
        .map(|mut entries| entries.next().is_none())
        .unwrap_or(false)
}

fn create_dir_all(dir: &Path) -> Result<(), PackageError> {
    std::fs::create_dir_all(dir).map_err(|error| PackageError::Io {
        path: dir.display().to_string(),
        reason: error.to_string(),
    })
}

fn write_file(path: &Path, bytes: &[u8]) -> Result<(), PackageError> {
    if let Some(parent) = path.parent() {
        create_dir_all(parent)?;
    }
    std::fs::write(path, bytes).map_err(|error| PackageError::Io {
        path: path.display().to_string(),
        reason: error.to_string(),
    })
}

/// Remove blobs the manifest no longer references.
///
/// Scoped to `blobs/sha256` of a directory this function just wrote a layout
/// into, and only for entries whose names are the digests a blob store uses.
fn prune_unreferenced(blobs: &Path, written: &BTreeSet<String>) -> Result<(), PackageError> {
    let entries = match std::fs::read_dir(blobs) {
        Ok(entries) => entries,
        Err(_) => return Ok(()),
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let is_blob_name = name.len() == 64 && name.chars().all(|ch| ch.is_ascii_hexdigit());
        if !is_blob_name || written.contains(&name) {
            continue;
        }
        let path = entry.path();
        if path.is_file() {
            std::fs::remove_file(&path).map_err(|error| PackageError::Io {
                path: path.display().to_string(),
                reason: error.to_string(),
            })?;
        }
    }
    Ok(())
}

fn read_bytes(path: &Path) -> Result<Vec<u8>, PackageError> {
    std::fs::read(path).map_err(|error| PackageError::Io {
        path: path.display().to_string(),
        reason: error.to_string(),
    })
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, PackageError> {
    let bytes = read_bytes(path)?;
    parse_json(&bytes, &path.display().to_string())
}

fn parse_json<T: serde::de::DeserializeOwned>(bytes: &[u8], what: &str) -> Result<T, PackageError> {
    serde_json::from_slice(bytes).map_err(|error| PackageError::Malformed {
        path: what.to_string(),
        reason: error.to_string(),
    })
}

/// Read one blob and check it against the descriptor that named it.
fn read_blob(
    dir: &Path,
    descriptor: &Descriptor,
    blobs: &mut BTreeMap<String, Vec<u8>>,
) -> Result<Vec<u8>, PackageError> {
    let Some(relative) = oci::blob_path(&descriptor.digest) else {
        return Err(PackageError::Malformed {
            path: descriptor.digest.clone(),
            reason: "not a well-formed sha256 digest".to_string(),
        });
    };
    let path: PathBuf = dir.join(&relative);
    let bytes = read_bytes(&path)?;

    let actual = oci::digest(&bytes);
    if actual != descriptor.digest {
        return Err(PackageError::BlobMismatch {
            path: relative,
            expected: descriptor.digest.clone(),
            actual,
        });
    }
    if bytes.len() as u64 != descriptor.size {
        return Err(PackageError::Malformed {
            path: relative,
            reason: format!(
                "descriptor claims {} bytes and the blob is {}",
                descriptor.size,
                bytes.len()
            ),
        });
    }
    blobs.insert(descriptor.digest.clone(), bytes.clone());
    Ok(bytes)
}
