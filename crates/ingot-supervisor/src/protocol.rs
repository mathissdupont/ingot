//! The wire format between a contained run and its supervisor.
//!
//! Newline-delimited JSON, one object per line, UTF-8. Initiative flows one way:
//! **the guest calls, the host answers.** The host never initiates, which is why
//! a guest is a loop rather than a state machine, and why there is no
//! interleaving problem of the kind [`ingot_mcp`] has to solve.
//!
//! # Why the request types are mirrored rather than reused
//!
//! [`ingot_runtime::CompletionRequest`] and its parts are in-process types. They
//! change when the interpreter needs them to, and adding `Serialize` to them
//! would quietly make every future variant a compatibility question. The mirrors
//! here are the wire contract, versioned by [`PROTOCOL_VERSION`], and the
//! conversions in both directions are exercised by round-trip tests — so a new
//! variant on either side is a compile error rather than a field that silently
//! stops crossing.

use std::collections::BTreeMap;

use ingot_ir::AgentIr;
use ingot_mcp::McpConfig;
use ingot_runtime::provider::{
    CompletionRequest, CompletionResponse, ModelSelection, ProviderError, Usage,
};
use ingot_runtime::schema::ResponseShape;
use ingot_runtime::{ApprovalRequest, Artifact, ConsultRequest};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The only protocol version there is.
///
/// Checked rather than negotiated: in normal use the host and the guest are the
/// same binary, and a mismatch means the image was built from a different source
/// than the host — exactly the situation where guessing is worst.
pub const PROTOCOL_VERSION: u32 = 1;

/// Method names. Constants because both halves must agree on the spelling and a
/// typo in one of them would be a runtime mystery.
pub const CALL_CONFIG: &str = "config";
pub const CALL_MODEL: &str = "model";
pub const CALL_APPROVAL: &str = "approval";
pub const CALL_CONSULT: &str = "consult";
pub const NOTIFY_EVENT: &str = "event";
pub const NOTIFY_FINISHED: &str = "finished";
pub const NOTIFY_FAILED: &str = "failed";

/// One line from the guest, before its payload is interpreted.
///
/// Deliberately permissive, in the same shape as
/// [`ingot_mcp::jsonrpc::Incoming`]: a line naming a method the host does not
/// implement must produce "the guest called `frobnicate`" rather than "data did
/// not match any variant". The guest may be a different build of `ingot`, and
/// that is precisely when a legible error earns its keep.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GuestLine {
    /// Present when the guest is waiting for a reply, absent on a notification.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seq: Option<u64>,
    /// A call the host must answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call: Option<String>,
    /// A notification the host must not answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notify: Option<String>,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub params: Value,
}

impl GuestLine {
    pub fn call(seq: u64, method: &str, params: Value) -> GuestLine {
        GuestLine {
            seq: Some(seq),
            call: Some(method.to_string()),
            notify: None,
            params,
        }
    }

    pub fn notify(method: &str, params: Value) -> GuestLine {
        GuestLine {
            seq: None,
            call: None,
            notify: Some(method.to_string()),
            params,
        }
    }
}

/// One line from the host: always a reply, always carrying the `seq` it answers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HostLine {
    pub seq: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ok: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub err: Option<WireError>,
}

impl HostLine {
    pub fn ok(seq: u64, value: Value) -> HostLine {
        HostLine {
            seq,
            ok: Some(value),
            err: None,
        }
    }

    pub fn err(seq: u64, error: WireError) -> HostLine {
        HostLine {
            seq,
            ok: None,
            err: Some(error),
        }
    }
}

/// The `config` call. Always first, and only once.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Hello {
    pub protocol: u32,
}

/// The `config` reply: everything a run needs, so that nothing has to be
/// mounted or passed in an environment variable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunConfig {
    pub protocol: u32,
    /// The agent to run.
    pub agent: String,
    /// Every agent in the program, so a sub-agent call has something to resolve
    /// against. Sent rather than mounted: one fewer path inside the boundary,
    /// and the boundary then contains only what the policy named.
    pub agents: Vec<AgentIr>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub inputs: BTreeMap<String, Value>,
    pub max_steps: u32,
    /// Tool servers to start inside the boundary. `image` and `cwd` are
    /// meaningless there and are ignored by the guest.
    #[serde(default)]
    pub mcp: McpConfig,
    /// What the host's provider calls itself, so the `runStarted` event names
    /// the service that will actually answer rather than naming the channel.
    pub provider: String,
}

/// The `model` call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelCall {
    pub node: String,
    pub model: WireSelection,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    pub prompt: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub context: Vec<(String, Value)>,
    pub response_type: String,
    pub shape: WireShape,
    pub max_tokens: u32,
}

