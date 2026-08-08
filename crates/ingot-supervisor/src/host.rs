//! The outside half: the process that holds the credential and the terminal.
//!
//! The host owns everything the boundary excludes — the API key, the operator's
//! keyboard, the output directory — and lends the run exactly three of them
//! through the channel: completions, approval decisions, and somewhere to put
//! progress. It never initiates. [`serve`] is a loop that answers.
//!
//! That asymmetry is the design. A run inside a boundary cannot be trusted to
//! ask for the right things, so it is not given the ability to ask for anything
//! not on this list.

use std::fmt;
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

use ingot_runtime::{ApprovalMode, ApprovalRequest, ModelProvider, RunEvent};

use crate::protocol::{
    version_mismatch, ApprovalCall, ApprovalReply, Failed, Finished, GuestLine, Hello, HostLine,
    ModelCall, ModelReply, RunConfig, WireError, CALL_APPROVAL, CALL_CONFIG, CALL_MODEL,
    NOTIFY_EVENT, NOTIFY_FAILED, NOTIFY_FINISHED, PROTOCOL_VERSION,
};

/// How a supervised run ended, as the run itself reported it.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Finished(Finished),
    Failed(Failed),
}

/// Why supervision could not be carried out.
///
/// Distinct from [`Outcome::Failed`], which is a run that ran and did not work.
/// These are failures of the arrangement: an image with no `ingot` in it, a
/// desynchronised channel, a guest that died without a word.
#[derive(Debug)]
pub enum HostError {
    /// The guest sent something outside the protocol.
    Protocol(String),
    /// The guest ended without reporting an outcome.
    NoOutcome(String),
    /// The guest could not be started, or its streams failed.
    Io(String),
}

impl fmt::Display for HostError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HostError::Protocol(message) => {
                write!(
                    f,
                    "the contained run broke the supervisor protocol: {message}"
                )
            }
            HostError::NoOutcome(detail) => write!(
                f,
                "the contained run ended without saying how it went\n  {detail}"
            ),
            HostError::Io(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for HostError {}

/// What the host lends to a contained run.
pub struct Supervisor<'a> {
    /// The answer to the guest's `config` call.
    pub config: RunConfig,
    /// The real provider. It stays here, with its credential.
    pub provider: &'a mut dyn ModelProvider,
    /// How gates are decided. `Ask` reaches the operator's terminal, which the
    /// guest has no access to.
    pub approval: &'a mut ApprovalMode,
}

impl fmt::Debug for Supervisor<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Supervisor")
            .field("agent", &self.config.agent)
            .field("provider", &self.provider.name())
            .field("approval", &self.approval)
            .finish()
    }
}

/// Answer a guest's calls until it reports an outcome.
///
/// `reader` is the guest's standard output and `writer` its standard input.
/// Taking them separately, rather than a process, is what makes the protocol
/// testable in memory — the interesting half of this feature needs no container
/// runtime to exercise.
pub fn serve(
    reader: impl BufRead,
    mut writer: impl Write,
    supervisor: &mut Supervisor<'_>,
    on_event: &mut dyn FnMut(&RunEvent),
) -> Result<Outcome, HostError> {
    let mut configured = false;

    for line in reader.lines() {
        let line =
            line.map_err(|error| HostError::Io(format!("reading the run's output: {error}")))?;
        if line.trim().is_empty() {
            continue;
        }

        let message: GuestLine = serde_json::from_str(&line).map_err(|error| {
            HostError::Protocol(format!("{error}\n  the line was: {}", truncate(&line, 200)))
        })?;

        if let Some(method) = message.notify.as_deref() {
            match method {
                NOTIFY_EVENT => {
                    let event: RunEvent = decode(&message.params, NOTIFY_EVENT)?;
                    on_event(&event);
                }
                NOTIFY_FINISHED => {
                    return Ok(Outcome::Finished(decode(&message.params, NOTIFY_FINISHED)?))
                }
                NOTIFY_FAILED => {
                    return Ok(Outcome::Failed(decode(&message.params, NOTIFY_FAILED)?))
                }
                other => {
                    return Err(HostError::Protocol(format!(
                        "the run sent a `{other}` notification, which this host does not implement"
                    )))
                }
            }
            continue;
        }

        let (Some(method), Some(seq)) = (message.call.as_deref(), message.seq) else {
            return Err(HostError::Protocol(format!(
                "a line was neither a call nor a notification: {}",
                truncate(&line, 200)
            )));
        };

        let reply = answer(method, seq, &message.params, supervisor, &mut configured);
        let text = serde_json::to_string(&reply)
            .map_err(|error| HostError::Io(format!("encoding a reply: {error}")))?;
        writeln!(writer, "{text}")
            .and_then(|()| writer.flush())
            .map_err(|error| {
                HostError::Io(format!("writing to the run's standard input: {error}"))
            })?;
    }

    Err(HostError::NoOutcome(
        "its output ended with no `finished` and no `failed`".to_string(),
    ))
}

