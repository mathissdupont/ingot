//! Preparing the version-matched reference image for a contained run.
//!
//! Remote acquisition waits for M6 to define digest and signature verification.
//! Until then this command makes the repository's auditable Dockerfile a product
//! operation: one command finds the checkout, verifies its version, and asks the
//! detected local runtime to build the exact tag the current binary selects.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};

const REFERENCE_DOCKERFILE: &str = "tools/ingot.Dockerfile";

/// The local image selected when a contained run has no deliberate override.
pub fn reference_image() -> String {
    format!("ingot/run:{}", env!("CARGO_PKG_VERSION"))
}

/// Build the version-matched reference image from an Ingot source checkout.
pub fn build(source: Option<&Path>) -> Result<u8> {
    let root = reference_source(source)?;
    ensure_matching_version(&root)?;

    let runtime = ingot_sandbox::detect().map_err(|error| anyhow::anyhow!("{error}"))?;
    let image = reference_image();
    eprintln!(
        "building {image} from {} with {} {}",
        root.display(),
        runtime.program,
        runtime.version
    );

    let status = Command::new(&runtime.program)
        .args(build_arguments(&root, &image))
        .status()
        .with_context(|| format!("starting {} build", runtime.program))?;
    if !status.success() {
        bail!(
            "{} failed to build reference image `{image}` (exit {})",
            runtime.program,
            status
        );
    }

    match ingot_sandbox::image_exists(&runtime, &image) {
        Ok(true) => {
            eprintln!("ready      {image}");
            eprintln!("next       ingot run --contained ...");
            Ok(crate::EXIT_OK)
        }
        Ok(false) => bail!(
            "{} reported success, but reference image `{image}` is not present locally",
            runtime.program
        ),
        Err(error) => Err(anyhow::anyhow!("{error}")),
    }
}

/// Explain an absent image without allowing a runtime to pull it implicitly.
pub fn missing_image(image: &str) -> String {
    if image == reference_image() {
        format!(
            "reference image `{image}` is not present locally\n  \
             build the version-matched image:  ingot image build\n  \
             Ingot will not download an unverified image or fall back to a host run"
        )
    } else {
        format!(
            "configured image `{image}` is not present locally\n  \
             build or acquire that image explicitly; Ingot will not pull it automatically or \
             fall back to a host run"
        )
    }
}

fn build_arguments(root: &Path, image: &str) -> Vec<OsString> {
    vec![
        "build".into(),
        "-f".into(),
        root.join(REFERENCE_DOCKERFILE).into_os_string(),
        "-t".into(),
        image.into(),
        root.as_os_str().to_owned(),
    ]
}

fn reference_source(explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(path) = explicit {
        return validate_source(path);
    }

    let cwd = std::env::current_dir().context("finding the current directory")?;
    if let Some(root) = cwd
        .ancestors()
        .find(|candidate| is_reference_source(candidate))
    {
        return validate_source(root);
    }

    let executable = std::env::current_exe().context("finding this executable")?;
    if let Some(root) = executable
        .ancestors()
        .find(|candidate| is_reference_source(candidate))
    {
        return validate_source(root);
    }

    bail!(
        "could not find an Ingot source checkout containing `{REFERENCE_DOCKERFILE}`\n  \
         run from the checkout, or pass it explicitly:  ingot image build <SOURCE>\n  \
         verified remote image acquisition is deferred until M6"
    )
}

fn validate_source(path: &Path) -> Result<PathBuf> {
    let root = path
        .canonicalize()
        .with_context(|| format!("resolving image source {}", path.display()))?;
    if !is_reference_source(&root) {
        bail!(
            "{} is not an Ingot source checkout: `{REFERENCE_DOCKERFILE}` is missing",
            root.display()
        );
    }
    Ok(root)
}

fn is_reference_source(path: &Path) -> bool {
    path.join("Cargo.toml").is_file() && path.join(REFERENCE_DOCKERFILE).is_file()
}

fn ensure_matching_version(root: &Path) -> Result<()> {
    let manifest_path = root.join("Cargo.toml");
    let source = std::fs::read_to_string(&manifest_path)
        .with_context(|| format!("reading {}", manifest_path.display()))?;
    let manifest: toml::Value =
        toml::from_str(&source).with_context(|| format!("parsing {}", manifest_path.display()))?;
    let version = manifest
        .get("workspace")
        .and_then(|workspace| workspace.get("package"))
        .and_then(|package| package.get("version"))
        .and_then(toml::Value::as_str)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "{} has no workspace package version",
                manifest_path.display()
            )
        })?;

    if version != env!("CARGO_PKG_VERSION") {
        bail!(
            "source version {version} does not match this Ingot binary version {}\n  \
             build with the matching checkout so the host and contained interpreter agree",
            env!("CARGO_PKG_VERSION")
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_reference_tag_is_the_binary_version() {
        assert_eq!(
            reference_image(),
            format!("ingot/run:{}", env!("CARGO_PKG_VERSION"))
        );
    }

    #[test]
    fn the_build_uses_the_shipped_recipe_and_exact_reference_tag() {
        let root = Path::new("/source");
        let args = build_arguments(root, "ingot/run:0.3.0");
        assert_eq!(args[0], "build");
        assert_eq!(args[1], "-f");
        assert_eq!(PathBuf::from(&args[2]), root.join(REFERENCE_DOCKERFILE));
        assert_eq!(args[3], "-t");
        assert_eq!(args[4], "ingot/run:0.3.0");
        assert_eq!(PathBuf::from(&args[5]), root);
    }

    #[test]
    fn a_missing_reference_image_never_suggests_a_pull_or_host_fallback() {
        let message = missing_image(&reference_image());
        assert!(message.contains("ingot image build"), "{message}");
        assert!(message.contains("will not download"), "{message}");
        assert!(message.contains("fall back to a host run"), "{message}");
    }
}
