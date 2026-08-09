//! `ingot package` — the checked artifact, made movable.
//!
//! The work is in [`ingot_package`]; this is the part that knows what a project
//! is. It collects the compilation's sources, the manifest's declared tool
//! servers and model services, and the portability reports the operator asked
//! for, then hands them over and writes the result.
//!
//! The `[mcp]` and `[model]` sections are read for **names only**. A lockfile is
//! committed, so a field that could hold an environment value would be a way to
//! publish a credential, which is why neither the manifest nor the lockfile has
//! one.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use ingot_compiler::Compilation;
use ingot_package::{
    secrets, Inputs, LockedModelProvider, LockedToolServer, Package, Portability, Project, Source,
    LOCK_NAME,
};

use crate::manifest::Target;

/// The directory a package is written to, under the project's build output.
pub const PACKAGE_DIR: &str = "package";

pub struct PackageConfig {
    pub out_dir: Option<PathBuf>,
    /// Targets whose portability report travels with the package. Empty means
    /// the package makes no portability claim, which is the honest default.
    pub reports: Vec<crate::ReportTarget>,
    /// Compare an existing package with the project instead of writing one.
    pub verify: bool,
    pub json: bool,
}

pub fn run(compilation: &Compilation, target: &Target, config: &PackageConfig) -> Result<u8> {
    let dir = config
        .out_dir
        .clone()
        .unwrap_or_else(|| target.out_dir.join(PACKAGE_DIR));
    let current = build(compilation, target, &config.reports)?;

    if config.verify {
        return verify(&dir, &current, config.json);
    }

    current
        .write(&dir)
        .map_err(|error| anyhow::anyhow!("{error}"))?;
    // The lockfile lives in the project as well as in the package, so it can be
    // committed and reviewed. A lockfile that only existed inside the artifact
    // could not appear in a pull request.
    let lock_path = target.root.join(LOCK_NAME);
    std::fs::write(&lock_path, current.lockfile_json())
        .with_context(|| format!("writing {}", lock_path.display()))?;

    if config.json {
        let payload = serde_json::json!({
            "digest": current.digest(),
            "path": dir.display().to_string(),
            "lockfile": lock_path.display().to_string(),
            "artifactType": ingot_package::oci::PACKAGE_ARTIFACT,
            "layers": current
                .layers()
                .iter()
                .map(|(title, bytes)| serde_json::json!({ "title": title, "size": bytes.len() }))
                .collect::<Vec<_>>(),
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
    } else {
        println!("{} -> {}", current.digest(), dir.display());
        for (title, bytes) in current.layers() {
            println!("  {title} ({} bytes)", bytes.len());
        }
        println!("{}", lock_path.display());
        println!();
        println!("push it with any OCI client, for example:");
        println!(
            "  oras cp --from-oci-layout {}:latest <registry>/<repo>:<tag>",
            dir.display()
        );
    }
    Ok(crate::EXIT_OK)
}

fn verify(dir: &Path, current: &Package, json: bool) -> Result<u8> {
    if !dir.join("oci-layout").is_file() {
        bail!(
            "{} is not a package\n  write one first:  ingot package",
            dir.display()
        );
    }
    let stored = Package::read(dir).map_err(|error| anyhow::anyhow!("{error}"))?;
    let differences = Package::differences(&stored, current);

    if json {
        let payload = serde_json::json!({
            "matches": differences.is_empty(),
            "stored": stored.digest(),
            "current": current.digest(),
            "differences": differences
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
    } else if differences.is_empty() {
        println!("{} matches the project", stored.digest());
    } else {
        println!("the package and the project have diverged:");
        for difference in &differences {
            println!("  {difference}");
        }
    }

    Ok(if differences.is_empty() {
        crate::EXIT_OK
    } else {
        crate::EXIT_DIAGNOSTICS
    })
}

/// Assemble a package from a compiled project.
fn build(
    compilation: &Compilation,
    target: &Target,
    reports: &[crate::ReportTarget],
) -> Result<Package> {
    let manifest = target.manifest.as_ref();
    let project = Project {
        name: manifest
            .map(|manifest| manifest.project.name.clone())
            .unwrap_or_else(|| crate::project_name_for_dir(&target.root)),
        version: manifest
            .map(|manifest| manifest.project.version.clone())
            .unwrap_or_else(|| "0.0.0".to_string()),
    };

    let portability = reports
        .iter()
        .map(|report| match report {
            crate::ReportTarget::Python => {
                let analysis = ingot_backend_python::analyse(
                    ingot_backend_python::TARGET,
                    &compilation.agents,
                );
                Ok(Portability {
                    target: ingot_backend_python::TARGET.to_string(),
                    json: canonical(&analysis)?,
                })
            }
        })
        .collect::<Result<Vec<_>>>()?;

    Package::build(Inputs {
        project,
        generator: format!("ingot {}", env!("CARGO_PKG_VERSION")),
        agents: &compilation.agents,
        sources: project_sources(compilation, &target.root),
        tool_servers: tool_servers(target),
        model_providers: model_providers(target),
        image: target.image(),
        portability,
    })
    .map_err(|error| anyhow::anyhow!("{error}"))
}

/// The canonical encoding, for a document this crate did not model itself.
fn canonical<T: serde::Serialize>(value: &T) -> Result<String> {
    let mut json = serde_json::to_string_pretty(value)?;
    json.push('\n');
    Ok(json)
}

/// Every compilation unit, with a path two machines can agree on.
///
/// A source outside the project keeps its display name rather than an absolute
/// path: an artifact that recorded `C:\Users\…` would not be the same artifact
/// as one built on Linux from the same tree.
pub(crate) fn project_sources(compilation: &Compilation, root: &Path) -> Vec<Source> {
    compilation
        .sources
        .files()
        .map(|file| Source {
            path: relative_path(file.path(), file.name(), root),
            text: file.text().to_string(),
        })
        .collect()
}

fn relative_path(path: Option<&Path>, name: &str, root: &Path) -> String {
    let relative = path
        .and_then(|path| {
            let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
            let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
            path.strip_prefix(&root).ok().map(Path::to_path_buf)
        })
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| {
            // Not under the project root: keep the name the diagnostics use,
            // reduced to its final component so no host path travels.
            name.rsplit('/').next().unwrap_or(name).to_string()
        });
    relative.replace('\\', "/")
}

fn tool_servers(target: &Target) -> Vec<LockedToolServer> {
    target
        .mcp()
        .servers
        .iter()
        .map(|server| LockedToolServer {
            args: server.args.clone(),
            command: server.command.clone(),
            image: server.image.clone(),
            name: server.name.clone(),
            pass_env: server.pass_env.clone(),
        })
        .collect()
}

fn model_providers(target: &Target) -> Vec<LockedModelProvider> {
    target
        .model()
        .providers
        .iter()
        .map(|provider| LockedModelProvider {
            api_key_env: provider.api_key_env.clone(),
            base_url: Some(provider.base_url.clone()),
            name: provider.name.clone(),
        })
        .collect()
}

/// Refuse a build or a package that carries a credential-shaped value.
///
/// Runs over source, the compiled Agent IR bytes and every cassette in the
/// project — the three places [SECURITY.md](../../../SECURITY.md) says a secret
/// must never reach. The message names the file, the line and the shape, and
/// never the value: a report that quoted it would have copied the credential
/// into a terminal and a CI log.
pub fn scan_project(compilation: &Compilation, target: &Target) -> Result<()> {
    for source in project_sources(compilation, &target.root) {
        refuse(secrets::check(&source.path, &source.text))?;
    }
    for agent in &compilation.agents {
        refuse(secrets::check(&agent.agent, &agent.to_canonical_json()))?;
    }
    for (path, text) in cassettes(&target.root) {
        refuse(secrets::check(&path, &text))?;
    }
    Ok(())
}

fn refuse(result: Result<(), secrets::Refusal>) -> Result<()> {
    result.map_err(|refusal| {
        anyhow::anyhow!(
            "{refusal}\n  \
             a credential belongs in the environment, named by `pass-env` in {}, never in a \
             file that is committed or shipped\n  \
             the value is not repeated here on purpose",
            crate::MANIFEST_NAME
        )
    })
}

/// Every cassette in the project's usual place.
///
/// Cassettes never enter a package, but they are scanned: a recording with a key
/// in it is committed to the repository, which is the thing worth catching.
fn cassettes(root: &Path) -> Vec<(String, String)> {
    let dir = root.join("tests").join("cassettes");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut found: Vec<(String, String)> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("json"))
        .filter_map(|path| {
            let name = path.file_name()?.to_string_lossy().to_string();
            let text = std::fs::read_to_string(&path).ok()?;
            Some((format!("tests/cassettes/{name}"), text))
        })
        .collect();
    found.sort();
    found
}