impl ModelCall {
    pub fn of(request: &CompletionRequest) -> ModelCall {
        ModelCall {
            node: request.node.clone(),
            model: WireSelection::of(&request.model),
            system: request.system.clone(),
            prompt: request.prompt.clone(),
            context: request.context.clone(),
            response_type: request.response_type.clone(),
            shape: WireShape::of(&request.shape),
            max_tokens: request.max_tokens,
        }
    }

    pub fn into_request(self) -> CompletionRequest {
        CompletionRequest {
            node: self.node,
            model: self.model.into_selection(),
            system: self.system,
            prompt: self.prompt,
            context: self.context,
            response_type: self.response_type,
            shape: self.shape.into_shape(),
            max_tokens: self.max_tokens,
        }
    }
}

/// The `model` reply.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelReply {
    pub value: Value,
    #[serde(default)]
    pub usage: Usage,
    /// The model that answered, for the event stream.
    pub model: String,
}

impl ModelReply {
    pub fn of(response: &CompletionResponse) -> ModelReply {
        ModelReply {
            value: response.value.clone(),
            usage: response.usage,
            model: response.model.clone(),
        }
    }

    pub fn into_response(self) -> CompletionResponse {
        CompletionResponse {
            value: self.value,
            usage: self.usage,
            model: self.model,
        }
    }
}

/// The `approval` call. The gate is asked inside and decided outside, which is
/// the whole reason it crosses: there is nobody in the box to ask.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApprovalCall {
    pub node: String,
    pub effects: Vec<String>,
    pub reason: String,
}

impl ApprovalCall {
    pub fn of(request: &ApprovalRequest) -> ApprovalCall {
        ApprovalCall {
            node: request.node.clone(),
            effects: request.effects.clone(),
            reason: request.reason.clone(),
        }
    }

    pub fn into_request(self) -> ApprovalRequest {
        ApprovalRequest {
            node: self.node,
            effects: self.effects,
            reason: self.reason,
        }
    }
}

/// The `approval` reply.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApprovalReply {
    pub allowed: bool,
}

/// The `consult` call. A question is written inside and answered outside, for
/// the same reason a gate is: there is nobody in the box to ask.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConsultCall {
    pub node: String,
    pub index: usize,
    pub question: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub choices: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub context: Vec<(String, Value)>,
}

impl ConsultCall {
    pub fn of(request: &ConsultRequest) -> ConsultCall {
        ConsultCall {
            node: request.node.clone(),
            index: request.index,
            question: request.question.clone(),
            choices: request.choices.clone(),
            context: request.context.clone(),
        }
    }

    pub fn into_request(self) -> ConsultRequest {
        ConsultRequest {
            node: self.node,
            index: self.index,
            question: self.question,
            choices: self.choices,
            context: self.context,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConsultReply {
    pub answer: String,
}

/// The `finished` notification: what the run produced.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Finished {
    pub agent: String,
    pub steps: u32,
    #[serde(default)]
    pub usage: Usage,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub outputs: BTreeMap<String, Artifact>,
}

/// The `failed` notification.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Failed {
    pub reason: String,
    /// Whether the failure is the operator's to fix rather than the artifact's.
    /// Carried across so the host can print the same hint it would have printed
    /// for an uncontained run.
    #[serde(default)]
    pub operator_error: bool,
}

/// [`ModelSelection`], on the wire.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum WireSelection {
    /// A pinned `vendor/model` reference. The vendor half decides which provider
    /// the *host* uses, so it has to cross intact.
    Exact {
        reference: String,
    },
    Capabilities {
        capabilities: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        min_context_tokens: Option<i64>,
    },
    Default,
}

impl WireSelection {
    pub fn of(selection: &ModelSelection) -> WireSelection {
        match selection {
            ModelSelection::Exact(reference) => WireSelection::Exact {
                reference: reference.clone(),
            },
            ModelSelection::Capabilities {
                capabilities,
                min_context_tokens,
            } => WireSelection::Capabilities {
                capabilities: capabilities.clone(),
                min_context_tokens: *min_context_tokens,
            },
            ModelSelection::Default => WireSelection::Default,
        }
    }

    pub fn into_selection(self) -> ModelSelection {
        match self {
            WireSelection::Exact { reference } => ModelSelection::Exact(reference),
            WireSelection::Capabilities {
                capabilities,
                min_context_tokens,
            } => ModelSelection::Capabilities {
                capabilities,
                min_context_tokens,
            },
            WireSelection::Default => ModelSelection::Default,
        }
    }
}

/// [`ResponseShape`], on the wire.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum WireShape {
    Prose,
    FreeJson,
    Schema { schema: Value, wrapped: bool },
}

