//! Which model services this machine can reach.
//!
//! Two are built in, because most people want them and neither needs
//! configuring. Everything else is declared by the operator:
//!
//! ```toml
//! [model]
//! default = "local"
//!
//! [[model.provider]]
//! name = "local"                 # the vendor half of `model exact "local/…"`
//! kind = "openai"                # the wire protocol it speaks
//! base-url = "http://localhost:11434/v1/chat/completions"
//!
//! [[model.provider]]
//! name = "azure"
//! kind = "openai"
//! base-url = "https://…/chat/completions?api-version=2024-10-21"
//! api-key-env = "AZURE_OPENAI_KEY"
//! ```
//!
//! `kind` names a **protocol**, not a company. Ingot implements two, and a
//! service that speaks either can be reached by naming it here — which is why
//! "how many providers does Ingot support" is the wrong question.
//!
//! `api-key-env` names an environment variable. There is no way to write a key
//! into a manifest, for the same reason there is none in `[[mcp.server]]`: a
//! manifest is committed. A provider with no `api-key-env` sends no
//! authentication at all, which is what a local server usually wants.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

/// The wire protocols Ingot speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProviderKind {
    /// OpenAI Chat Completions. Spoken by OpenAI, Azure OpenAI, vLLM,
    /// llama.cpp, Ollama, LM Studio, and most hosted gateways.
    #[serde(alias = "openai-compatible")]
    Openai,
    /// The Anthropic Messages API.
    Anthropic,
}

impl ProviderKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ProviderKind::Openai => "openai",
            ProviderKind::Anthropic => "anthropic",
        }
    }
}

/// One service the operator declared.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct ProviderConfig {
    /// The vendor half of a pinned reference: `model exact "<name>/<model>"`.
    pub name: String,
    /// Which protocol to speak.
    pub kind: ProviderKind,
    /// The endpoint, in full. Not a host: services disagree about the path,
    /// and guessing it produces a 404 that looks like a missing model.
    pub base_url: String,
    /// The **name** of the variable holding the key. Absent means no auth,
    /// which is what a local server usually wants.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_env: Option<String>,
}

/// The `[model]` section of a manifest.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct ModelConfig {
    /// Which provider answers a call the artifact did not pin to a vendor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    #[serde(default, rename = "provider", skip_serializing_if = "Vec::is_empty")]
    pub providers: Vec<ProviderConfig>,
    /// What each model costs, so `budget.cost` can be charged.
    ///
    /// Deployment configuration rather than part of the program, for the same
    /// reason `[[mcp.server]]` is: a price is provider- and time-dependent, and
    /// an artifact carrying one would be stale the moment it was published.
    #[serde(default, rename = "price", skip_serializing_if = "Vec::is_empty")]
    pub prices: Vec<crate::price::ModelPrice>,
}

impl ModelConfig {
    pub fn is_empty(&self) -> bool {
        self.providers.is_empty() && self.default.is_none() && self.prices.is_empty()
    }

    /// The prices a run is given, as the interpreter wants them.
    pub fn pricing(&self) -> crate::price::Pricing {
        crate::price::Pricing::new(self.prices.clone())
    }

    /// Reject what cannot work, before a key is read or a request is built.
    ///
    /// `built_in` are the names available without being declared, so that
    /// `default = "anthropic"` is accepted without an `[[model.provider]]`
    /// entry restating what Ingot already knows.
    pub fn validate(&self, built_in: &[&str]) -> Result<(), String> {
        let mut seen: BTreeSet<&str> = BTreeSet::new();

        for provider in &self.providers {
            let name = provider.name.trim();
            if name.is_empty() {
                return Err("an [[model.provider]] has an empty `name`".to_string());
            }
            if name.contains('/') {
                return Err(format!(
                    "the provider name `{name}` contains `/`, which separates the vendor from \
                     the model in `model exact \"vendor/model\"`"
                ));
            }
            if provider.base_url.trim().is_empty() {
                return Err(format!("model provider `{name}` has an empty `base-url`"));
            }
            if !seen.insert(name) {
                return Err(format!(
                    "two [[model.provider]] entries are both named `{name}`; names must be unique"
                ));
            }
            if let Some(variable) = &provider.api_key_env {
                if variable.trim().is_empty() {
                    return Err(format!(
                        "model provider `{name}` has an empty `api-key-env`; omit it entirely to \
                         send no authentication"
                    ));
                }
            }
        }

        if let Some(default) = &self.default {
            let known = seen.contains(default.as_str()) || built_in.contains(&default.as_str());
            if !known {
                let mut all: Vec<&str> = seen
                    .iter()
                    .copied()
                    .chain(built_in.iter().copied())
                    .collect();
                all.sort_unstable();
                all.dedup();
                return Err(format!(
                    "`default = \"{default}\"` names no provider\n  declared or built in: {}",
                    all.join(", ")
                ));
            }
        }

        Ok(())
    }
}

