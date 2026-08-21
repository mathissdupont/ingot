//! The inside half: a run that gets its model answers from somewhere else.
//!
//! A contained run has no network and no credential, so it cannot call a
//! provider. What it has is a pipe. [`GuestProvider`] implements
//! [`ModelProvider`] by asking down that pipe, which is the whole trick: the
//! interpreter is unchanged and does not know it is contained.
//!
//! Three of the interpreter's collaborators — the provider, the approval
//! handler and the event sink — are all borrowed mutably at once by
//! [`ingot_runtime::run`], and all three need the same channel. Hence
//! [`Guest`] hands out three cheap handles over one shared channel. It is
//! single-threaded by construction: the interpreter makes one call at a time and
//! blocks for the answer, so `Rc<RefCell<_>>` is the honest representation
//! rather than a compromise.

use std::cell::RefCell;
use std::fmt;
use std::io::{BufRead, BufReader, Write};
use std::rc::Rc;

use ingot_runtime::provider::{CompletionRequest, CompletionResponse, ProviderError};
use ingot_runtime::{
    ApprovalRequest, ConsultError, ConsultRequest, EventSink, Interlocutor, ModelProvider,
    RunError, RunEvent, RunReport,
};
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::protocol::{
    ApprovalCall, ApprovalReply, ConsultCall, ConsultReply, Failed, Finished, GuestLine, Hello,
    HostLine, ModelCall, ModelReply, RunConfig, WireError, CALL_APPROVAL, CALL_CONFIG,
    CALL_CONSULT, CALL_MODEL, NOTIFY_EVENT, NOTIFY_FAILED, NOTIFY_FINISHED, PROTOCOL_VERSION,
};

/// Why a guest could not talk to its supervisor.
#[derive(Debug)]
pub enum GuestError {
    /// The channel is gone: the supervisor exited, or the pipe broke.
    Channel(String),
    /// The two halves do not agree on the protocol, or the reply made no sense.
    Protocol(String),
}

impl fmt::Display for GuestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GuestError::Channel(message) => {
                write!(f, "the supervisor channel failed: {message}")
            }
            GuestError::Protocol(message) => write!(f, "the supervisor said: {message}"),
        }
    }
}

impl std::error::Error for GuestError {}

impl From<WireError> for GuestError {
    fn from(error: WireError) -> GuestError {
        match &error {
            WireError::Transport { message } => GuestError::Channel(message.clone()),
            _ => GuestError::Protocol(error.into_provider_error().to_string()),
        }
    }
}

/// The framing. One line out, one line back, in that order, forever.
struct Channel {
    reader: Box<dyn BufRead>,
    writer: Box<dyn Write>,
    seq: u64,
}

impl Channel {
    fn call<P: Serialize, R: DeserializeOwned>(
        &mut self,
        method: &str,
        params: &P,
    ) -> Result<R, WireError> {
        self.seq += 1;
        let seq = self.seq;

        let params = serde_json::to_value(params).map_err(|error| WireError::Transport {
            message: format!("could not encode a `{method}` call: {error}"),
        })?;
        self.send(&GuestLine::call(seq, method, params))?;

        let line = self.receive()?;
        let reply: HostLine =
            serde_json::from_str(&line).map_err(|error| WireError::Transport {
                message: format!("the supervisor sent something unreadable: {error}"),
            })?;

        // A reply carrying the wrong sequence number means the channel is one
        // message out of step. Continuing would answer this call with the
        // previous one's result, so it stops here.
        if reply.seq != seq {
            return Err(WireError::protocol(format!(
                "expected the answer to call {seq} and got the answer to {}",
                reply.seq
            )));
        }
        if let Some(error) = reply.err {
            return Err(error);
        }
        let value = reply.ok.ok_or_else(|| {
            WireError::protocol(format!(
                "the answer to call {seq} was neither a result nor an error"
            ))
        })?;
        serde_json::from_value(value).map_err(|error| {
            WireError::protocol(format!(
                "the answer to `{method}` had the wrong shape: {error}"
            ))
        })
    }

    fn notify<P: Serialize>(&mut self, method: &str, params: &P) -> Result<(), WireError> {
        let params = serde_json::to_value(params).map_err(|error| WireError::Transport {
            message: format!("could not encode a `{method}` notification: {error}"),
        })?;
        self.send(&GuestLine::notify(method, params))
    }