impl WireShape {
    pub fn of(shape: &ResponseShape) -> WireShape {
        match shape {
            ResponseShape::Prose => WireShape::Prose,
            ResponseShape::FreeJson => WireShape::FreeJson,
            ResponseShape::Schema { schema, wrapped } => WireShape::Schema {
                schema: schema.clone(),
                wrapped: *wrapped,
            },
        }
    }

    pub fn into_shape(self) -> ResponseShape {
        match self {
            WireShape::Prose => ResponseShape::Prose,
            WireShape::FreeJson => ResponseShape::FreeJson,
            WireShape::Schema { schema, wrapped } => ResponseShape::Schema { schema, wrapped },
        }
    }
}

/// [`ProviderError`], on the wire.
///
/// Reproduced variant by variant rather than flattened to a string. A rate limit
/// inside the boundary must be the same condition as a rate limit outside it, or
/// the interpreter's behaviour depends on where it happens to be running — and
/// `Truncated { limit }` collapsed into prose loses the number an operator needs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum WireError {
    Transport {
        message: String,
    },
    Request {
        status: u16,
        message: String,
    },
    RateLimited {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        retry_after_seconds: Option<u64>,
    },
    Refused {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        category: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        explanation: Option<String>,
    },
    InvalidResponse {
        message: String,
    },
    Truncated {
        limit: u32,
    },
    Configuration {
        message: String,
    },
    Cassette {
        message: String,
    },
    /// The host could not serve the call at all: an unknown method, a bad
    /// payload, a version mismatch. Not a provider condition, so it is its own
    /// variant rather than being disguised as one.
    Protocol {
        message: String,
    },
}

impl WireError {
    pub fn of(error: &ProviderError) -> WireError {
        match error {
            ProviderError::Transport(message) => WireError::Transport {
                message: message.clone(),
            },
            ProviderError::Request { status, message } => WireError::Request {
                status: *status,
                message: message.clone(),
            },
            ProviderError::RateLimited {
                retry_after_seconds,
            } => WireError::RateLimited {
                retry_after_seconds: *retry_after_seconds,
            },
            ProviderError::Refused {
                category,
                explanation,
            } => WireError::Refused {
                category: category.clone(),
                explanation: explanation.clone(),
            },
            ProviderError::InvalidResponse(message) => WireError::InvalidResponse {
                message: message.clone(),
            },
            ProviderError::Truncated { limit } => WireError::Truncated { limit: *limit },
            ProviderError::Configuration(message) => WireError::Configuration {
                message: message.clone(),
            },
            ProviderError::Cassette(message) => WireError::Cassette {
                message: message.clone(),
            },
        }
    }

    /// The same condition, inside the boundary.
    ///
    /// `Protocol` has no `ProviderError` counterpart and becomes `Transport`:
    /// from the interpreter's point of view the channel to the model failed,
    /// which is exactly what happened.
    pub fn into_provider_error(self) -> ProviderError {
        match self {
            WireError::Transport { message } => ProviderError::Transport(message),
            WireError::Request { status, message } => ProviderError::Request { status, message },
            WireError::RateLimited {
                retry_after_seconds,
            } => ProviderError::RateLimited {
                retry_after_seconds,
            },
            WireError::Refused {
                category,
                explanation,
            } => ProviderError::Refused {
                category,
                explanation,
            },
            WireError::InvalidResponse { message } => ProviderError::InvalidResponse(message),
            WireError::Truncated { limit } => ProviderError::Truncated { limit },
            WireError::Configuration { message } => ProviderError::Configuration(message),
            WireError::Cassette { message } => ProviderError::Cassette(message),
            WireError::Protocol { message } => {
                ProviderError::Transport(format!("the supervisor refused the call: {message}"))
            }
        }
    }

    pub fn protocol(message: impl Into<String>) -> WireError {
        WireError::Protocol {
            message: message.into(),
        }
    }
}

