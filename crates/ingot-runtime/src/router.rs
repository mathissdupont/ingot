//! Sending each call to the provider the artifact asked for.
//!
//! `model exact "openai/gpt-5.1"` names two things: a vendor and a model. The
//! model half is the provider's business; the vendor half decides *which*
//! provider, and that decision belongs here rather than in a flag somebody has
//! to remember.
//!
//! Deliberately vendor-neutral: this knows about prefixes, not about OpenAI or
//! Anthropic. A backend that adds a third provider registers a prefix and
//! changes nothing else.

use std::collections::BTreeMap;

use crate::provider::{
    CompletionRequest, CompletionResponse, ModelProvider, ModelSelection, ProviderError,
};

/// Dispatches on the `vendor/` prefix of a pinned model reference.
///
/// The routes are `Send` because a router is what a live run usually holds, and a
/// fan-out builds one per overlapping iteration — see
/// [`ProviderFactory`](crate::provider::ProviderFactory). Nothing here is called
/// from two threads at once; each thread has its own router.
pub struct RoutingProvider {
    routes: BTreeMap<String, Box<dyn ModelProvider + Send>>,
    /// Used when the artifact pins nothing, or pins a vendor with no route.
    fallback: Option<Box<dyn ModelProvider + Send>>,
    fallback_name: String,
    /// The name reported for the last call, so a run log names the provider
    /// that actually answered rather than "router".
    last: String,
}

impl RoutingProvider {
    pub fn new() -> RoutingProvider {
        RoutingProvider {
            routes: BTreeMap::new(),
            fallback: None,
            fallback_name: "none".to_string(),
            last: "router".to_string(),
        }
    }

    /// Register the provider for a vendor prefix.
    pub fn with(
        mut self,
        vendor: impl Into<String>,
        provider: Box<dyn ModelProvider + Send>,
    ) -> Self {
        self.routes.insert(vendor.into(), provider);
        self
    }

    /// The provider for a call that names no vendor.
    ///
    /// `vendor` is the operator's label for it, which is not the same as the
    /// provider's own name: a service called `local` may well speak the OpenAI
    /// protocol, and an artifact that pins `local/…` means the label.
    pub fn or_else(
        mut self,
        vendor: impl Into<String>,
        provider: Box<dyn ModelProvider + Send>,
    ) -> Self {
        self.fallback_name = vendor.into();
        self.fallback = Some(provider);
        self
    }

    /// Vendors this router can reach, in name order.
    ///
    /// Includes the fallback's own vendor: a run with one provider can serve
    /// `anthropic/claude-…` even though nothing was registered under a prefix,
    /// because the fallback knows what it is.
    pub fn vendors(&self) -> Vec<&str> {
        let mut vendors: Vec<&str> = self.routes.keys().map(String::as_str).collect();
        if self.fallback.is_some() && !vendors.contains(&self.fallback_name.as_str()) {
            vendors.push(&self.fallback_name);
        }
        vendors.sort_unstable();
        vendors
    }

    pub fn is_empty(&self) -> bool {
        self.routes.is_empty() && self.fallback.is_none()
    }

    /// One line for the run log, so which service answered is never a guess.
    pub fn describe(&self) -> String {
        match (self.routes.is_empty(), self.fallback.is_some()) {
            (true, true) => format!("model calls go to {}", self.fallback_name),
            (_, true) => format!(
                "model calls route by vendor ({}), otherwise {}",
                self.vendors().join(", "),
                self.fallback_name
            ),
            (_, false) => format!(
                "model calls route by vendor ({}); the artifact must pin one",
                self.vendors().join(", ")
            ),
        }
    }

    /// The vendor half of a pinned reference, if there is one.
    fn vendor_of(selection: &ModelSelection) -> Option<&str> {
        match selection {
            ModelSelection::Exact(reference) => reference.split_once('/').map(|(vendor, _)| vendor),
            _ => None,
        }
    }

