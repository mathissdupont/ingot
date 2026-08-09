//! The OCI documents a package is made of.
//!
//! Nothing here is Ingot-specific except the media types. A package is an
//! ordinary [OCI image layout] holding one artifact manifest, so `oras`,
//! `skopeo` and `crane` move it without knowing what Ingot is — which is the
//! whole reason not to invent a transport.
//!
//! Every struct declares its fields **alphabetically**. That is not a style
//! choice: it makes the canonical encoding's "sorted object keys" rule a
//! property of the types rather than of the serialiser's mood, the same way
//! [ADR-0004](../../../docs/adr/0004-canonical-ir-encoding.md) makes it a
//! property of using `BTreeMap`.
//!
//! [OCI image layout]: https://github.com/opencontainers/image-spec/blob/main/image-layout.md

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The `artifactType` that says a manifest is an Ingot package.
pub const PACKAGE_ARTIFACT: &str = "application/vnd.ingot.package.v1+json";
/// The config blob.
pub const PACKAGE_CONFIG: &str = "application/vnd.ingot.package.config.v1+json";
/// One Agent IR document, verbatim.
pub const AGENT_IR_LAYER: &str = "application/vnd.ingot.agent-ir.v1+json";
/// The lockfile.
pub const LOCK_LAYER: &str = "application/vnd.ingot.lock.v1+json";
/// One target's portability report.
pub const PORTABILITY_LAYER: &str = "application/vnd.ingot.portability.v1+json";

pub const OCI_MANIFEST: &str = "application/vnd.oci.image.manifest.v1+json";
pub const OCI_INDEX: &str = "application/vnd.oci.image.index.v1+json";

/// The annotation every layer carries: the file name it is written back as.
pub const TITLE: &str = "org.opencontainers.image.title";
/// The annotation an Agent IR layer carries: the fully qualified agent name.
pub const AGENT: &str = "dev.ingot.agent";
pub const VERSION: &str = "org.opencontainers.image.version";

pub const LAYOUT_VERSION: &str = "1.0.0";
pub const SCHEMA_VERSION: u32 = 2;

/// `sha256:<64 lowercase hex characters>`, which is what every registry accepts
/// and the only algorithm this format uses. A second algorithm would mean a
/// second identity for one artifact.
pub fn digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

/// The path a blob lives at inside a layout, relative to the layout root.
pub fn blob_path(digest: &str) -> Option<String> {
    digest
        .strip_prefix("sha256:")
        .filter(|hex| hex.len() == 64 && hex.chars().all(|ch| ch.is_ascii_hexdigit()))
        .map(|hex| format!("blobs/sha256/{hex}"))
}

/// A pointer to a blob, by digest and size.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Descriptor {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub annotations: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_type: Option<String>,
    pub digest: String,
    pub media_type: String,
    pub size: u64,
}

impl Descriptor {
    pub fn new(media_type: &str, bytes: &[u8]) -> Descriptor {
        Descriptor {
            annotations: BTreeMap::new(),
            artifact_type: None,
            digest: digest(bytes),
            media_type: media_type.to_string(),
            size: bytes.len() as u64,
        }
    }

    pub fn with_annotation(mut self, key: &str, value: &str) -> Descriptor {
        self.annotations.insert(key.to_string(), value.to_string());
        self
    }

    /// The file name this blob is written back as, when it names one.
    pub fn title(&self) -> Option<&str> {
        self.annotations.get(TITLE).map(String::as_str)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub annotations: BTreeMap<String, String>,
    pub artifact_type: String,
    pub config: Descriptor,
    pub layers: Vec<Descriptor>,
    pub media_type: String,
    pub schema_version: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Index {
    pub manifests: Vec<Descriptor>,
    pub media_type: String,
    pub schema_version: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Layout {
    pub image_layout_version: String,
}

impl Default for Layout {
    fn default() -> Layout {
        Layout {
            image_layout_version: LAYOUT_VERSION.to_string(),
        }
    }
}

/// Whether a layer title is a file name rather than a path.
///
/// A title becomes a file name on somebody else's disk, so `..` and separators
/// are refused at the point they would be written rather than at the point they
/// would be read.
pub fn is_file_name(title: &str) -> bool {
    !title.is_empty()
        && title != "."
        && title != ".."
        && !title.contains('/')
        && !title.contains('\\')
        && !title.contains(':')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_digest_is_the_sha256_of_the_bytes() {
        // The empty string's SHA-256, which is worth pinning: an off-by-one in
        // the hex formatting would still look plausible.
        assert_eq!(
            digest(b""),
            "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn a_blob_path_is_derived_from_a_well_formed_digest() {
        let digest = digest(b"hello");
        let path = blob_path(&digest).expect("a well-formed digest");
        assert!(path.starts_with("blobs/sha256/"), "{path}");
        assert_eq!(path.len(), "blobs/sha256/".len() + 64);
    }

    #[test]
    fn a_malformed_digest_yields_no_path() {
        // The path is built from attacker-influenceable text the moment a
        // package arrives from elsewhere, so a bad digest must not become a
        // traversal.
        for bad in [
            "sha256:../../etc/passwd",
            "sha256:short",
            "sha512:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            "sha256:E3B0C44298FC1C149AFBF4C8996FB92427AE41E4649B934CA495991B7852B85",
        ] {
            assert_eq!(blob_path(bad), None, "{bad}");
        }
    }

    #[test]
    fn a_title_must_be_a_file_name() {
        assert!(is_file_name("ResearchAgent.ir.json"));
        for bad in ["", ".", "..", "a/b", "a\\b", "C:x", "../etc/passwd"] {
            assert!(!is_file_name(bad), "{bad}");
        }
    }

    #[test]
    fn a_descriptor_records_the_size_it_hashed() {
        let descriptor = Descriptor::new(AGENT_IR_LAYER, b"{}\n");
        assert_eq!(descriptor.size, 3);
        assert_eq!(descriptor.digest, digest(b"{}\n"));
    }
}
