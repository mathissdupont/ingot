//! Which model services this machine can reach.
//!
//! Three are built in, because most people want them and none needs
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
//! `kind` names a **protocol**, not a company. Ingot implements three, and a
//! service that speaks any of them can be reached by naming it here — which is
//! why "how many providers does Ingot support" is the wrong question. The
//! answer to the right one:
//!
//! | `kind` | Reaches |
//! |--------|---------|
//! | `openai` | OpenAI, Azure OpenAI, Ollama, vLLM, llama.cpp, LM Studio, Groq, Together, OpenRouter, Fireworks, DeepSeek, Mistral's compatible endpoint, and most hosted gateways |
//! | `anthropic` | Anthropic, and gateways that front the Messages API |
//! | `google` | Google Gemini |
//!
//! A third protocol is here for one reason: Gemini is the vendor that cannot be
//! reached by pretending to be something else. Anything that already speaks one
//! of the first two needs no code, only a `base-url`.
//!
//! # What a model can do, as opposed to where it is
//!
//! `model requires { structured_output, context >= 128k }` has to be matched
//! against something. That something is also here:
//!
//! ```toml
//! [[model.catalogue]]
//! model = "openai/gpt-5.1"
//! context = 400000
//! capabilities = ["tool_calling", "structured_output", "streaming"]
//! ```
//!
//! These facts used to be `const`s in a provider module, which had two costs. A
//! model growing a larger context window was a code change and a release. And
//! only one of the three providers had them at all — the other two refused a
//! capability requirement outright, saying they had "no catalogue to match them
//! against". This is that catalogue, and all three now consult it through
//! [`ModelConfig::resolve_capabilities`], so they cannot answer the same
//! question differently.
//!
//! `base-url` is a complete endpoint for `openai` and `anthropic`. For `google`
//! it is the API base, because that protocol puts the model and the method in
//! the path — see [`ProviderKind::base_url_is_an_endpoint`].
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
    /// The Anthropic Messages API. Spoken by Anthropic and by gateways that
    /// front it.
    Anthropic,
    /// Google's Generative Language API (Gemini).
    ///
    /// Here because it is the one a service cannot reach by pretending to be
    /// something else: it is neither of the two above.
    #[serde(alias = "gemini")]
    Google,
}

impl ProviderKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ProviderKind::Openai => "openai",
            ProviderKind::Anthropic => "anthropic",
            ProviderKind::Google => "google",
        }
    }

    /// Whether every request this protocol makes carries a credential.
    ///
    /// False only for `openai`, and only because a server on the same machine
    /// usually wants no authentication at all. Asserted here rather than
    /// rediscovered by each caller, so `doctor` and the provider builder cannot
    /// disagree about whether a declaration is complete.
    pub fn requires_authentication(self) -> bool {
        match self {
            ProviderKind::Openai => false,
            ProviderKind::Anthropic | ProviderKind::Google => true,
        }
    }

    /// Whether `base-url` is a complete endpoint or the base to build one from.
    ///
    /// Worth being explicit about rather than leaving to a reader of two
    /// provider modules: the Gemini protocol puts the model and the method in
    /// the path, so there is no one endpoint to name.
    pub fn base_url_is_an_endpoint(self) -> bool {
        match self {
            ProviderKind::Openai | ProviderKind::Anthropic => true,
            ProviderKind::Google => false,
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
    /// What each model can do, so `model requires { ... }` can be matched
    /// against something.
    ///
    /// Deployment configuration for the same reason a price is: a model's
    /// context window and capabilities change on the vendor's schedule, not on
    /// this project's release schedule. They used to be `const`s in a provider
    /// module, which meant a model growing a larger window was a code change
    /// and a release -- and that two of the three providers refused capability
    /// requirements outright, because neither had "a catalogue to match them
    /// against". This is that catalogue.
    #[serde(default, rename = "catalogue", skip_serializing_if = "Vec::is_empty")]
    pub catalogue: Vec<ModelEntry>,
}

/// What one model provides.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct ModelEntry {
    /// `vendor/model`, the same form `model exact` uses.
    ///
    /// Qualified because a bare model name would make two vendors' catalogues
    /// collide, and because matching a requirement means first knowing which
    /// vendor is answering.
    pub model: String,
    /// Context window in tokens, for `context >= N`.
    ///
    /// Absent means unknown, and an unknown window **does not satisfy** a
    /// requirement. Guessing would turn a refusal an operator can fix into a
    /// provider error at the first long prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<i64>,
    /// Capability names, as `model requires { ... }` spells them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<String>,
}

