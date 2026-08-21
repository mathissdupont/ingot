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

// --- the signature -------------------------------------------------------
//
// Provenance has one more way to fail silently than a checksum does: a
// verification that never happened looks exactly like one that passed. So what
// is asserted here is not that cosign works — it is that the four descriptions of
// one signature agree, because the moment they stop agreeing, every install
// quietly stops checking anything.

/// The bundle both ends have to name identically.
const BUNDLE: &str = "SHA256SUMS.sigstore.json";

/// Named, because a lone escaped backslash in an assertion reads as a typo.
const BACKSLASH: char = '\u{5c}';

#[test]
fn the_signature_is_published_under_the_name_the_installers_fetch() {
    let workflow = read(".github/workflows/release.yml");
    assert!(
        workflow.contains(&format!("--bundle dist/{BUNDLE}")),
        "the workflow no longer writes `{BUNDLE}`; the installers fetch exactly that"
    );
    for (name, script) in [
        ("install.sh", read("scripts/install.sh")),
        ("install.ps1", read("scripts/install.ps1")),
    ] {
        assert!(
            script.contains(&format!("$base/{BUNDLE}")),
            "{name} does not fetch `{BUNDLE}` from the release"
        );
    }
}

#[test]
fn the_job_that_signs_asks_for_an_identity_token() {
    // Keyless signing is the identity token and nothing else. Without this
    // permission the step fails at the certificate request — loudly, but only on
    // a tag, which is the worst moment to find out.
    let workflow = read(".github/workflows/release.yml");
    assert!(
        workflow.contains("id-token: write"),
        "nothing in the release workflow asks for an identity token, so keyless \
         signing cannot happen"
    );
}

#[test]
fn what_is_signed_is_what_is_published() {
    // The signature covers bytes, not a filename. If the publishing job rebuilt
    // `SHA256SUMS` from the per-target files, it would publish a file nobody
    // signed — and it would verify locally in CI and fail for everybody else.
    let workflow = read(".github/workflows/release.yml");
    assert_eq!(
        workflow.matches("cat *.sha256 > SHA256SUMS").count(),
        1,
        "`SHA256SUMS` is assembled more than once, so the published copy may not \
         be the copy that was signed"
    );
}

#[test]
fn the_installers_check_the_identity_of_the_workflow_that_signs() {
    // A signature verifies against *some* identity. The one that matters is this
    // repository's release workflow on a tag: anything looser would accept a
    // signature made by another workflow, another branch, or another project
    // that happens to use Sigstore. So the path in the identity has to be the
    // path of the file that actually signs, and a rename has to break this test
    // rather than every install.
    let identity_path = ".github/workflows/release.yml";
    assert!(
        repo_root().join(identity_path).is_file(),
        "the workflow the installers name does not exist"
    );

    for (name, script) in [
        ("install.sh", read("scripts/install.sh")),
        ("install.ps1", read("scripts/install.ps1")),
    ] {
        assert!(
            script.contains("certificate-identity-regexp"),
            "{name} does not pin the identity, so it would accept any Sigstore signature"
        );
        assert!(
            script.contains(r"workflows/release\.yml@refs/tags/v"),
            "{name} does not require the signature to come from {identity_path} on a tag"
        );
        assert!(
            script.contains("token.actions.githubusercontent.com"),
            "{name} does not pin the issuer, so another provider's token would do"
        );
    }
}

#[test]
fn the_identity_pattern_matches_a_whole_subject() {
    // Found by running cosign against a real signature, which is the only way it
    // could have been found. `--certificate-identity-regexp` has to match the
    // **entire** certificate subject: a pattern anchored only at the front
    // matches nothing, so the first version of this refused every signature it
    // was shown. The symptom would have been an installer that worked until
    // signing started and then stopped, for everybody, at once.
    for (name, script) in [
        ("install.sh", read("scripts/install.sh")),
        ("install.ps1", read("scripts/install.ps1")),
    ] {
        let pattern = script
            .lines()
            .find(|line| line.contains("refs/tags/v") && line.contains("workflows/release"))
            .unwrap_or_else(|| panic!("{name} has no identity pattern"));
        // With each shell's own escaping removed, so `\$` and `$` are the same
        // end-anchor to this test.
        let bare = pattern.replace(BACKSLASH, "");
        assert!(
            bare.contains(".*$"),
            "{name}'s identity pattern is not anchored at the end, so cosign matches              nothing against it:
  {pattern}"
        );
    }
}

#[test]
fn a_signature_that_fails_to_verify_cannot_be_shrugged_off() {
    // The one flag on offer turns a *missing* signature into a refusal. There is
    // deliberately none that turns a *failing* one into a warning: absence is
    // what old releases legitimately have, and a mismatch is never legitimate.
    let unix = read("scripts/install.sh");
    assert!(
        unix.contains("INGOT_REQUIRE_SIGNATURE"),
        "install.sh offers no way to demand a verified signature"
    );
    assert!(
        unix.contains("did not verify, so nothing was installed"),
        "install.sh does not refuse on a signature that fails to verify"
    );

    let windows = read("scripts/install.ps1");
    assert!(
        windows.contains("RequireSignature"),
        "install.ps1 offers no way to demand a verified signature"
    );
    assert!(
        windows.contains("did not verify, so nothing was installed"),
        "install.ps1 does not refuse on a signature that fails to verify"
    );

    for (name, script) in [
        ("install.sh", unix.as_str()),
        ("install.ps1", windows.as_str()),
    ] {
        for escape in [
            "SKIP_SIGNATURE",
            "SkipSignature",
            "NoSignature",
            "no-signature",
        ] {
            assert!(
                !script.contains(escape),
                "{name} has a `{escape}` escape hatch; a failed verification has no \
                 legitimate override"
            );
        }
    }
}