    fn pick(
        &mut self,
        selection: &ModelSelection,
    ) -> Result<&mut dyn ModelProvider, ProviderError> {
        if let Some(vendor) = RoutingProvider::vendor_of(selection) {
            let vendor = vendor.to_string();
            if self.routes.contains_key(&vendor) {
                let provider = self.routes.get_mut(&vendor).expect("just checked");
                return Ok(provider.as_mut());
            }
            if self.fallback.is_some() && self.fallback_name == vendor {
                return Ok(self
                    .fallback
                    .as_mut()
                    .map(Box::as_mut)
                    .expect("checked immediately above"));
            }
            // Naming a vendor nobody serves is an error, not a reason to send
            // the call somewhere else: an artifact that says `openai/…` must
            // not quietly reach Anthropic and produce a plausible answer.
            let reachable = match self.vendors() {
                vendors if vendors.is_empty() => "none".to_string(),
                vendors => vendors.join(", "),
            };
            return Err(ProviderError::Configuration(format!(
                "the artifact pins `{vendor}/…`, and this run has no provider for `{vendor}`\n  \
                 available: {reachable}\n  \
                 export that vendor's key, or override the model with --model"
            )));
        }

        if self.fallback.is_none() {
            let reachable = if self.routes.is_empty() {
                "nothing".to_string()
            } else {
                self.vendors().join(", ")
            };
            return Err(ProviderError::Configuration(format!(
                "this artifact names no vendor and no default provider is configured\n  \
                 pin one with `model exact \"<vendor>/<model>\"`; this run can reach: {reachable}"
            )));
        }
        Ok(self
            .fallback
            .as_mut()
            .map(Box::as_mut)
            .expect("checked immediately above"))
    }
}

impl Default for RoutingProvider {
    fn default() -> Self {
        RoutingProvider::new()
    }
}

impl ModelProvider for RoutingProvider {
    fn name(&self) -> &str {
        &self.last
    }

    fn complete(
        &mut self,
        request: &CompletionRequest,
    ) -> Result<CompletionResponse, ProviderError> {
        let provider = self.pick(&request.model)?;
        let name = provider.name().to_string();
        let response = provider.complete(request);
        self.last = name;
        response
    }

    /// Only when every route streams.
    ///
    /// The interpreter asks this before a request exists, so the answer has to
    /// hold for whichever route ends up answering. One provider that cannot
    /// stream therefore keeps the smaller output ceiling for all of them —
    /// conservative on purpose, because the alternative is an artifact that
    /// asks for more tokens than a service accepts and fails on a route the
    /// operator did not think they were using.
    fn streams(&self) -> bool {
        !self.is_empty()
            && self.routes.values().all(|provider| provider.streams())
            && self
                .fallback
                .as_ref()
                .is_none_or(|provider| provider.streams())
    }