/// One reply. Everything that can go wrong here is reported to the guest rather
/// than ending supervision, so a run that asks for something impossible fails
/// with its own diagnostics rather than being killed from outside.
fn answer(
    method: &str,
    seq: u64,
    params: &serde_json::Value,
    supervisor: &mut Supervisor<'_>,
    configured: &mut bool,
) -> HostLine {
    if method == CALL_CONFIG {
        if *configured {
            return HostLine::err(
                seq,
                WireError::protocol("`config` was asked for twice; it happens once per run"),
            );
        }
        let hello: Hello = match serde_json::from_value(params.clone()) {
            Ok(hello) => hello,
            Err(error) => {
                return HostLine::err(
                    seq,
                    WireError::protocol(format!("the `config` call was malformed: {error}")),
                )
            }
        };
        if hello.protocol != PROTOCOL_VERSION {
            return HostLine::err(seq, version_mismatch(hello.protocol));
        }
        *configured = true;
        return match serde_json::to_value(&supervisor.config) {
            Ok(value) => HostLine::ok(seq, value),
            Err(error) => HostLine::err(
                seq,
                WireError::protocol(format!("the run configuration could not be sent: {error}")),
            ),
        };
    }

    // Nothing is served before the run has identified itself. A guest that skips
    // `config` is not a guest this host built the boundary for.
    if !*configured {
        return HostLine::err(
            seq,
            WireError::protocol(format!(
                "`{method}` came before `config`, so this host does not know what run it is serving"
            )),
        );
    }

    match method {
        CALL_MODEL => {
            let call: ModelCall = match serde_json::from_value(params.clone()) {
                Ok(call) => call,
                Err(error) => {
                    return HostLine::err(
                        seq,
                        WireError::protocol(format!("the `model` call was malformed: {error}")),
                    )
                }
            };
            match supervisor.provider.complete(&call.into_request()) {
                Ok(response) => match serde_json::to_value(ModelReply::of(&response)) {
                    Ok(value) => HostLine::ok(seq, value),
                    Err(error) => HostLine::err(
                        seq,
                        WireError::protocol(format!("the answer could not be sent: {error}")),
                    ),
                },
                Err(error) => HostLine::err(seq, WireError::of(&error)),
            }
        }
        CALL_APPROVAL => {
            let call: ApprovalCall = match serde_json::from_value(params.clone()) {
                Ok(call) => call,
                Err(error) => {
                    return HostLine::err(
                        seq,
                        WireError::protocol(format!("the `approval` call was malformed: {error}")),
                    )
                }
            };
            let allowed = decide(supervisor.approval, &call.into_request());
            HostLine::ok(seq, serde_json::json!(ApprovalReply { allowed }))
        }
        other => HostLine::err(
            seq,
            WireError::protocol(format!(
                "the run called `{other}`, which this host does not implement"
            )),
        ),
    }
}

/// Apply the operator's approval mode.
///
/// Borrowed rather than taken, so a gate reached during one call does not disarm
/// the gates after it — the same mistake the interpreter made once already.
fn decide(approval: &mut ApprovalMode, request: &ApprovalRequest) -> bool {
    match approval {
        ApprovalMode::AssumeYes => true,
        ApprovalMode::Deny => false,
        ApprovalMode::Ask(handler) => handler.approve(request),
    }
}

fn decode<T: serde::de::DeserializeOwned>(
    params: &serde_json::Value,
    method: &str,
) -> Result<T, HostError> {
    serde_json::from_value(params.clone()).map_err(|error| {
        HostError::Protocol(format!("the `{method}` payload was malformed: {error}"))
    })
}

fn truncate(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    let head: String = text.chars().take(limit).collect();
    format!("{head}…")
}