    fn send(&mut self, line: &GuestLine) -> Result<(), WireError> {
        let mut text = serde_json::to_string(line).map_err(|error| WireError::Transport {
            message: error.to_string(),
        })?;
        text.push('\n');
        self.writer
            .write_all(text.as_bytes())
            .and_then(|()| self.writer.flush())
            .map_err(|error| WireError::Transport {
                message: format!("writing to the supervisor: {error}"),
            })
    }

    fn receive(&mut self) -> Result<String, WireError> {
        let mut line = String::new();
        loop {
            line.clear();
            match self.reader.read_line(&mut line) {
                // End of file. The supervisor is gone, and there is nothing this
                // side can do about it except say so clearly.
                Ok(0) => {
                    return Err(WireError::Transport {
                        message: "the supervisor closed the channel".to_string(),
                    })
                }
                Ok(_) if line.trim().is_empty() => continue,
                Ok(_) => return Ok(line.trim().to_string()),
                Err(error) => {
                    return Err(WireError::Transport {
                        message: format!("reading from the supervisor: {error}"),
                    })
                }
            }
        }
    }
}

/// The guest's end of the channel.
#[derive(Clone)]
pub struct Guest {
    inner: Rc<RefCell<Channel>>,
}

impl fmt::Debug for Guest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Guest(supervised)")
    }
}

impl Guest {
    /// The normal case: the supervisor is on the other side of this process's
    /// standard streams.
    ///
    /// Nothing else may write to stdout afterwards. A stray `println!` would be
    /// read as a protocol line and desynchronise the channel, so diagnostics go
    /// to stderr — which the supervisor relays.
    pub fn on_stdio() -> Guest {
        Guest::new(
            Box::new(BufReader::new(std::io::stdin())),
            Box::new(std::io::stdout()),
        )
    }

    pub fn new(reader: Box<dyn BufRead>, writer: Box<dyn Write>) -> Guest {
        Guest {
            inner: Rc::new(RefCell::new(Channel {
                reader,
                writer,
                seq: 0,
            })),
        }
    }

    /// Ask for everything this run needs. Always first, and only once.
    pub fn config(&self) -> Result<RunConfig, GuestError> {
        let config: RunConfig = self.inner.borrow_mut().call(
            CALL_CONFIG,
            &Hello {
                protocol: PROTOCOL_VERSION,
            },
        )?;

        // The host checks this too. Checking it on both sides costs one
        // comparison and means neither half has to trust the other to have done
        // it — which is the point of a boundary.
        if config.protocol != PROTOCOL_VERSION {
            return Err(GuestError::Protocol(format!(
                "the supervisor answered with protocol {} and this run implements {PROTOCOL_VERSION}",
                config.protocol
            )));
        }
        Ok(config)
    }

    /// A provider that answers by asking the host.
    ///
    /// `name` is the host provider's own name, taken from the config, so the
    /// `runStarted` event names the service that will actually answer rather
    /// than naming this channel.
    pub fn provider(&self, name: impl Into<String>) -> GuestProvider {
        GuestProvider {
            inner: Rc::clone(&self.inner),
            name: name.into(),
        }
    }

    /// An approval handler that asks the operator, who is outside.
    pub fn approvals(&self) -> GuestApprovals {
        GuestApprovals {
            inner: Rc::clone(&self.inner),
        }
    }

    /// An event sink that streams to the host's terminal.
    pub fn events(&self) -> GuestEvents {
        GuestEvents {
            inner: Rc::clone(&self.inner),
        }
    }

    /// Report a completed run and everything it produced.
    pub fn finished(&self, report: &RunReport) -> Result<(), GuestError> {
        self.inner
            .borrow_mut()
            .notify(
                NOTIFY_FINISHED,
                &Finished {
                    agent: report.agent.clone(),
                    steps: report.steps,
                    usage: report.usage,
                    outputs: report.outputs.clone(),
                    // The charging happened here, so the ledger is here. The
                    // host has no prices of its own to redo the arithmetic
                    // with, and a cost it was never told is a cost it cannot
                    // report.
                    spend: report.spend.clone(),
                },
            )
            .map_err(GuestError::from)
    }

    /// Report a failed run.
    ///
    /// The reason is rendered here rather than on the host, because [`RunError`]
    /// knows how to describe itself and the host would only be reconstructing a
    /// worse version of the same sentence.
    pub fn failed(&self, error: &RunError) -> Result<(), GuestError> {
        self.fail_with(&error.to_string(), error.is_operator_error())
    }

