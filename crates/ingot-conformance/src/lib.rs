//! The conformance suite, as bytes, so that a binary can carry it.
//!
//! # Why this is a crate and not a directory
//!
//! The suite used to live at `specs/conformance`, and `ingot-cli`'s build
//! script reached two directories upwards to embed it. That worked from a
//! checkout and only from a checkout: `cargo package` carries the files under a
//! crate's own root and nothing else, so a published `ingot-cli` would have
//! built against a directory that was not there — and the build script's
//! `read_dir` failure was a silent one, so it would have built *successfully*
//! and shipped a binary whose `ingot conform` had no cases to run.
//!
//! A suite that reports nothing while looking like it reported something is
//! worse than no suite. So the cases live inside a package now, and travelling
//! with the code is a property of where they are rather than a convention
//! somebody has to keep. [`build.rs`] refuses to produce an empty [`FILES`],
//! which turns the remaining version of that failure into a build error.
//!
//! # What is in here
//!
//! Every file under `cases/`, keyed by its path relative to the suite root and
//! sorted, so the table is the same on every platform and in every build. The
//! consumer writes them back out to a directory and runs them from there; see
//! `ingot conform --export`.
//!
//! `tools/` is not included. It is a worked adapter an author reads next to the
//! specification, and embedding a Python script in every binary would serve
//! nobody.
//!
//! # What this crate deliberately does not do
//!
//! It does not run anything. The runner, the comparison and the verdict live in
//! `ingot-cli`, because running a case needs a compiler and a backend and this
//! crate should stay a pile of bytes that cannot disagree with the files it was
//! built from.
//!
//! [`build.rs`]: https://github.com/mathissdupont/ingot/blob/main/crates/ingot-conformance/build.rs

/// Every file in the suite: a suite-relative path, and its contents.
///
/// Sorted by path, and never empty — the build fails rather than emitting an
/// empty table, because an empty suite passes every backend.
pub const FILES: &[(&str, &[u8])] = generated::FILES;

mod generated {
    include!(concat!(env!("OUT_DIR"), "/conformance_suite.rs"));
}

/// The number of files the suite carries.
///
/// Here so that a caller can say "this binary has no suite" without walking the
/// table, and so that the const below has something to assert on.
pub fn file_count() -> usize {
    FILES.len()
}

/// The suite is not empty. Checked at compile time as well as in `build.rs`,
/// because the two failures have different causes: the build script can only
/// see the directory it read, and this sees the table that reached the binary.
const _: () = assert!(
    !FILES.is_empty(),
    "the embedded conformance suite is empty; see crates/ingot-conformance/build.rs"
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_suite_is_here() {
        assert!(file_count() > 0, "no files embedded");
    }

    #[test]
    fn every_case_carries_the_files_a_run_needs() {
        let mut cases: Vec<&str> = FILES
            .iter()
            .filter_map(|(path, _)| path.strip_prefix("cases/"))
            .filter_map(|rest| rest.split('/').next())
            .collect();
        cases.sort_unstable();
        cases.dedup();
        assert!(!cases.is_empty(), "no cases embedded");

        for case in cases {
            for needed in ["case.toml", "agent.ir.json", "main.ing"] {
                let wanted = format!("cases/{case}/{needed}");
                assert!(
                    FILES.iter().any(|(path, _)| *path == wanted),
                    "{case} is missing {needed}"
                );
            }
        }
    }

    #[test]
    fn paths_are_relative_and_use_forward_slashes() {
        for (path, _) in FILES {
            assert!(!path.contains('\\'), "{path} has a backslash");
            assert!(!path.starts_with('/'), "{path} is absolute");
            assert!(!path.contains(".."), "{path} escapes the suite");
        }
    }

    #[test]
    fn tools_are_not_embedded() {
        for (path, _) in FILES {
            assert!(
                !path.starts_with("tools/"),
                "{path} is a worked example, not a case"
            );
        }
    }
}
