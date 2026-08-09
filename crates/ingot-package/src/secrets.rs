//! The build-time secret scan.
//!
//! [SECURITY.md](../../../SECURITY.md) commits to secret values never entering
//! source, IR, a lockfile or a layer. That holds by construction — there is no
//! syntax for a secret literal and no path from the environment into the IR —
//! but an argument is not a test, and packaging is the moment the bytes leave
//! the machine.
//!
//! The scan is about **values, not words**. An agent may legitimately be about
//! password resets, key rotation or token budgets; refusing the word would make
//! this a check operators route around, and a check nobody runs protects
//! nothing. What it refuses is a long opaque run of characters after a
//! credential marker, which authored source has no reason to contain.
//!
//! It is a check on the author, not a security boundary. A credential shaped
//! like an English sentence passes. The property SECURITY.md states is that the
//! toolchain provides no *path* for a secret to reach an artifact; this does not
//! replace that and must not be described as if it did.

use std::fmt;

/// Where a credential-shaped value was found, and what shape it had.
///
/// It never carries the value. The point of finding one is to stop it being
/// copied, and a report that quotes it has copied it into a terminal and a CI
/// log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// 1-based line within the scanned text.
    pub line: usize,
    pub shape: &'static str,
}

/// A finding with the file it was found in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub path: String,
    pub line: usize,
    pub shape: &'static str,
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} line {} contains {}",
            self.path, self.line, self.shape
        )
    }
}

impl std::error::Error for Refusal {}

/// Refuse `text` when it carries a credential-shaped value.
pub fn check(path: &str, text: &str) -> Result<(), Refusal> {
    match scan(text) {
        Some(finding) => Err(Refusal {
            path: path.to_string(),
            line: finding.line,
            shape: finding.shape,
        }),
        None => Ok(()),
    }
}

/// The first credential-shaped value in `text`, if there is one.
pub fn scan(text: &str) -> Option<Finding> {
    for (index, line) in text.lines().enumerate() {
        if let Some(shape) = shape_of(line) {
            return Some(Finding {
                line: index + 1,
                shape,
            });
        }
    }
    None
}

/// Vendor prefixes whose presence in front of an opaque run is unambiguous.
const VENDOR_PREFIXES: &[&str] = &["sk-", "sk_live_", "ghp_", "github_pat_", "xoxb-", "aiza"];

/// Markers that only mean a credential when something opaque is assigned to them.
const ASSIGNMENT_MARKERS: &[&str] = &[
    "api_key", "apikey", "api-key", "secret", "password", "passwd", "token",
];

/// How long an opaque run has to be before it stops being a word.
const OPAQUE_MINIMUM: usize = 16;

fn shape_of(line: &str) -> Option<&'static str> {
    let lowered = line.to_ascii_lowercase();

    for prefix in VENDOR_PREFIXES {
        if let Some(position) = lowered.find(prefix) {
            if opaque_run(&line[position + prefix.len()..]) >= OPAQUE_MINIMUM {
                return Some("a vendor-prefixed API key");
            }
        }
    }

    if let Some(position) = lowered.find("bearer ") {
        if opaque_run(line[position + "bearer ".len()..].trim_start()) >= OPAQUE_MINIMUM {
            return Some("a bearer token");
        }
    }

    for marker in ASSIGNMENT_MARKERS {
        let mut from = 0usize;
        while let Some(offset) = lowered[from..].find(marker) {
            let after = from + offset + marker.len();
            from = after;
            let rest = line[after..].trim_start();
            let Some(rest) = rest
                .strip_prefix('=')
                .or_else(|| rest.strip_prefix(':'))
                .map(str::trim_start)
            else {
                continue;
            };
            if opaque_run(rest.trim_start_matches(['"', '\''])) >= OPAQUE_MINIMUM {
                return Some("an assigned credential value");
            }
        }
    }
    None
}

/// The length of the leading run of characters a secret is made of.
///
/// ASCII alphanumerics and the three separators that appear in real tokens. A
/// space or a quote ends the run, so `password: "the user's own words"` is not a
/// finding and `password: "aG9sZHRoaXNzZWNyZXQ="` is.
fn opaque_run(text: &str) -> usize {
    text.chars()
        .take_while(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '+' | '/' | '='))
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinary_words_are_not_credentials() {
        for line in [
            r#"  emit reply = ask<markdown>("Explain how to reset a password.")"#,
            r#"  emit reply = ask<markdown>("The token budget is tight.")"#,
            r#"  emit reply = ask<markdown>("Ask the user for their api key by email.")"#,
            "tool vault.read_secret(name: string) -> string !network",
            r#"pass-env = ["OPENAI_API_KEY"]"#,
            r#"{ "apiKeyEnv": "ANTHROPIC_API_KEY" }"#,
        ] {
            assert_eq!(scan(line), None, "{line}");
        }
    }

    #[test]
    fn real_credential_values_are_found() {
        for (line, shape) in [
            (
                r#"token: "ghp_16C7e42F292c6912E7710c838347Ae178B4a""#,
                "a vendor-prefixed API key",
            ),
            (
                r#"header = "Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9""#,
                "a bearer token",
            ),
            (
                r#"password = "aG9sZHRoaXNzZWNyZXR2YWx1ZQ==""#,
                "an assigned credential value",
            ),
            (
                r#"  "value": "sk-live-4f9ac1d3b7e25a86e1""#,
                "a vendor-prefixed API key",
            ),
        ] {
            assert_eq!(
                scan(line).map(|finding| finding.shape),
                Some(shape),
                "{line}"
            );
        }
    }

    #[test]
    fn the_line_is_reported_and_the_value_is_not() {
        let text = "one\ntwo\npassword = \"aG9sZHRoaXNzZWNyZXR2YWx1ZQ==\"\n";
        let refusal = check("main.ing", text).expect_err("a credential");
        assert_eq!(refusal.line, 3);
        let message = refusal.to_string();
        assert!(message.contains("main.ing line 3"), "{message}");
        assert!(
            !message.contains("aG9sZHRoaXNz"),
            "a refusal must never reproduce the value: {message}"
        );
    }

    #[test]
    fn a_clean_document_passes() {
        assert!(check("main.ing", "language 0.1\n\nagent A() -> b<markdown> {}\n").is_ok());
    }
}