    /// Report a failure that is not a [`RunError`] — a tool host that would not
    /// start, an unusable configuration.
    pub fn fail_with(&self, reason: &str, operator_error: bool) -> Result<(), GuestError> {
        self.inner
            .borrow_mut()
            .notify(
                NOTIFY_FAILED,
                &Failed {
                    reason: reason.to_string(),
                    operator_error,
                },
            )
            .map_err(GuestError::from)
    }
}

/// Completions, fetched from outside the boundary.
///
/// This one does not stream, and inherits the default `streams() == false`
/// deliberately. The provider with the credential is on the other side of the
/// channel, so a fragment would have to cross it as a notification the protocol
/// does not have. Until it does, a contained run keeps the smaller output
/// ceiling and shows no live text — which is a real limitation, recorded as
/// such rather than papered over by claiming a capability this side lacks.
pub struct GuestProvider {
    inner: Rc<RefCell<Channel>>,
    name: String,
}

impl fmt::Debug for GuestProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "GuestProvider({})", self.name)
    }
}

impl ModelProvider for GuestProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn complete(
        &mut self,
        request: &CompletionRequest,
    ) -> Result<CompletionResponse, ProviderError> {
        let reply: ModelReply = self
            .inner
            .borrow_mut()
            .call(CALL_MODEL, &ModelCall::of(request))
            .map_err(WireError::into_provider_error)?;
        Ok(reply.into_response())
    }
}

/// Gates, decided outside the boundary.
pub struct GuestApprovals {
    inner: Rc<RefCell<Channel>>,
}

impl fmt::Debug for GuestApprovals {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("GuestApprovals")
    }
}

impl Interlocutor for GuestApprovals {
    fn approve(&mut self, request: &ApprovalRequest) -> bool {
        match self
            .inner
            .borrow_mut()
            .call::<_, ApprovalReply>(CALL_APPROVAL, &ApprovalCall::of(request))
        {
            Ok(reply) => reply.allowed,
            // A gate that cannot reach anybody is refused. Defaulting the other
            // way would turn a broken pipe into consent, which is the one
            // failure mode an approval gate exists to prevent.
            Err(_) => false,
        }
    }

    fn consult(&mut self, request: &ConsultRequest) -> Result<String, ConsultError> {
        // No safe default here, unlike a gate: there is no answer to fall back
        // to, so a broken channel is reported rather than papered over.
        self.inner
            .borrow_mut()
            .call::<_, ConsultReply>(CALL_CONSULT, &ConsultCall::of(request))
            .map(|reply| reply.answer)
            .map_err(|error| ConsultError::Failed(GuestError::from(error).to_string()))
    }
}

/// Progress, streamed outside the boundary.
pub struct GuestEvents {
    inner: Rc<RefCell<Channel>>,
}

impl fmt::Debug for GuestEvents {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("GuestEvents")
    }
}