    fn complete_streaming(
        &mut self,
        request: &CompletionRequest,
        on_delta: crate::provider::DeltaSink<'_>,
    ) -> Result<CompletionResponse, ProviderError> {
        let provider = self.pick(&request.model)?;
        let name = provider.name().to_string();
        let response = provider.complete_streaming(request, on_delta);
        self.last = name;
        response
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::Usage;
    use serde_json::{json, Value};

    /// Answers with its own name, so a test can see who was reached.
    struct Echo(&'static str);

    impl ModelProvider for Echo {
        fn name(&self) -> &str {
            self.0
        }
        fn complete(
            &mut self,
            _request: &CompletionRequest,
        ) -> Result<CompletionResponse, ProviderError> {
            Ok(CompletionResponse {
                value: Value::String(self.0.to_string()),
                usage: Usage::default(),
                model: self.0.to_string(),
            })
        }
    }

    fn request(model: ModelSelection) -> CompletionRequest {
        CompletionRequest {
            node: "n0".into(),
            model,
            system: None,
            prompt: "hello".into(),
            context: Vec::new(),
            response_type: "markdown".into(),
            shape: crate::schema::ResponseShape::Prose,
            max_tokens: 100,
        }
    }

    fn router() -> RoutingProvider {
        RoutingProvider::new()
            .with("openai", Box::new(Echo("openai")))
            .with("anthropic", Box::new(Echo("anthropic")))
    }

    #[test]
    fn a_pinned_vendor_decides_which_provider_answers() {
        let mut router = router();
        for (reference, expected) in [
            ("openai/gpt-test", "openai"),
            ("anthropic/claude-test", "anthropic"),
        ] {
            let response = router
                .complete(&request(ModelSelection::Exact(reference.into())))
                .unwrap();
            assert_eq!(response.value, json!(expected));
            assert_eq!(router.name(), expected, "the run log names who answered");
        }
    }

    #[test]
    fn a_vendor_nobody_serves_is_an_error_rather_than_a_silent_substitution() {
        // An artifact that says `mistral/…` must not quietly reach Anthropic
        // and come back with a plausible answer from the wrong model.
        let error = router()
            .complete(&request(ModelSelection::Exact("mistral/large".into())))
            .unwrap_err();
        let text = error.to_string();
        assert!(text.contains("mistral"), "{text}");
        assert!(text.contains("anthropic, openai"), "{text}");
    }

    #[test]
    fn an_unpinned_call_goes_to_the_fallback() {
        let mut router = router().or_else("anthropic", Box::new(Echo("anthropic")));
        let response = router.complete(&request(ModelSelection::Default)).unwrap();
        assert_eq!(response.value, json!("anthropic"));
    }

    #[test]
    fn an_unpinned_call_with_no_fallback_says_how_to_pin_one() {
        let error = router()
            .complete(&request(ModelSelection::Default))
            .unwrap_err();
        let text = error.to_string();
        assert!(text.contains("model exact"), "{text}");
        assert!(text.contains("anthropic, openai"), "{text}");
    }

    #[test]
    fn a_single_provider_serves_its_own_vendor_without_being_registered_twice() {
        // The common case: one key exported. `model exact "anthropic/claude-x"`
        // must reach it, and so must an artifact that pins nothing.
        let mut router = RoutingProvider::new().or_else("anthropic", Box::new(Echo("anthropic")));
        assert_eq!(router.vendors(), vec!["anthropic"]);

        let pinned = router
            .complete(&request(ModelSelection::Exact("anthropic/claude-x".into())))
            .unwrap();
        assert_eq!(pinned.value, json!("anthropic"));

        let unpinned = router.complete(&request(ModelSelection::Default)).unwrap();
        assert_eq!(unpinned.value, json!("anthropic"));

        // And it still refuses a vendor it is not.
        let error = router
            .complete(&request(ModelSelection::Exact("openai/gpt-x".into())))
            .unwrap_err();
        assert!(
            error.to_string().contains("no provider for `openai`"),
            "{error}"
        );
    }

    #[test]
    fn several_providers_and_no_fallback_ask_the_artifact_to_pin() {
        let described = router().describe();
        assert!(described.contains("must pin one"), "{described}");
    }

    #[test]
    fn a_bare_model_name_is_not_read_as_a_vendor() {
        // `model exact "gpt-test"` names a model, not a vendor, so it goes to
        // the fallback rather than looking for a `gpt-test` provider.
        let mut router = router().or_else("anthropic", Box::new(Echo("anthropic")));
        let response = router
            .complete(&request(ModelSelection::Exact("claude-test".into())))
            .unwrap();
        assert_eq!(response.value, json!("anthropic"));
    }

    #[test]
    fn the_description_says_what_this_run_can_reach() {
        let described = router()
            .or_else("anthropic", Box::new(Echo("anthropic")))
            .describe();
        assert!(described.contains("anthropic, openai"), "{described}");
    }

    #[test]
    fn an_empty_router_is_empty() {
        assert!(RoutingProvider::new().is_empty());
        assert!(!router().is_empty());
    }
}