impl ModelEntry {
    /// The vendor half of `model`.
    pub fn vendor(&self) -> &str {
        self.model.split_once('/').map(|(v, _)| v).unwrap_or("")
    }

    /// The model half, which is what a provider puts on the wire.
    pub fn name(&self) -> &str {
        self.model
            .split_once('/')
            .map(|(_, name)| name)
            .unwrap_or(&self.model)
    }

    /// Whether this model satisfies a requirement.
    pub fn satisfies(&self, capabilities: &[String], min_context: Option<i64>) -> bool {
        if let Some(required) = min_context {
            match self.context {
                Some(window) if window >= required => {}
                // Unknown is not "big enough". See `context`.
                _ => return false,
            }
        }
        capabilities
            .iter()
            .all(|wanted| self.capabilities.iter().any(|has| has == wanted))
    }

    /// Why it does not, in words an operator can act on.
    pub fn shortfall(&self, capabilities: &[String], min_context: Option<i64>) -> String {
        let mut reasons = Vec::new();
        if let Some(required) = min_context {
            match self.context {
                Some(window) if window < required => {
                    reasons.push(format!("provides {window} context tokens, not {required}"))
                }
                None => reasons.push("declares no context window".to_string()),
                _ => {}
            }
        }
        let missing: Vec<&str> = capabilities
            .iter()
            .filter(|wanted| !self.capabilities.iter().any(|has| &has == wanted))
            .map(String::as_str)
            .collect();
        if !missing.is_empty() {
            reasons.push(format!("lacks {}", missing.join(", ")));
        }
        format!("{}: {}", self.model, reasons.join("; "))
    }
}

/// The models Ingot knows about without being told.
///
/// A short list, and deliberately not a directory of everything on the market:
/// a built-in catalogue is stale the moment it ships, and the operator's
/// entries come first so a stale one is always overridable. What is here is
/// what makes `model requires { … }` work out of the box for the one provider
/// that has a sensible default at all.
pub const BUILT_IN_CATALOGUE: &[(&str, i64, &[&str])] = &[(
    "anthropic/claude-opus-5",
    1_000_000,
    &[
        "tool_calling",
        "structured_output",
        "streaming",
        "vision",
        "reasoning",
        "parallel_tool_calls",
    ],
)];

impl ModelConfig {
    pub fn is_empty(&self) -> bool {
        self.providers.is_empty()
            && self.default.is_none()
            && self.prices.is_empty()
            && self.catalogue.is_empty()
    }

    /// Every model this deployment knows about, the operator's first.
    ///
    /// Order is preference order and it is the operator's: their entries are
    /// tried before the built-in ones, so overriding a built-in means declaring
    /// it, not editing this binary. Within each group, declaration order.
    pub fn known_models(&self) -> Vec<ModelEntry> {
        let mut models = self.catalogue.clone();
        for (model, context, capabilities) in BUILT_IN_CATALOGUE {
            // A declared entry for the same model wins outright, rather than
            // merging: a half-overridden model is a set of facts that came from
            // two places and matches neither.
            if models.iter().any(|entry| entry.model == *model) {
                continue;
            }
            models.push(ModelEntry {
                model: (*model).to_string(),
                context: Some(*context),
                capabilities: capabilities
                    .iter()
                    .map(|name| (*name).to_string())
                    .collect(),
            });
        }
        models
    }