impl EventSink for GuestEvents {
    fn emit(&mut self, event: RunEvent) {
        // A failed notification is deliberately not escalated. [`EventSink`]
        // cannot fail, and it should not: losing a progress line must never turn
        // a working run into a failed one. If the channel is really gone, the
        // next call fails and says so.
        let _ = self.inner.borrow_mut().notify(NOTIFY_EVENT, &event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{HostLine, WireError};
    use ingot_runtime::provider::{ModelSelection, Usage};
    use ingot_runtime::schema::ResponseShape;
    use serde_json::json;

    /// A supervisor made of a fixed list of replies.
    fn guest_reading(replies: &[HostLine]) -> (Guest, Rc<RefCell<Vec<u8>>>) {
        let mut script = String::new();
        for reply in replies {
            script.push_str(&serde_json::to_string(reply).unwrap());
            script.push('\n');
        }
        let sent = Rc::new(RefCell::new(Vec::new()));
        let guest = Guest::new(
            Box::new(std::io::Cursor::new(script.into_bytes())),
            Box::new(Sink(Rc::clone(&sent))),
        );
        (guest, sent)
    }

    /// Captures what the guest wrote, so a test can assert the wire.
    struct Sink(Rc<RefCell<Vec<u8>>>);

    impl Write for Sink {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.borrow_mut().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn request() -> CompletionRequest {
        CompletionRequest {
            node: "n0".into(),
            model: ModelSelection::Default,
            system: None,
            prompt: "hello".into(),
            context: Vec::new(),
            response_type: "markdown".into(),
            shape: ResponseShape::Prose,
            max_tokens: 100,
        }
    }

    #[test]
    fn a_completion_comes_back_through_the_channel() {
        let reply = ModelReply {
            value: json!("# Brief"),
            usage: Usage {
                input_tokens: 10,
                output_tokens: 3,
                cache_read_tokens: 0,
            },
            model: "test-model".into(),
        };
        let (guest, sent) =
            guest_reading(&[HostLine::ok(1, serde_json::to_value(&reply).unwrap())]);

        let mut provider = guest.provider("anthropic");
        assert_eq!(provider.name(), "anthropic", "the event names the service");
        let response = provider.complete(&request()).unwrap();
        assert_eq!(response.value, json!("# Brief"));
        assert_eq!(response.usage.output_tokens, 3);

        let wire = String::from_utf8(sent.borrow().clone()).unwrap();
        assert!(wire.contains("\"call\":\"model\""), "{wire}");
        assert!(wire.ends_with('\n'), "lines must be newline-delimited");
    }

    #[test]
    fn a_provider_error_arrives_as_the_same_condition() {
        let (guest, _) = guest_reading(&[HostLine::err(
            1,
            WireError::RateLimited {
                retry_after_seconds: Some(7),
            },
        )]);
        let error = guest.provider("x").complete(&request()).unwrap_err();
        assert!(
            matches!(
                error,
                ProviderError::RateLimited {
                    retry_after_seconds: Some(7)
                }
            ),
            "{error}"
        );
    }

    #[test]
    fn a_closed_channel_is_a_transport_failure_and_not_a_hang() {
        let (guest, _) = guest_reading(&[]);
        let error = guest.provider("x").complete(&request()).unwrap_err();
        assert!(error.to_string().contains("closed the channel"), "{error}");
    }

    #[test]
    fn an_approval_that_cannot_reach_anybody_is_refused() {
        // The channel is empty, so the call fails. Denying is the only safe
        // answer: a broken pipe must not read as consent.
        let (guest, _) = guest_reading(&[]);
        let allowed = guest.approvals().approve(&ApprovalRequest {
            node: "n7".into(),
            effects: vec!["external_write".into()],
            reason: "post the review".into(),
        });
        assert!(!allowed);
    }

    #[test]
    fn an_approval_is_decided_outside() {
        let (guest, sent) = guest_reading(&[HostLine::ok(1, json!({"allowed": true}))]);
        let allowed = guest.approvals().approve(&ApprovalRequest {
            node: "n7".into(),
            effects: vec!["external_write".into()],
            reason: "post the review".into(),
        });
        assert!(allowed);
        let wire = String::from_utf8(sent.borrow().clone()).unwrap();
        assert!(wire.contains("post the review"), "{wire}");
    }

    #[test]
    fn a_reply_to_the_wrong_call_stops_the_run_rather_than_being_used() {
        // One message out of step would answer this call with the previous
        // one's result, which is worse than failing.
        let (guest, _) = guest_reading(&[HostLine::ok(99, json!({}))]);
        let error = guest.provider("x").complete(&request()).unwrap_err();
        assert!(error.to_string().contains("expected the answer"), "{error}");
    }

    #[test]
    fn a_version_mismatch_in_the_config_reply_is_refused() {
        let (guest, _) = guest_reading(&[HostLine::ok(
            1,
            json!({
                "protocol": PROTOCOL_VERSION + 1,
                "agent": "a.B",
                "agents": [],
                "maxSteps": 10,
                "provider": "test"
            }),
        )]);
        let error = guest.config().unwrap_err();
        assert!(matches!(error, GuestError::Protocol(_)), "{error}");
    }

    #[test]
    fn an_event_never_fails_the_run() {
        // Nowhere to write, and `emit` still returns.
        let (guest, _) = guest_reading(&[]);
        let mut events = guest.events();
        events.emit(RunEvent::RunFinished {
            steps: 1,
            usage: Usage::default(),
        });
    }

    #[test]
    fn a_blank_line_is_skipped_rather_than_read_as_a_message() {
        let sent = Rc::new(RefCell::new(Vec::new()));
        let script = format!(
            "\n\n{}\n",
            serde_json::to_string(&HostLine::ok(1, json!({"allowed": false}))).unwrap()
        );
        let guest = Guest::new(
            Box::new(std::io::Cursor::new(script.into_bytes())),
            Box::new(Sink(sent)),
        );
        let allowed = guest.approvals().approve(&ApprovalRequest {
            node: "n0".into(),
            effects: Vec::new(),
            reason: String::new(),
        });
        assert!(!allowed, "the reply said no, and it was found");
    }
}
