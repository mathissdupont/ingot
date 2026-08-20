//! The installers and the release workflow, held to each other.
//!
//! `scripts/install.sh` and `scripts/install.ps1` reconstruct an archive's name
//! and its contents from scratch. Nothing connects them to
//! `.github/workflows/release.yml`, which is what actually produces those
//! archives — so a rename on either side leaves an installer that downloads a
//! 404, and the symptom appears only to somebody trying to install the thing.
//!
//! These are equalities rather than a parse: the point is that the two
//! descriptions of one artifact agree, not that either is well-formed.

use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("the workspace root is two above this crate")
        .to_path_buf()
}

fn read(relative: &str) -> String {
    let path = repo_root().join(relative);
    std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("reading {relative}: {error}"))
}

/// Every `target:` in the release matrix.
fn released_targets(workflow: &str) -> Vec<String> {
    workflow
        .lines()
        .filter_map(|line| line.trim().strip_prefix("- target: "))
        .map(|target| target.trim().to_string())
        .collect()
}

#[test]
fn the_installers_and_the_workflow_agree_about_the_archive_name() {
    // The workflow builds `ingot-<version>-<target>`, and both installers
    // rebuild that string. If one of them ever gains or loses a piece, the
    // download is a 404.
    let workflow = read(".github/workflows/release.yml");
    assert!(
        workflow.contains(
            r#"name="ingot-${{ needs.check-version.outputs.version }}-${{ matrix.target }}""#
        ),
        "the workflow no longer builds `ingot-<version>-<target>`; the installers assume it does"
    );

    let unix = read("scripts/install.sh");
    assert!(
        unix.contains(r#"name="ingot-$version-$target""#),
        "install.sh no longer builds `ingot-<version>-<target>`"
    );

    let windows = read("scripts/install.ps1");
    assert!(
        windows.contains(r#"$name = "ingot-$Version-$target""#),
        "install.ps1 no longer builds `ingot-<version>-<target>`"
    );
}

#[test]
fn every_released_target_is_one_an_installer_can_ask_for() {
    // A target added to the matrix that no installer can name is a build
    // nobody can install by the documented route.
    let workflow = read(".github/workflows/release.yml");
    let unix = read("scripts/install.sh");
    let windows = read("scripts/install.ps1");

    let targets = released_targets(&workflow);
    assert!(
        targets.len() >= 5,
        "expected the five shipped targets, found {targets:?}"
    );

    for target in &targets {
        // The scripts assemble a triple from `uname` rather than listing it, so
        // this checks the pieces they assemble it from.
        let (arch, rest) = target
            .split_once('-')
            .unwrap_or_else(|| panic!("`{target}` is not a target triple"));
        let covered = if rest == "pc-windows-msvc" {
            windows.contains(rest) && windows.contains(arch)
        } else {
            unix.contains(rest) && unix.contains(arch)
        };
        assert!(
            covered,
            "nothing in the installers can produce `{target}`; it ships and cannot be installed"
        );
    }
}

#[test]
fn the_installers_place_every_binary_the_archive_carries() {
    // The archive is three permissions to grant a machine, and the point of one
    // installer is that it grants them together. A binary added to the archive
    // and not to the installers is one nobody who used the one-liner has.
    let workflow = read(".github/workflows/release.yml");
    let unix = read("scripts/install.sh");
    let windows = read("scripts/install.ps1");

    for binary in ["ingot", "ingot-mcp-fs", "ingot-lsp"] {
        assert!(
            workflow.contains(&format!("release/{binary} ")),
            "the workflow no longer archives `{binary}`"
        );
        assert!(
            unix.contains(binary),
            "install.sh does not install `{binary}`"
        );
        assert!(
            windows.contains(&format!("{binary}.exe")),
            "install.ps1 does not install `{binary}.exe`"
        );
    }
}

#[test]
fn neither_installer_trusts_a_download_it_did_not_check() {
    // The one property that must not be refactored away: an archive whose hash
    // does not match `SHA256SUMS` is not unpacked, and there is no flag that
    // says otherwise.
    for (name, script) in [
        ("install.sh", read("scripts/install.sh")),
        ("install.ps1", read("scripts/install.ps1")),
    ] {
        assert!(
            script.contains("SHA256SUMS"),
            "{name} does not fetch the checksums"
        );
        assert!(
            script.contains("nothing was installed"),
            "{name} has no refusal for a hash that does not match"
        );
        for escape in [
            "--no-verify",
            "skip-verify",
            "SkipVerify",
            "INGOT_SKIP_VERIFY",
        ] {
            assert!(
                !script.contains(escape),
                "{name} grew `{escape}`, which is a way to install something unchecked"
            );
        }
    }
}

#[test]
fn the_installers_do_not_reach_for_an_endpoint_that_answers_404_here() {
    // Every release is marked as a pre-release, and GitHub's `releases/latest`
    // excludes those — it answers 404 for this project. An installer that asks
    // it cannot resolve a version at all, which is the first wall this work hit.
    for (name, script) in [
        ("install.sh", read("scripts/install.sh")),
        ("install.ps1", read("scripts/install.ps1")),
    ] {
        // Matched on the form that *fetches* it — the URL, ending in its
        // closing quote — rather than on the substring, because both scripts
        // carry a comment saying why they do not use this endpoint, and a test
        // that cannot tell an explanation from a call gets deleted.
        assert!(
            !script.contains("releases/latest\""),
            "{name} asks for `releases/latest`, which is 404 while releases are pre-releases"
        );
        assert!(
            script.contains("releases?per_page=1"),
            "{name} has no way to find the newest release"
        );
    }

    // And the flag that makes it so is still there, so this test keeps meaning
    // what it says.
    assert!(
        read(".github/workflows/release.yml").contains("--prerelease"),
        "releases are no longer pre-releases: `releases/latest` works now, and the \
         installers could use it"
    );
}