/// The refusal for a version the other half does not implement.
pub fn version_mismatch(theirs: u32) -> WireError {
    WireError::protocol(format!(
        "the run asked for supervisor protocol {theirs} and this host implements \
         {PROTOCOL_VERSION}; the image was built from different source than the host, so rebuild \
         it rather than running two versions against each other"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn request() -> CompletionRequest {
        CompletionRequest {
            node: "n3".into(),
            model: ModelSelection::Exact("openai/gpt-test".into()),
            system: Some("Be brief".into()),
            prompt: "Summarise".into(),
            context: vec![("document".into(), json!("hello"))],
            response_type: "markdown".into(),
            shape: ResponseShape::Prose,
            max_tokens: 4096,
        }
    }

    #[test]
    fn a_model_call_round_trips_without_losing_the_vendor() {
        // The vendor half decides which provider the host uses. Dropping it is
        // the bug that made `model exact "openai/…"` reach Anthropic once
        // already, and a boundary is a new place for it to happen.
        let call = ModelCall::of(&request());
        let text = serde_json::to_string(&call).unwrap();
        assert!(text.contains("openai/gpt-test"), "{text}");

        let parsed: ModelCall = serde_json::from_str(&text).unwrap();
        let back = parsed.into_request();
        assert_eq!(back.model, ModelSelection::Exact("openai/gpt-test".into()));
        assert_eq!(back.digest(), request().digest());
    }

    #[test]
    fn every_response_shape_survives_the_crossing() {
        for shape in [
            ResponseShape::Prose,
            ResponseShape::FreeJson,
            ResponseShape::Schema {
                schema: json!({"type": "object"}),
                wrapped: true,
            },
        ] {
            let wire = WireShape::of(&shape);
            let text = serde_json::to_string(&wire).unwrap();
            let parsed: WireShape = serde_json::from_str(&text).unwrap();
            assert_eq!(parsed.into_shape(), shape, "{text}");
        }
    }

    #[test]
    fn every_selection_survives_the_crossing() {
        for selection in [
            ModelSelection::Default,
            ModelSelection::Exact("anthropic/claude-test".into()),
            ModelSelection::Capabilities {
                capabilities: vec!["structured_output".into()],
                min_context_tokens: Some(100_000),
            },
        ] {
            let wire = WireSelection::of(&selection);
            let text = serde_json::to_string(&wire).unwrap();
            let parsed: WireSelection = serde_json::from_str(&text).unwrap();
            assert_eq!(parsed.into_selection(), selection, "{text}");
        }
    }

    #[test]
    fn a_provider_error_keeps_its_kind_across_the_boundary() {
        // Flattening these to a string would make a rate limit indistinguishable
        // from a refusal, and the interpreter would behave differently depending
        // on whether it happened to be contained.
        let cases = [
            ProviderError::Transport("socket".into()),
            ProviderError::Request {
                status: 400,
                message: "bad".into(),
            },
            ProviderError::RateLimited {
                retry_after_seconds: Some(30),
            },
            ProviderError::RateLimited {
                retry_after_seconds: None,
            },
            ProviderError::Refused {
                category: Some("safety".into()),
                explanation: None,
            },
            ProviderError::InvalidResponse("not json".into()),
            ProviderError::Truncated { limit: 512 },
            ProviderError::Configuration("no key".into()),
            ProviderError::Cassette("no match".into()),
        ];
        for error in cases {
            let wire = WireError::of(&error);
            let text = serde_json::to_string(&wire).unwrap();
            let parsed: WireError = serde_json::from_str(&text).unwrap();
            assert_eq!(
                parsed.into_provider_error().to_string(),
                error.to_string(),
                "{text}"
            );
        }
    }

    #[test]
    fn a_protocol_fault_reaches_the_interpreter_as_a_transport_failure() {
        let error = WireError::protocol("no such method").into_provider_error();
        assert!(matches!(error, ProviderError::Transport(_)), "{error}");
        assert!(error.to_string().contains("no such method"), "{error}");
    }

    #[test]
    fn a_call_and_a_notification_are_told_apart_by_the_presence_of_a_reply() {
        let call = GuestLine::call(7, CALL_MODEL, json!({}));
        let notice = GuestLine::notify(NOTIFY_EVENT, json!({}));
        assert_eq!(call.seq, Some(7));
        assert!(notice.seq.is_none(), "a notification expects no reply");

        let text = serde_json::to_string(&notice).unwrap();
        assert!(!text.contains("seq"), "{text}");
        assert!(!text.contains("\"call\""), "{text}");
    }

    #[test]
    fn a_line_naming_an_unknown_method_still_parses() {
        // So the host can say which method it was. A strict enum here would
        // report "data did not match any variant" for a guest built from
        // different source, which is the least useful moment to be vague.
        let line: GuestLine =
            serde_json::from_str(r#"{"seq":1,"call":"frobnicate","params":{"a":1}}"#).unwrap();
        assert_eq!(line.call.as_deref(), Some("frobnicate"));
    }

    #[test]
    fn a_version_mismatch_names_both_numbers() {
        let text = version_mismatch(99).into_provider_error().to_string();
        assert!(text.contains("99"), "{text}");
        assert!(text.contains(&PROTOCOL_VERSION.to_string()), "{text}");
        assert!(text.contains("rebuild"), "{text}");
    }

    #[test]
    fn a_reply_is_either_a_value_or_an_error_and_says_which() {
        let ok = serde_json::to_string(&HostLine::ok(1, json!({"a": 1}))).unwrap();
        assert!(!ok.contains("err"), "{ok}");
        let err = serde_json::to_string(&HostLine::err(1, WireError::protocol("x"))).unwrap();
        assert!(!err.contains("\"ok\""), "{err}");
        assert!(err.contains("protocol"), "{err}");
    }
}