/// Build the provider a declaration describes.
///
/// Separate from the declaration so that a manifest can be read, validated and
/// printed on a build with no HTTP support at all.
#[cfg(any(feature = "anthropic", feature = "openai"))]
pub fn build(
    config: &ProviderConfig,
    model_override: Option<String>,
    effort: Option<String>,
) -> Result<Box<dyn crate::provider::ModelProvider>, crate::provider::ProviderError> {
    use crate::provider::ProviderError;

    let key = match &config.api_key_env {
        Some(variable) => Some(crate::http::key_from_env(variable)?),
        None => None,
    };

    match config.kind {
        ProviderKind::Openai => {
            #[cfg(feature = "openai")]
            {
                let provider = match key {
                    Some(key) => crate::openai::OpenAiProvider::with_key(key),
                    None => crate::openai::OpenAiProvider::without_key(),
                };
                Ok(Box::new(
                    provider
                        .with_base_url(config.base_url.clone())
                        .with_model(model_override)
                        .with_effort(effort),
                ))
            }
            #[cfg(not(feature = "openai"))]
            {
                Err(ProviderError::Configuration(format!(
                    "model provider `{}` needs the `openai` protocol, which this build does not \
                     include; rebuild with `--features openai`",
                    config.name
                )))
            }
        }
        ProviderKind::Anthropic => {
            #[cfg(feature = "anthropic")]
            {
                let Some(key) = key else {
                    return Err(ProviderError::Configuration(format!(
                        "model provider `{}` speaks the Anthropic protocol, which authenticates \
                         every request; give it an `api-key-env`",
                        config.name
                    )));
                };
                Ok(Box::new(
                    crate::anthropic::AnthropicProvider::with_key(key)
                        .with_base_url(config.base_url.clone())
                        .with_model(model_override)
                        .with_effort(effort),
                ))
            }
            #[cfg(not(feature = "anthropic"))]
            {
                Err(ProviderError::Configuration(format!(
                    "model provider `{}` needs the `anthropic` protocol, which this build does \
                     not include; rebuild with `--features anthropic`",
                    config.name
                )))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BUILT_IN: &[&str] = &["anthropic", "openai"];

    fn provider(name: &str) -> ProviderConfig {
        ProviderConfig {
            name: name.to_string(),
            kind: ProviderKind::Openai,
            base_url: "http://localhost:11434/v1/chat/completions".to_string(),
            api_key_env: None,
        }
    }

    #[test]
    fn an_absent_section_declares_nothing() {
        let config = ModelConfig::default();
        assert!(config.is_empty());
        assert!(config.validate(BUILT_IN).is_ok());
    }

    #[test]
    fn a_local_server_needs_no_key() {
        // The common case for Ollama, llama.cpp or LM Studio: no auth at all.
        let config = ModelConfig {
            prices: Vec::new(),
            providers: vec![provider("local")],
            ..ModelConfig::default()
        };
        assert!(config.validate(BUILT_IN).is_ok());
        assert!(config.providers[0].api_key_env.is_none());
    }

    #[test]
    fn duplicate_names_are_refused() {
        let config = ModelConfig {
            prices: Vec::new(),
            providers: vec![provider("local"), provider("local")],
            ..ModelConfig::default()
        };
        let error = config.validate(BUILT_IN).unwrap_err();
        assert!(error.contains("unique"), "{error}");
    }

    #[test]
    fn a_name_containing_a_slash_is_refused_because_that_is_the_separator() {
        let config = ModelConfig {
            prices: Vec::new(),
            providers: vec![provider("my/llm")],
            ..ModelConfig::default()
        };
        let error = config.validate(BUILT_IN).unwrap_err();
        assert!(error.contains("separates the vendor"), "{error}");
    }

    #[test]
    fn an_empty_endpoint_is_refused_before_a_request_is_built() {
        let mut bare = provider("local");
        bare.base_url = "  ".to_string();
        let config = ModelConfig {
            prices: Vec::new(),
            providers: vec![bare],
            ..ModelConfig::default()
        };
        assert!(config.validate(BUILT_IN).is_err());
    }

    #[test]
    fn an_empty_key_variable_is_refused_rather_than_read_as_no_auth() {
        // Omitting the field means no auth; setting it to "" is a mistake.
        let mut confused = provider("local");
        confused.api_key_env = Some(String::new());
        let config = ModelConfig {
            prices: Vec::new(),
            providers: vec![confused],
            ..ModelConfig::default()
        };
        let error = config.validate(BUILT_IN).unwrap_err();
        assert!(error.contains("omit it entirely"), "{error}");
    }

    #[test]
    fn a_default_may_name_a_built_in_without_redeclaring_it() {
        let config = ModelConfig {
            prices: Vec::new(),
            default: Some("anthropic".to_string()),
            providers: Vec::new(),
        };
        assert!(config.validate(BUILT_IN).is_ok());
    }

    #[test]
    fn a_default_naming_nothing_lists_what_there_is() {
        let config = ModelConfig {
            prices: Vec::new(),
            default: Some("mistral".to_string()),
            providers: vec![provider("local")],
        };
        let error = config.validate(BUILT_IN).unwrap_err();
        assert!(error.contains("mistral"), "{error}");
        assert!(error.contains("anthropic, local, openai"), "{error}");
    }

    #[test]
    fn kind_names_a_protocol_and_accepts_the_longer_spelling() {
        let config: ModelConfig = toml::from_str(
            r#"
            [[provider]]
            name = "local"
            kind = "openai-compatible"
            base-url = "http://localhost:8000/v1/chat/completions"
            "#,
        )
        .expect("must parse");
        assert_eq!(config.providers[0].kind, ProviderKind::Openai);
    }

    #[test]
    fn an_unknown_key_is_refused_rather_than_ignored() {
        // A typo that is silently dropped is how someone ends up believing a
        // key was forwarded when it was not.
        let error = toml::from_str::<ModelConfig>(
            r#"
            [[provider]]
            name = "local"
            kind = "openai"
            base-url = "http://localhost:8000/v1"
            api_key = "sk-secret"
            "#,
        )
        .expect_err("an unknown key must fail");
        assert!(error.to_string().contains("api_key"), "{error}");
    }
}