/// Start a guest and supervise it to completion.
///
/// `command` is the caller's, because the two things this runs are unrelated:
/// `docker run … <image> ingot exec` for a contained run, and plain
/// `ingot exec` for `--supervised`. What supervision does is identical either
/// way, which is the reason the boundary is not this function's business.
pub fn supervise(
    command: &mut Command,
    supervisor: &mut Supervisor<'_>,
    on_event: &mut dyn FnMut(&RunEvent),
) -> Result<Outcome, HostError> {
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = command.spawn().map_err(|error| {
        HostError::Io(format!(
            "could not start the run: {error}\n  the command was: {}",
            describe(command)
        ))
    })?;

    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| HostError::Io("the run has no standard input".to_string()))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| HostError::Io("the run has no standard output".to_string()))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| HostError::Io("the run has no standard error".to_string()))?;

    // Relayed live and also kept. Live, because a run that is slow for a reason
    // should say so while it is happening; kept, because "the image has no
    // `ingot` in it" arrives on this stream and is the explanation for the
    // otherwise mysterious silence that follows.
    let relay = std::thread::spawn(move || {
        let mut kept: Vec<String> = Vec::new();
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            eprintln!("  [contained] {line}");
            kept.push(line);
        }
        kept
    });

    let outcome = serve(BufReader::new(stdout), stdin, supervisor, on_event);

    let status = child.wait();
    let diagnostics = relay.join().unwrap_or_default();

    match outcome {
        // A run that reported an outcome has said everything that matters, even
        // if the process then exited oddly.
        Ok(outcome) => Ok(outcome),
        Err(HostError::NoOutcome(_)) => Err(HostError::NoOutcome(explain(&status, &diagnostics))),
        Err(other) => Err(other),
    }
}

fn explain(status: &std::io::Result<std::process::ExitStatus>, diagnostics: &[String]) -> String {
    let mut detail = match status {
        Ok(status) if status.success() => {
            "it exited successfully, which it should not have without \
                                          reporting an outcome"
                .to_string()
        }
        Ok(status) => match status.code() {
            Some(code) => format!("it exited with status {code}"),
            None => "it was terminated by a signal".to_string(),
        },
        Err(error) => format!("its exit status could not be read: {error}"),
    };
    // The last few lines are the useful ones: a runtime's own complaint about a
    // missing binary or an unreadable image lands at the end.
    let tail: Vec<&String> = diagnostics.iter().rev().take(5).rev().collect();
    for line in tail {
        detail.push_str("\n  ");
        detail.push_str(line);
    }
    detail
}