    /// The first model of `vendor` that satisfies a requirement, or why none
    /// does.
    ///
    /// The one place capability matching happens, so the three providers cannot
    /// answer the same question differently -- which is what they did before
    /// this existed: one had a hardcoded default and two refused outright.
    pub fn resolve_capabilities(
        &self,
        vendor: &str,
        capabilities: &[String],
        min_context: Option<i64>,
    ) -> Result<String, String> {
        let known: Vec<ModelEntry> = self
            .known_models()
            .into_iter()
            .filter(|entry| entry.vendor() == vendor)
            .collect();

        if let Some(entry) = known
            .iter()
            .find(|entry| entry.satisfies(capabilities, min_context))
        {
            return Ok(entry.name().to_string());
        }

        let wanted = {
            let mut parts = Vec::new();
            if let Some(context) = min_context {
                parts.push(format!("context >= {context}"));
            }
            parts.extend(capabilities.iter().cloned());
            if parts.is_empty() {
                "no requirements".to_string()
            } else {
                parts.join(", ")
            }
        };

        if known.is_empty() {
            return Err(format!(
                "this artifact requires {wanted} rather than naming a model, and no model of \
                 `{vendor}` is in the catalogue\n  \
                 declare one with `[[model.catalogue]]` in ingot.toml, or pin a model with \
                 `model exact {vendor}/<model>` or --model"
            ));
        }
        Err(format!(
            "this artifact requires {wanted}, and no `{vendor}` model in the catalogue \
             provides it\n  {}\n  \
             add or correct an entry with `[[model.catalogue]]`, or pin a model with --model",
            known
                .iter()
                .map(|entry| entry.shortfall(capabilities, min_context))
                .collect::<Vec<_>>()
                .join("\n  ")
        ))
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
#[cfg(feature = "http")]
pub fn build(
    config: &ProviderConfig,
    models: &ModelConfig,
    model_override: Option<String>,
    effort: Option<String>,
) -> Result<Box<dyn crate::provider::ModelProvider>, crate::provider::ProviderError> {
    use crate::provider::ProviderError;

    // The whole manifest, not just this declaration. A declared provider used
    // to be built without one, which made `[[model.catalogue]]` invisible to
    // the endpoint it was most likely written for: the operator's own. The
    // vendor is the name they gave it, because that is the name
    // `model exact "<name>/…"` already uses and a manifest should not spell one
    // provider two ways.
    let catalogue = models.clone();
    let vendor = Some(config.name.clone());

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
                        .with_effort(effort)
                        .with_catalogue(catalogue)
                        .with_vendor(vendor),
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
                        .with_effort(effort)
                        .with_catalogue(catalogue)
                        .with_vendor(vendor),
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
        ProviderKind::Google => {
            #[cfg(feature = "google")]
            {
                let Some(key) = key else {
                    return Err(ProviderError::Configuration(format!(
                        "model provider `{}` speaks the Gemini protocol, which authenticates \
                         every request; give it an `api-key-env`",
                        config.name
                    )));
                };
                Ok(Box::new(
                    crate::google::GoogleProvider::with_key(key)
                        .with_base_url(config.base_url.clone())
                        .with_model(model_override)
                        .with_effort(effort)
                        .with_catalogue(catalogue)
                        .with_vendor(vendor),
                ))
            }
            #[cfg(not(feature = "google"))]
            {
                Err(ProviderError::Configuration(format!(
                    "model provider `{}` needs the `google` protocol, which this build does not \
                     include; rebuild with `--features google`",
                    config.name
                )))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BUILT_IN: &[&str] = &["anthropic", "google", "openai"];

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
            catalogue: Vec::new(),
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
            catalogue: Vec::new(),
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
            catalogue: Vec::new(),
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
            catalogue: Vec::new(),
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
            catalogue: Vec::new(),
            providers: vec![confused],
            ..ModelConfig::default()
        };
        let error = config.validate(BUILT_IN).unwrap_err();
        assert!(error.contains("omit it entirely"), "{error}");
    }

    fn entry(model: &str, context: Option<i64>, capabilities: &[&str]) -> ModelEntry {
        ModelEntry {
            model: model.to_string(),
            context,
            capabilities: capabilities.iter().map(|c| c.to_string()).collect(),
        }
    }

    #[test]
    fn a_model_with_a_wide_enough_window_and_the_right_capabilities_satisfies() {
        let model = entry("openai/gpt-x", Some(400_000), &["tool_calling", "vision"]);
        assert!(model.satisfies(&["vision".to_string()], Some(128_000)));
        assert!(model.satisfies(&[], None));
    }

    #[test]
    fn an_unknown_context_window_does_not_satisfy_a_requirement() {
        // Guessing would turn a refusal the operator can fix into a provider
        // error at the first long prompt, a long way from the cause.
        let model = entry("openai/gpt-x", None, &["vision"]);
        assert!(!model.satisfies(&[], Some(1)));
        assert!(model.satisfies(&["vision".to_string()], None));
        assert!(model
            .shortfall(&[], Some(1))
            .contains("declares no context window"));
    }

    #[test]
    fn a_shortfall_names_both_halves_of_what_is_missing() {
        let model = entry("openai/gpt-x", Some(8_000), &["tool_calling"]);
        let text = model.shortfall(&["vision".to_string()], Some(128_000));
        assert!(text.contains("8000"), "{text}");
        assert!(text.contains("128000"), "{text}");
        assert!(text.contains("vision"), "{text}");
    }

    #[test]
    fn an_operators_entry_replaces_a_built_in_of_the_same_name() {
        // Overriding a built-in means declaring it, not editing the binary --
        // and it replaces rather than merges, because a half-overridden model
        // is a set of facts from two places that matches neither.
        let config = ModelConfig {
            catalogue: vec![entry(
                "anthropic/claude-opus-5",
                Some(2_000_000),
                &["vision"],
            )],
            ..ModelConfig::default()
        };
        let known = config.known_models();
        let found: Vec<&ModelEntry> = known
            .iter()
            .filter(|e| e.model == "anthropic/claude-opus-5")
            .collect();
        assert_eq!(found.len(), 1, "one entry per model");
        assert_eq!(found[0].context, Some(2_000_000));
        assert_eq!(found[0].capabilities, vec!["vision".to_string()]);
    }

    #[test]
    fn the_operators_models_are_preferred_to_the_built_in_ones() {
        let config = ModelConfig {
            catalogue: vec![entry(
                "anthropic/mine",
                Some(1_000_000),
                &["structured_output"],
            )],
            ..ModelConfig::default()
        };
        let chosen = config
            .resolve_capabilities("anthropic", &["structured_output".to_string()], None)
            .expect("mine satisfies it");
        assert_eq!(chosen, "mine");
    }

    #[test]
    fn a_vendor_with_nothing_in_the_catalogue_says_what_to_declare() {
        // What OpenAI and Google used to say unconditionally, now said only
        // when it is true.
        let error = ModelConfig::default()
            .resolve_capabilities("openai", &["vision".to_string()], None)
            .unwrap_err();
        assert!(error.contains("openai"), "{error}");
        assert!(error.contains("[[model.catalogue]]"), "{error}");
    }

    #[test]
    fn a_vendor_whose_models_all_fall_short_says_how_each_one_does() {
        let config = ModelConfig {
            catalogue: vec![
                entry("openai/small", Some(8_000), &["vision"]),
                entry("openai/blind", Some(400_000), &["tool_calling"]),
            ],
            ..ModelConfig::default()
        };
        let error = config
            .resolve_capabilities("openai", &["vision".to_string()], Some(128_000))
            .unwrap_err();
        assert!(error.contains("openai/small"), "{error}");
        assert!(error.contains("openai/blind"), "{error}");
        assert!(error.contains("8000"), "{error}");
        assert!(error.contains("vision"), "{error}");
    }

    #[test]
    fn a_model_is_named_without_its_vendor_on_the_wire() {
        let model = entry("anthropic/claude-opus-5", None, &[]);
        assert_eq!(model.vendor(), "anthropic");
        assert_eq!(model.name(), "claude-opus-5");
    }

    #[test]
    fn a_default_may_name_a_built_in_without_redeclaring_it() {
        let config = ModelConfig {
            prices: Vec::new(),
            catalogue: Vec::new(),
            default: Some("anthropic".to_string()),
            providers: Vec::new(),
        };
        assert!(config.validate(BUILT_IN).is_ok());
    }

    #[test]
    fn a_default_naming_nothing_lists_what_there_is() {
        let config = ModelConfig {
            prices: Vec::new(),
            catalogue: Vec::new(),
            default: Some("mistral".to_string()),
            providers: vec![provider("local")],
        };
        let error = config.validate(BUILT_IN).unwrap_err();
        assert!(error.contains("mistral"), "{error}");
        assert!(
            error.contains("anthropic, google, local, openai"),
            "{error}"
        );
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
