//! Policy subjects and decisions.
//!
//! Ingot is **default-deny**: an effect is available only if the agent's policy
//! block grants it. An absent rule is a denial, not a permission, so a compiled
//! artifact never has more reach than its source states.

use std::fmt;

use crate::effects::Effect;

/// What a policy rule talks about.
///
/// Subjects map one-to-one onto effects, which is what lets the checker answer
/// "may this call happen?" with a single lookup.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PolicySubject {
    Network,
    FilesystemRead,
    FilesystemWrite,
    ExternalWrite,
    /// Written `secrets` in source, because the rule reads better in the plural.
    Secrets,
}

impl PolicySubject {
    pub fn from_name(name: &str) -> Option<PolicySubject> {
        let subject = match name {
            "network" => PolicySubject::Network,
            "filesystem_read" => PolicySubject::FilesystemRead,
            "filesystem_write" => PolicySubject::FilesystemWrite,
            "external_write" => PolicySubject::ExternalWrite,
            "secrets" => PolicySubject::Secrets,
            _ => return None,
        };
        Some(subject)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            PolicySubject::Network => "network",
            PolicySubject::FilesystemRead => "filesystem_read",
            PolicySubject::FilesystemWrite => "filesystem_write",
            PolicySubject::ExternalWrite => "external_write",
            PolicySubject::Secrets => "secrets",
        }
    }

    pub fn effect(self) -> Effect {
        match self {
            PolicySubject::Network => Effect::Network,
            PolicySubject::FilesystemRead => Effect::FilesystemRead,
            PolicySubject::FilesystemWrite => Effect::FilesystemWrite,
            PolicySubject::ExternalWrite => Effect::ExternalWrite,
            PolicySubject::Secrets => Effect::SecretAccess,
        }
    }

    /// The subject governing an effect, if the effect is policy-controlled.
    pub fn for_effect(effect: Effect) -> Option<PolicySubject> {
        let subject = match effect {
            Effect::Network => PolicySubject::Network,
            Effect::FilesystemRead => PolicySubject::FilesystemRead,
            Effect::FilesystemWrite => PolicySubject::FilesystemWrite,
            Effect::ExternalWrite => PolicySubject::ExternalWrite,
            Effect::SecretAccess => PolicySubject::Secrets,
            Effect::ModelAccess => return None,
        };
        Some(subject)
    }

    /// Subjects that accept a value list, such as a host allowlist.
    pub fn accepts_values(self) -> bool {
        matches!(
            self,
            PolicySubject::Network | PolicySubject::FilesystemRead | PolicySubject::FilesystemWrite
        )
    }

    /// Qualifiers a subject accepts, e.g. `secrets deny export`.
    pub fn accepted_qualifiers(self) -> &'static [&'static str] {
        match self {
            PolicySubject::Secrets => &["export"],
            _ => &[],
        }
    }

    pub fn all() -> [PolicySubject; 5] {
        [
            PolicySubject::Network,
            PolicySubject::FilesystemRead,
            PolicySubject::FilesystemWrite,
            PolicySubject::ExternalWrite,
            PolicySubject::Secrets,
        ]
    }
}

impl fmt::Display for PolicySubject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The outcome of asking the policy about an effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyDecision {
    /// Permitted outright.
    Allow,
    /// Permitted, but the compiler inserts an approval checkpoint first.
    RequireApproval,
    /// Explicitly denied by a rule.
    Deny,
    /// No rule mentions this subject. Default-deny applies, but the diagnostic
    /// differs from an explicit denial: the fix is to state the intent.
    Unspecified,
}

impl PolicyDecision {
    pub fn permits(self) -> bool {
        matches!(
            self,
            PolicyDecision::Allow | PolicyDecision::RequireApproval
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_policy_controlled_effect_has_a_subject() {
        for effect in Effect::all() {
            let subject = PolicySubject::for_effect(effect);
            if effect.is_implicitly_granted() {
                assert!(
                    subject.is_none(),
                    "{effect} should not be policy-controlled"
                );
            } else {
                assert_eq!(subject.map(PolicySubject::effect), Some(effect));
            }
        }
    }

    #[test]
    fn subject_names_round_trip() {
        for subject in PolicySubject::all() {
            assert_eq!(PolicySubject::from_name(subject.as_str()), Some(subject));
        }
    }

    #[test]
    fn secrets_is_spelled_in_the_plural_but_maps_to_secret_access() {
        assert_eq!(PolicySubject::Secrets.effect(), Effect::SecretAccess);
        assert!(PolicySubject::from_name("secret_access").is_none());
    }

    #[test]
    fn unspecified_does_not_permit() {
        assert!(!PolicyDecision::Unspecified.permits());
        assert!(!PolicyDecision::Deny.permits());
        assert!(PolicyDecision::Allow.permits());
        assert!(PolicyDecision::RequireApproval.permits());
    }
}
