//! End-to-end tests for `ingot package`.
//!
//! The conformance tests named in
//! [RFC-0012](../../../rfcs/0012-the-ingot-package.md) live here. They exercise
//! the binary rather than the library, because the properties being asserted are
//! about the artifact a person receives, not about a function's return value.

mod support;

use std::path::{Path, PathBuf};
use std::process::Output;

use support::{code, run_env, stderr, stdout, toml_string, TempDir, EXIT_DIAGNOSTICS, EXIT_OK};

/// The command itself failed, as opposed to the program having diagnostics.
const EXIT_FAILURE: i32 = 2;

/// A credential value that must never reach a package, a lockfile or a log.
///
/// Exported into the environment by the tests that check a lockfile records the
/// *name* of a variable and never what it holds.
const SECRET_VALUE: &str = "sk-live-9f3a1c7e5b2d84a06e1f";

const SOURCE: &str = r#"language 0.1
package packaged

/// Summarises a topic.
agent Brief(topic: string) -> brief<markdown> {
  model requires {
    structured_output
  }

  budget {
    steps <= 4
    tokens <= 20000
  }

  policy {
    network deny
  }

  flow {
    emit brief = ask<markdown>("Write a short brief about ${topic}.")
  }
}
"#;

/// A project with a tool server and a model service declared, so the lockfile
/// has something to record besides the source.
struct Project {
    dir: TempDir,
}

impl Project {
    fn new(tag: &str, source: &str) -> Project {
        let dir = TempDir::new(tag);
        std::fs::write(dir.path().join("main.ing"), source).expect("writing the source");
        std::fs::write(
            dir.path().join("ingot.toml"),
            format!(
                r#"[project]
name = "packaged"
version = "0.2.1"

[build]
entry = "main.ing"
out-dir = "target/ingot"

[run]
image = "ingot/run:0.3.0"

[[model.provider]]
name = "local"
kind = "openai"
base-url = "http://127.0.0.1:1/v1/chat/completions"
api-key-env = "PACKAGE_TEST_KEY"

[[mcp.server]]
name = "workspace"
command = {}
args = ["--root", "data"]
pass-env = ["PACKAGE_TEST_KEY"]
"#,
                toml_string("ingot-mcp-fs")
            ),
        )
        .expect("writing the manifest");
        Project { dir }
    }

    fn path(&self) -> String {
        self.dir.path().display().to_string()
    }

    fn root(&self) -> &Path {
        self.dir.path()
    }

    fn package_dir(&self) -> PathBuf {
        self.root().join("target/ingot/package")
    }
}

/// Run with the credential exported, so any path that could copy a value into an
/// artifact has one available to copy.
fn ingot(args: &[&str]) -> Output {
    run_env(args, &[("PACKAGE_TEST_KEY", SECRET_VALUE)])
}

/// Every byte of every file in a directory, for "this string is nowhere"
/// assertions.
fn all_bytes(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return found;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            found.extend(all_bytes(&path));
        } else if let Ok(bytes) = std::fs::read(&path) {
            found.push((path, bytes));
        }
    }
    found
}

fn contains(bytes: &[u8], needle: &str) -> bool {
    bytes
        .windows(needle.len())
        .any(|window| window == needle.as_bytes())
}

#[test]
fn the_packaged_ir_is_the_same_bytes_that_the_tested_build_produced() {
    let project = Project::new("package-same-bytes", SOURCE);

    let build = ingot(&["build", &project.path()]);
    assert_eq!(code(&build), EXIT_OK, "{}", stderr(&build));
    let built = std::fs::read(project.root().join("target/ingot/Brief.ir.json"))
        .expect("the build must leave an IR artifact");

    let package = ingot(&["package", &project.path()]);
    assert_eq!(code(&package), EXIT_OK, "{}", stderr(&package));

    // The packaged blob is found by its digest, exactly as a consumer would find
    // it, and must be the same bytes rather than an equivalent re-encoding.
    let packaged = all_bytes(&project.package_dir())
        .into_iter()
        .find(|(_, bytes)| bytes == &built)
        .map(|(path, _)| path);
    assert!(
        packaged.is_some(),
        "no blob in the package holds the bytes `ingot build` wrote:\n{}",
        String::from_utf8_lossy(&built)
    );

    // And the manifest names it, so it is reachable rather than merely present.
    let digest = ingot_package::digest(&built);
    let manifest = all_bytes(&project.package_dir())
        .into_iter()
        .map(|(_, bytes)| String::from_utf8_lossy(&bytes).into_owned())
        .find(|text| text.contains("\"artifactType\""))
        .expect("a package has a manifest");
    assert!(manifest.contains(&digest), "{manifest}");
    assert!(manifest.contains("Brief.ir.json"), "{manifest}");
}

