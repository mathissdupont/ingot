//! The session token, and an honest account of where it comes from.
//!
//! A loopback server is reachable by every process on the machine and, through
//! a page in the same browser, by every site the person is visiting. The token
//! is what makes "on this machine" mean "started by this person": it exists for
//! the length of one `ingot studio` process, appears in the URL that process
//! prints, and is never written anywhere.
//!
//! It is one of three checks and not the only one. [`crate`] also refuses a
//! request whose `Host` is not a loopback authority naming this port, which is
//! what stops a name resolving here from being treated as here, and refuses a
//! cross-site `Origin`. A guess at the token still has to arrive past both.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};

/// A fresh 128-bit token, as 32 hexadecimal characters.
///
/// The entropy is the operating system's: `RandomState`'s keys are seeded from
/// the platform random source the first time a thread builds one. Standard
/// library internals are not a stability promise, so this is deliberately not
/// the only thing standing between a stranger and the reports — see the module
/// note above for the other two checks.
pub fn fresh() -> String {
    let state = RandomState::new();
    let mut token = String::with_capacity(32);
    for round in 0..2u64 {
        let mut hasher = state.build_hasher();
        hasher.write_u64(round);
        token.push_str(&format!("{:016x}", hasher.finish()));
    }
    token
}

/// Compare without returning early on the first differing byte.
///
/// The timing of a local comparison is not a realistic way in, but a token
/// check written the obvious way is the kind of thing that gets copied into a
/// place where it is.
pub fn matches(expected: &str, offered: &str) -> bool {
    if expected.len() != offered.len() {
        return false;
    }
    let mut difference = 0u8;
    for (left, right) in expected.bytes().zip(offered.bytes()) {
        difference |= left ^ right;
    }
    difference == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_tokens_are_not_the_same_token() {
        assert_ne!(fresh(), fresh());
    }

    #[test]
    fn a_token_is_thirty_two_hexadecimal_characters() {
        let token = fresh();
        assert_eq!(token.len(), 32);
        assert!(token.chars().all(|character| character.is_ascii_hexdigit()));
    }

    #[test]
    fn a_prefix_is_not_a_match() {
        let token = fresh();
        assert!(matches(&token, &token));
        assert!(!matches(&token, &token[..16]));
        assert!(!matches(&token, ""));
    }
}