fn describe(command: &Command) -> String {
    let mut text = command.get_program().to_string_lossy().to_string();
    for arg in command.get_args() {
        text.push(' ');
        text.push_str(&arg.to_string_lossy());
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use ingot_runtime::provider::{CompletionRequest, CompletionResponse, ProviderError, Usage};
    use ingot_runtime::{Artifact, ScriptedApprovals};
    use serde_json::json;

    struct Fixed(&'static str);

    impl ModelProvider for Fixed {
        fn name(&self) -> &str {
            "fixed"
        }
        fn complete(
            &mut self,
            _request: &CompletionRequest,
        ) -> Result<CompletionResponse, ProviderError> {
            Ok(CompletionResponse {
                value: json!(self.0),
                usage: Usage {
                    input_tokens: 1,
                    output_tokens: 2,
                    cache_read_tokens: 0,
                },
                model: "fixed-1".to_string(),
            })
        }
    }

    struct Failing;

    impl ModelProvider for Failing {
        fn name(&self) -> &str {
            "failing"
        }
        fn complete(
            &mut self,
            _request: &CompletionRequest,
        ) -> Result<CompletionResponse, ProviderError> {
            Err(ProviderError::Truncated { limit: 512 })
        }
    }

    fn config() -> RunConfig {
        RunConfig {
            protocol: PROTOCOL_VERSION,
            agent: "test.Agent".to_string(),
            agents: Vec::new(),
            inputs: Default::default(),
            max_steps: 100,
            mcp: Default::default(),
            provider: "fixed".to_string(),
        }
    }

    /// Drive `serve` over a scripted guest and return what the host wrote.
    fn exchange(
        guest_lines: &[String],
        provider: &mut dyn ModelProvider,
        approval: &mut ApprovalMode,
    ) -> (Result<Outcome, HostError>, String, Vec<RunEvent>) {
        let script = guest_lines.join("\n");
        let mut written: Vec<u8> = Vec::new();
        let mut events = Vec::new();
        let mut supervisor = Supervisor {
            config: config(),
            provider,
            approval,
        };
        let outcome = serve(
            std::io::Cursor::new(script.into_bytes()),
            &mut written,
            &mut supervisor,
            &mut |event| events.push(event.clone()),
        );
        (outcome, String::from_utf8(written).unwrap(), events)
    }

    fn hello(seq: u64) -> String {
        serde_json::to_string(&GuestLine::call(
            seq,
            CALL_CONFIG,
            json!({"protocol": PROTOCOL_VERSION}),
        ))
        .unwrap()
    }

    fn model_call(seq: u64) -> String {
        serde_json::to_string(&GuestLine::call(
            seq,
            CALL_MODEL,
            json!({
                "node": "n0",
                "model": {"kind": "default"},
                "prompt": "hi",
                "responseType": "markdown",
                "shape": {"kind": "prose"},
                "maxTokens": 10
            }),
        ))
        .unwrap()
    }

    fn finished() -> String {
        serde_json::to_string(&GuestLine::notify(
            NOTIFY_FINISHED,
            json!({
                "agent": "test.Agent",
                "steps": 1,
                "usage": {"inputTokens": 1, "outputTokens": 2},
                "outputs": {"brief": {"name": "brief", "contentType": "markdown", "value": "ok"}}
            }),
        ))
        .unwrap()
    }

    #[test]
    fn a_run_is_configured_then_served_then_reports_what_it_produced() {
        let (outcome, written, _) = exchange(
            &[hello(1), model_call(2), finished()],
            &mut Fixed("# Brief"),
            &mut ApprovalMode::Deny,
        );

        let Ok(Outcome::Finished(report)) = outcome else {
            panic!("expected a finished run, got {outcome:?}");
        };
        assert_eq!(report.steps, 1);
        assert_eq!(
            report.outputs["brief"],
            Artifact {
                name: "brief".into(),
                content_type: "markdown".into(),
                value: json!("ok"),
            }
        );

        // Two replies, in order, each naming the call it answers.
        let replies: Vec<HostLine> = written
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(replies.len(), 2, "{written}");
        assert_eq!(replies[0].seq, 1);
        assert_eq!(replies[1].seq, 2);
        assert!(written.contains("# Brief"), "{written}");
    }

    #[test]
    fn the_credential_holding_provider_stays_on_this_side() {
        // The guest asked for a completion and never learned anything about the
        // provider except the answer: no key, no base URL, no vendor list.
        let (_, written, _) = exchange(
            &[hello(1), model_call(2), finished()],
            &mut Fixed("answer"),
            &mut ApprovalMode::Deny,
        );
        for leak in ["api", "key", "http", "Authorization"] {
            assert!(
                !written
                    .to_ascii_lowercase()
                    .contains(&leak.to_ascii_lowercase()),
                "`{leak}` reached the guest:\n{written}"
            );
        }
    }

    #[test]
    fn a_provider_failure_is_reported_as_the_same_condition() {
        let (_, written, _) = exchange(
            &[hello(1), model_call(2), finished()],
            &mut Failing,
            &mut ApprovalMode::Deny,
        );
        assert!(written.contains("truncated"), "{written}");
        assert!(written.contains("512"), "{written}");
    }

    #[test]
    fn events_reach_the_hosts_terminal() {
        let event = serde_json::to_string(&GuestLine::notify(
            NOTIFY_EVENT,
            json!({"event": "nodeStarted", "node": "n0", "kind": "ask"}),
        ))
        .unwrap();
        let (_, written, events) = exchange(
            &[hello(1), event, finished()],
            &mut Fixed("x"),
            &mut ApprovalMode::Deny,
        );
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], RunEvent::NodeStarted { .. }));
        // A notification is not answered.
        assert_eq!(written.lines().count(), 1, "{written}");
    }

    #[test]
    fn an_approval_is_decided_here_and_not_in_there() {
        let gate = serde_json::to_string(&GuestLine::call(
            2,
            CALL_APPROVAL,
            json!({"node": "n7", "effects": ["external_write"], "reason": "post it"}),
        ))
        .unwrap();

        for (mut mode, expected) in [
            (ApprovalMode::AssumeYes, true),
            (ApprovalMode::Deny, false),
            (
                ApprovalMode::Ask(Box::new(ScriptedApprovals::new(vec![true]))),
                true,
            ),
        ] {
            let (_, written, _) = exchange(
                &[hello(1), gate.clone(), finished()],
                &mut Fixed("x"),
                &mut mode,
            );
            assert!(
                written.contains(&format!("\"allowed\":{expected}")),
                "{mode:?} should answer {expected}:\n{written}"
            );
        }
    }

    #[test]
    fn a_gate_does_not_disarm_the_gates_after_it() {
        // The interpreter had this bug once: consuming the approval mode to
        // serve one call left later gates unguarded. The supervisor borrows it.
        let gate = |seq: u64| {
            serde_json::to_string(&GuestLine::call(
                seq,
                CALL_APPROVAL,
                json!({"node": "n7", "effects": [], "reason": "x"}),
            ))
            .unwrap()
        };
        let mut mode = ApprovalMode::Ask(Box::new(ScriptedApprovals::new(vec![true, true])));
        let (_, written, _) = exchange(
            &[hello(1), gate(2), gate(3), finished()],
            &mut Fixed("x"),
            &mut mode,
        );
        assert_eq!(
            written.matches("\"allowed\":true").count(),
            2,
            "the second gate must still reach the handler:\n{written}"
        );
    }

    #[test]
    fn a_call_before_config_is_refused() {
        let (_, written, _) = exchange(
            &[model_call(1), finished()],
            &mut Fixed("x"),
            &mut ApprovalMode::Deny,
        );
        assert!(written.contains("before `config`"), "{written}");
    }

    #[test]
    fn asking_for_the_configuration_twice_is_refused() {
        let (_, written, _) = exchange(
            &[hello(1), hello(2), finished()],
            &mut Fixed("x"),
            &mut ApprovalMode::Deny,
        );
        assert!(written.contains("twice"), "{written}");
    }

    #[test]
    fn a_protocol_mismatch_is_refused_with_both_numbers() {
        let wrong = serde_json::to_string(&GuestLine::call(
            1,
            CALL_CONFIG,
            json!({"protocol": PROTOCOL_VERSION + 5}),
        ))
        .unwrap();
        let (_, written, _) = exchange(
            &[wrong, finished()],
            &mut Fixed("x"),
            &mut ApprovalMode::Deny,
        );
        assert!(
            written.contains(&(PROTOCOL_VERSION + 5).to_string()),
            "{written}"
        );
        assert!(written.contains("rebuild"), "{written}");
    }

    #[test]
    fn an_unknown_call_is_refused_by_name_rather_than_ending_supervision() {
        let odd = serde_json::to_string(&GuestLine::call(2, "frobnicate", json!({}))).unwrap();
        let (outcome, written, _) = exchange(
            &[hello(1), odd, finished()],
            &mut Fixed("x"),
            &mut ApprovalMode::Deny,
        );
        assert!(written.contains("frobnicate"), "{written}");
        assert!(
            matches!(outcome, Ok(Outcome::Finished(_))),
            "the run keeps going and fails on its own terms: {outcome:?}"
        );
    }

    #[test]
    fn a_run_that_ends_without_an_outcome_is_a_failure_not_an_empty_success() {
        let (outcome, _, _) = exchange(&[hello(1)], &mut Fixed("x"), &mut ApprovalMode::Deny);
        let Err(HostError::NoOutcome(_)) = outcome else {
            panic!("expected NoOutcome, got {outcome:?}");
        };
    }

    #[test]
    fn a_failed_run_carries_its_reason_and_whose_fault_it_is() {
        let failed = serde_json::to_string(&GuestLine::notify(
            NOTIFY_FAILED,
            json!({"reason": "missing input `topic`", "operatorError": true}),
        ))
        .unwrap();
        let (outcome, _, _) = exchange(
            &[hello(1), failed],
            &mut Fixed("x"),
            &mut ApprovalMode::Deny,
        );
        let Ok(Outcome::Failed(failure)) = outcome else {
            panic!("expected a failed run, got {outcome:?}");
        };
        assert!(failure.operator_error);
        assert!(failure.reason.contains("topic"));
    }

    #[test]
    fn an_unreadable_line_names_what_it_saw() {
        let (outcome, _, _) = exchange(
            &["{not json".to_string()],
            &mut Fixed("x"),
            &mut ApprovalMode::Deny,
        );
        let Err(HostError::Protocol(message)) = outcome else {
            panic!("expected a protocol error, got {outcome:?}");
        };
        assert!(message.contains("{not json"), "{message}");
    }

    #[test]
    fn a_command_that_cannot_start_says_what_it_tried_to_run() {
        let mut command = Command::new("ingot-no-such-program-exists");
        let mut approval = ApprovalMode::Deny;
        let mut provider = Fixed("x");
        let mut supervisor = Supervisor {
            config: config(),
            provider: &mut provider,
            approval: &mut approval,
        };
        let error = supervise(&mut command, &mut supervisor, &mut |_| {}).unwrap_err();
        assert!(
            error.to_string().contains("ingot-no-such-program-exists"),
            "{error}"
        );
    }

    #[test]
    fn long_lines_are_truncated_in_diagnostics() {
        let long = "x".repeat(500);
        assert_eq!(truncate(&long, 10), format!("{}…", "x".repeat(10)));
        assert_eq!(truncate("short", 10), "short");
    }
}