#[test]
fn a_package_digest_is_reproducible_across_runs_and_platforms() {
    let project = Project::new("package-reproducible", SOURCE);

    let first = ingot(&["package", &project.path(), "--json"]);
    assert_eq!(code(&first), EXIT_OK, "{}", stderr(&first));
    let second = ingot(&["package", &project.path(), "--json"]);
    assert_eq!(code(&second), EXIT_OK, "{}", stderr(&second));

    let first: serde_json::Value = serde_json::from_str(&stdout(&first)).expect("json");
    let second: serde_json::Value = serde_json::from_str(&stdout(&second)).expect("json");
    assert_eq!(first["digest"], second["digest"]);

    // A second project with the same inputs in a different directory must reach
    // the same digest, which is the platform-independence claim in miniature:
    // if any build-machine path or timestamp were in the bytes, this diverges.
    let elsewhere = Project::new("package-reproducible-elsewhere", SOURCE);
    let other = ingot(&["package", &elsewhere.path(), "--json"]);
    assert_eq!(code(&other), EXIT_OK, "{}", stderr(&other));
    let other: serde_json::Value = serde_json::from_str(&stdout(&other)).expect("json");
    assert_eq!(
        first["digest"], other["digest"],
        "the same inputs in another directory must produce the same package"
    );

    // No timestamp, under any spelling, anywhere in the artifact.
    for (path, bytes) in all_bytes(&project.package_dir()) {
        for stamp in ["created", "timestamp", "buildTime"] {
            assert!(
                !contains(&bytes, stamp),
                "{} contains `{stamp}`",
                path.display()
            );
        }
    }
}

#[test]
fn a_package_carries_no_credential_no_cassette_and_no_build_machine_path() {
    let project = Project::new("package-carries-nothing", SOURCE);
    std::fs::create_dir_all(project.root().join("tests/cassettes")).expect("cassette directory");
    std::fs::write(
        project.root().join("tests/cassettes/example.json"),
        r#"{"cassetteVersion":"0.1","agent":"packaged.Brief","interactions":[]}"#,
    )
    .expect("writing a cassette");

    let output = ingot(&["package", &project.path()]);
    assert_eq!(code(&output), EXIT_OK, "{}", stderr(&output));

    let machine_path = project.root().display().to_string();
    let home = machine_path.replace('\\', "/");
    for (path, bytes) in all_bytes(&project.package_dir()) {
        assert!(
            !contains(&bytes, SECRET_VALUE),
            "{} contains an exported credential value",
            path.display()
        );
        assert!(
            !contains(&bytes, &machine_path) && !contains(&bytes, &home),
            "{} contains a path from the build machine",
            path.display()
        );
        assert!(
            !contains(&bytes, "cassetteVersion"),
            "{} contains a cassette",
            path.display()
        );
        // Source identity travels; source text does not.
        assert!(
            !contains(&bytes, "emit brief = ask<markdown>"),
            "{} contains source text",
            path.display()
        );
    }
}

#[test]
fn a_lockfile_records_identity_and_never_an_environment_value() {
    let project = Project::new("package-lockfile", SOURCE);
    let output = ingot(&["package", &project.path()]);
    assert_eq!(code(&output), EXIT_OK, "{}", stderr(&output));

    let lock = std::fs::read_to_string(project.root().join("ingot.lock")).expect("a lockfile");
    let parsed: serde_json::Value = serde_json::from_str(&lock).expect("canonical json");

    assert_eq!(parsed["lockVersion"], "1");
    assert_eq!(parsed["project"]["name"], "packaged");
    assert_eq!(parsed["project"]["version"], "0.2.1");
    assert_eq!(parsed["image"], "ingot/run:0.3.0");
    assert_eq!(parsed["sources"][0]["path"], "main.ing");
    assert!(
        parsed["sources"][0]["digest"]
            .as_str()
            .expect("a digest")
            .starts_with("sha256:"),
        "{lock}"
    );
    assert_eq!(parsed["agents"][0]["agent"], "packaged.Brief");

    // The names of the variables, and nothing they hold.
    assert_eq!(parsed["toolServers"][0]["passEnv"][0], "PACKAGE_TEST_KEY");
    assert_eq!(parsed["modelProviders"][0]["apiKeyEnv"], "PACKAGE_TEST_KEY");
    assert!(
        !lock.contains(SECRET_VALUE),
        "a lockfile is committed; a value in one is a value published:\n{lock}"
    );

    // Canonical: sorted keys, two-space indent, one trailing newline.
    assert!(lock.ends_with("}\n") && !lock.ends_with("}\n\n"), "{lock}");
    let top: Vec<&str> = lock
        .lines()
        .filter(|line| line.starts_with("  \""))
        .filter_map(|line| line.trim_start().split('"').nth(1))
        .collect();
    let mut sorted = top.clone();
    sorted.sort_unstable();
    assert_eq!(top, sorted, "{lock}");

    // And the package carries the same bytes, so a received artifact describes
    // itself without the repository.
    let packaged = all_bytes(&project.package_dir())
        .into_iter()
        .any(|(_, bytes)| bytes == lock.as_bytes());
    assert!(packaged, "the lockfile must travel with the package");
}

