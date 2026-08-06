//! Effects: the observable, security-relevant things a program can do.
//!
//! Every tool declares its effects, every flow node carries the union of the
//! effects it can trigger, and the policy block decides which of them the agent
//! is permitted to perform. Effects are what make "what can this agent actually
//! reach?" answerable before the agent runs.

use std::fmt;

/// Variants are declared in alphabetical order of their wire names, so the
/// derived `Ord` — which drives [`EffectSet`] ordering — matches the order the
/// names appear in a canonical IR document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Effect {
    /// Side effects outside the workspace: sending mail, opening a pull request,
    /// calling a payment API. Always visible, never implicit.
    ExternalWrite,
    /// Reads from the workspace filesystem.
    FilesystemRead,
    /// Writes to the workspace filesystem.
    FilesystemWrite,
    /// Calls the language model. Implicit on every `ask`, and always permitted:
    /// an agent that may not call a model is not an agent.
    ModelAccess,
    /// Outbound network access performed by a tool.
    Network,
    /// Reads a secret value.
    SecretAccess,
}

impl Effect {
    pub fn from_name(name: &str) -> Option<Effect> {
        let effect = match name {
            "network" => Effect::Network,
            "filesystem_read" => Effect::FilesystemRead,
            "filesystem_write" => Effect::FilesystemWrite,
            "external_write" => Effect::ExternalWrite,
            "secret_access" => Effect::SecretAccess,
            "model_access" => Effect::ModelAccess,
            _ => return None,
        };
        Some(effect)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Effect::Network => "network",
            Effect::FilesystemRead => "filesystem_read",
            Effect::FilesystemWrite => "filesystem_write",
            Effect::ExternalWrite => "external_write",
            Effect::SecretAccess => "secret_access",
            Effect::ModelAccess => "model_access",
        }
    }

    /// Effects granted implicitly, without a policy rule.
    pub fn is_implicitly_granted(self) -> bool {
        matches!(self, Effect::ModelAccess)
    }

    pub fn all() -> [Effect; 6] {
        [
            Effect::ExternalWrite,
            Effect::FilesystemRead,
            Effect::FilesystemWrite,
            Effect::ModelAccess,
            Effect::Network,
            Effect::SecretAccess,
        ]
    }
}

impl fmt::Display for Effect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A sorted, duplicate-free set of effects.
///
/// Order is deterministic so that the IR it feeds is byte-for-byte reproducible.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EffectSet {
    effects: Vec<Effect>,
}

impl EffectSet {
    pub fn new() -> Self {
        EffectSet::default()
    }

    pub fn insert(&mut self, effect: Effect) {
        if let Err(index) = self.effects.binary_search(&effect) {
            self.effects.insert(index, effect);
        }
    }

    pub fn union_with(&mut self, other: &EffectSet) {
        for effect in &other.effects {
            self.insert(*effect);
        }
    }

    pub fn contains(&self, effect: Effect) -> bool {
        self.effects.binary_search(&effect).is_ok()
    }

    pub fn iter(&self) -> impl Iterator<Item = Effect> + '_ {
        self.effects.iter().copied()
    }

    pub fn is_empty(&self) -> bool {
        self.effects.is_empty()
    }

    pub fn len(&self) -> usize {
        self.effects.len()
    }

    pub fn names(&self) -> Vec<String> {
        self.effects
            .iter()
            .map(|effect| effect.as_str().to_string())
            .collect()
    }
}

impl FromIterator<Effect> for EffectSet {
    fn from_iter<T: IntoIterator<Item = Effect>>(iter: T) -> Self {
        let mut set = EffectSet::new();
        for effect in iter {
            set.insert(effect);
        }
        set
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_is_sorted_and_deduplicated() {
        let set: EffectSet = [Effect::SecretAccess, Effect::Network, Effect::Network]
            .into_iter()
            .collect();
        assert_eq!(set.len(), 2);
        assert_eq!(set.names(), vec!["network", "secret_access"]);
    }

    #[test]
    fn set_order_matches_alphabetical_wire_order() {
        let set: EffectSet = Effect::all().into_iter().collect();
        let mut sorted = set.names();
        sorted.sort();
        assert_eq!(set.names(), sorted);
    }

    #[test]
    fn insertion_order_does_not_change_the_result() {
        let a: EffectSet = [Effect::Network, Effect::ExternalWrite]
            .into_iter()
            .collect();
        let b: EffectSet = [Effect::ExternalWrite, Effect::Network]
            .into_iter()
            .collect();
        assert_eq!(a, b);
    }

    #[test]
    fn effect_names_round_trip() {
        for effect in Effect::all() {
            assert_eq!(Effect::from_name(effect.as_str()), Some(effect));
        }
    }

    #[test]
    fn only_model_access_is_implicit() {
        for effect in Effect::all() {
            assert_eq!(
                effect.is_implicitly_granted(),
                effect == Effect::ModelAccess
            );
        }
    }
}