#[test]
fn a_secret_in_source_or_a_cassette_fails_the_build() {
    let leaked = SOURCE.replace(
        "Write a short brief about ${topic}.",
        "Use api_key=\\\"sk-live-4f9ac1d3b7e25a86e1\\\" for ${topic}.",
    );
    let project = Project::new("package-secret-source", &leaked);

    for command in [["build"], ["package"]] {
        let output = ingot(&[command[0], &project.path()]);
        assert_eq!(
            code(&output),
            EXIT_FAILURE,
            "`ingot {}` must refuse a credential in source:\n{}",
            command[0],
            stderr(&output)
        );
        let message = stderr(&output);
        assert!(message.contains("main.ing line"), "{message}");
        assert!(message.contains("a vendor-prefixed API key"), "{message}");
        assert!(
            !message.contains("sk-live-4f9ac1d3b7e25a86e1"),
            "a refusal must never reproduce the value:\n{message}"
        );
    }
    assert!(
        !project.package_dir().exists(),
        "a refused build must leave no artifact"
    );

    // A cassette is not packaged, and is still scanned: a recording with a key in
    // it is committed to the repository, which is the thing worth catching.
    let clean = Project::new("package-secret-cassette", SOURCE);
    std::fs::create_dir_all(clean.root().join("tests/cassettes")).expect("cassette directory");
    std::fs::write(
        clean.root().join("tests/cassettes/leak.json"),
        "{\n  \"note\": \"Authorization: Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9\"\n}\n",
    )
    .expect("writing a cassette");

    let output = ingot(&["build", &clean.path()]);
    assert_eq!(code(&output), EXIT_FAILURE, "{}", stdout(&output));
    let message = stderr(&output);
    assert!(
        message.contains("tests/cassettes/leak.json line 2"),
        "{message}"
    );
    assert!(message.contains("a bearer token"), "{message}");
    assert!(
        !message.contains("eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9"),
        "{message}"
    );
}

#[test]
fn verify_reports_every_input_that_moved_since_the_package_was_written() {
    let project = Project::new("package-verify", SOURCE);
    let written = ingot(&["package", &project.path()]);
    assert_eq!(code(&written), EXIT_OK, "{}", stderr(&written));

    let matched = ingot(&["package", &project.path(), "--verify"]);
    assert_eq!(code(&matched), EXIT_OK, "{}", stderr(&matched));
    assert!(
        stdout(&matched).contains("matches the project"),
        "{}",
        stdout(&matched)
    );

    std::fs::write(
        project.root().join("main.ing"),
        SOURCE.replace("Write a short brief", "Write a very short brief"),
    )
    .expect("editing the source");

    let diverged = ingot(&["package", &project.path(), "--verify"]);
    assert_eq!(code(&diverged), EXIT_DIAGNOSTICS, "{}", stderr(&diverged));
    let out = stdout(&diverged);
    assert!(out.contains("package digest"), "{out}");
    assert!(out.contains("source main.ing changed"), "{out}");
    assert!(
        out.contains("agent packaged.Brief recompiled differently"),
        "{out}"
    );

    // A verify never repairs: the package on disk is the one that was written.
    let json = ingot(&["package", &project.path(), "--verify", "--json"]);
    let parsed: serde_json::Value = serde_json::from_str(&stdout(&json)).expect("json");
    assert_eq!(parsed["matches"], false);
    assert_ne!(parsed["stored"], parsed["current"]);
}

#[test]
fn a_corrupt_blob_is_refused_rather_than_read() {
    let project = Project::new("package-corrupt", SOURCE);
    let written = ingot(&["package", &project.path()]);
    assert_eq!(code(&written), EXIT_OK, "{}", stderr(&written));

    // Rewrite the largest blob — the Agent IR — with something else its digest
    // does not describe.
    let (path, _) = all_bytes(&project.package_dir())
        .into_iter()
        .filter(|(path, _)| path.to_string_lossy().contains("blobs"))
        .max_by_key(|(_, bytes)| bytes.len())
        .expect("a package has blobs");
    std::fs::write(&path, b"{}\n").expect("corrupting a blob");

    let output = ingot(&["package", &project.path(), "--verify"]);
    assert_eq!(code(&output), EXIT_FAILURE, "{}", stdout(&output));
    let message = stderr(&output);
    assert!(message.contains("does not match the digest"), "{message}");
}
