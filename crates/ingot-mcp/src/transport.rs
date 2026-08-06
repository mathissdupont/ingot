//! Getting bytes to and from an MCP server.
//!
//! The only transport Ingot implements is a child process speaking
//! newline-delimited JSON on its standard streams. It is behind a trait anyway,
//! because a test that drives the protocol should not have to spawn anything,
//! and because an HTTP transport is a later addition rather than a rewrite.
//!
//! Two properties matter more than they might look:
//!
//! * **Reads have a deadline.** A server that accepts a request and never
//!   answers must not hang the run. Child pipes have no read timeout, so the
//!   reading happens on its own thread and the caller waits on a channel.
//! * **Standard error is kept.** When a server dies, its last words are the
//!   only useful thing in the failure message.

use std::collections::BTreeMap;
use std::fmt;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How many stderr lines to keep for a failure message.
const STDERR_LINES: usize = 20;
/// How long a closing server is given to exit before it is killed.
const SHUTDOWN_GRACE: Duration = Duration::from_millis(500);
/// How long to wait for a dying server's standard error to arrive.
const DIAGNOSTICS_GRACE: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransportError {
    /// The peer went away.
    Closed(String),
    /// The deadline passed with no message.
    Timeout,
    /// The pipe itself failed.
    Io(String),
    /// The process could not be started.
    Spawn(String),
}

impl fmt::Display for TransportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TransportError::Closed(reason) => write!(f, "the server closed the connection{reason}"),
            TransportError::Timeout => f.write_str("the server did not answer in time"),
            TransportError::Io(reason) => write!(f, "the connection failed: {reason}"),
            TransportError::Spawn(reason) => write!(f, "the server could not be started: {reason}"),
        }
    }
}

impl std::error::Error for TransportError {}

/// A bidirectional stream of newline-delimited JSON messages.
pub trait Transport {
    /// Send one message. `line` never contains a newline.
    fn send(&mut self, line: &str) -> Result<(), TransportError>;

    /// Wait up to `timeout` for the next message.
    fn recv(&mut self, timeout: Duration) -> Result<String, TransportError>;

    /// Whatever the peer wrote to standard error, for a failure message.
    fn diagnostics(&self) -> String;

    /// Close politely, then forcefully. Idempotent.
    fn shutdown(&mut self);
}

/// The environment variables a child always keeps.
///
/// Clearing the environment outright breaks a subprocess on every platform —
/// Windows needs `SystemRoot` to open a socket, Unix programs need `PATH` — so
/// the choice is not "inherit or not" but "which". Everything else, including
/// every credential the operator happens to have exported, is dropped unless a
/// server's `pass-env` names it.
const ALWAYS_PASSED: &[&str] = &[
    "PATH",
    "PATHEXT",
    "SYSTEMROOT",
    "SystemRoot",
    "SYSTEMDRIVE",
    "SystemDrive",
    "COMSPEC",
    "ComSpec",
    "WINDIR",
    "windir",
    "TEMP",
    "TMP",
    "TMPDIR",
    "HOME",
    "USERPROFILE",
    "LANG",
    "LC_ALL",
    "TZ",
];

/// Work out the environment a server is started with.
///
/// Separate from the spawn so that the policy — deny by default, names only —
/// is testable without starting a process.
pub fn child_environment(
    pass_env: &[String],
    parent: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let mut environment = BTreeMap::new();
    for name in ALWAYS_PASSED {
        if let Some(value) = parent.get(*name) {
            environment.insert((*name).to_string(), value.clone());
        }
    }
    for name in pass_env {
        if let Some(value) = parent.get(name) {
            environment.insert(name.clone(), value.clone());
        }
    }
    environment
}

fn parent_environment() -> BTreeMap<String, String> {
    std::env::vars().collect()
}

enum FromReader {
    Line(String),
    Failed(String),
    Eof,
}

/// A child process speaking MCP on its standard streams.
#[derive(Debug)]
pub struct ChildTransport {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Receiver<FromReader>,
    stderr: Arc<Mutex<Vec<String>>>,
    /// Set when the stderr pipe closes, which is when the child has gone.
    stderr_done: Arc<AtomicBool>,
    closed: bool,
}

impl ChildTransport {
    pub fn spawn(
        command: &str,
        args: &[String],
        cwd: Option<&Path>,
        pass_env: &[String],
    ) -> Result<ChildTransport, TransportError> {
        let mut builder = Command::new(command);
        builder
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env_clear()
            .envs(child_environment(pass_env, &parent_environment()));
        if let Some(cwd) = cwd {
            builder.current_dir(cwd);
        }

        let mut child = builder
            .spawn()
            .map_err(|error| TransportError::Spawn(format!("`{command}`: {error}")))?;

        let stdin = child.stdin.take().expect("stdin was piped");
        let stdout = child.stdout.take().expect("stdout was piped");
        let child_stderr = child.stderr.take().expect("stderr was piped");

        // A bounded channel so a chatty server cannot grow the buffer without
        // limit; the reader blocks instead, which is the correct backpressure.
        let (sender, lines): (SyncSender<FromReader>, Receiver<FromReader>) = sync_channel(64);
        std::thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                let message = match line {
                    Ok(line) => FromReader::Line(line),
                    Err(error) => FromReader::Failed(error.to_string()),
                };
                let failed = matches!(message, FromReader::Failed(_));
                if sender.send(message).is_err() || failed {
                    return;
                }
            }
            let _ = sender.send(FromReader::Eof);
        });

        let stderr = Arc::new(Mutex::new(Vec::new()));
        let stderr_done = Arc::new(AtomicBool::new(false));
        let sink = Arc::clone(&stderr);
        let finished = Arc::clone(&stderr_done);
        std::thread::spawn(move || {
            let reader = BufReader::new(child_stderr);
            for line in reader.lines().map_while(Result::ok) {
                let Ok(mut buffer) = sink.lock() else { break };
                buffer.push(line);
                if buffer.len() > STDERR_LINES {
                    buffer.remove(0);
                }
            }
            finished.store(true, Ordering::SeqCst);
        });

        Ok(ChildTransport {
            child,
            stdin: Some(stdin),
            lines,
            stderr,
            stderr_done,
            closed: false,
        })
    }
}

impl Transport for ChildTransport {
    fn send(&mut self, line: &str) -> Result<(), TransportError> {
        let Some(stdin) = self.stdin.as_mut() else {
            return Err(TransportError::Closed(String::new()));
        };
        writeln!(stdin, "{line}").map_err(|error| TransportError::Io(error.to_string()))?;
        stdin
            .flush()
            .map_err(|error| TransportError::Io(error.to_string()))
    }

    fn recv(&mut self, timeout: Duration) -> Result<String, TransportError> {
        match self.lines.recv_timeout(timeout) {
            Ok(FromReader::Line(line)) => Ok(line),
            Ok(FromReader::Failed(reason)) => Err(TransportError::Io(reason)),
            Ok(FromReader::Eof) => Err(TransportError::Closed(self.exit_note())),
            Err(RecvTimeoutError::Timeout) => Err(TransportError::Timeout),
            Err(RecvTimeoutError::Disconnected) => Err(TransportError::Closed(self.exit_note())),
        }
    }

    fn diagnostics(&self) -> String {
        // Only ever called on a failing run, and the case that matters most is
        // a server that died during startup: the write that fails can beat the
        // reader thread to the pipe, leaving the buffer empty at exactly the
        // moment its contents are the whole explanation. Wait, briefly, for the
        // pipe to close — which is when the child has gone and there is nothing
        // more to come.
        if self.collected().is_empty() {
            let deadline = Instant::now() + DIAGNOSTICS_GRACE;
            while !self.stderr_done.load(Ordering::SeqCst) && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        self.collected()
    }

    fn shutdown(&mut self) {
        if self.closed {
            return;
        }
        self.closed = true;

        // Closing stdin is how an MCP server is asked to stop. Give it a moment
        // to do so before killing it, so it can flush and clean up.
        drop(self.stdin.take());
        let deadline = Instant::now() + SHUTDOWN_GRACE;
        while Instant::now() < deadline {
            match self.child.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) => std::thread::sleep(Duration::from_millis(10)),
                Err(_) => break,
            }
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl ChildTransport {
    fn collected(&self) -> String {
        self.stderr
            .lock()
            .map(|buffer| buffer.join("\n"))
            .unwrap_or_default()
    }

    fn exit_note(&mut self) -> String {
        match self.child.try_wait() {
            Ok(Some(status)) => format!(" (it exited with {status})"),
            _ => String::new(),
        }
    }
}

impl Drop for ChildTransport {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Answers one incoming line with zero or more outgoing lines, so a fake peer
/// can send notifications alongside a reply.
type Handler = Box<dyn FnMut(&str) -> Vec<String>>;

/// An in-process peer, for tests that exercise the protocol rather than the
/// process.
pub struct LoopbackTransport {
    handler: Handler,
    pending: std::collections::VecDeque<String>,
    stderr: String,
    closed: bool,
}

impl LoopbackTransport {
    pub fn new(handler: impl FnMut(&str) -> Vec<String> + 'static) -> LoopbackTransport {
        LoopbackTransport {
            handler: Box::new(handler),
            pending: std::collections::VecDeque::new(),
            stderr: String::new(),
            closed: false,
        }
    }

    pub fn with_stderr(mut self, stderr: &str) -> LoopbackTransport {
        self.stderr = stderr.to_string();
        self
    }
}

impl Transport for LoopbackTransport {
    fn send(&mut self, line: &str) -> Result<(), TransportError> {
        if self.closed {
            return Err(TransportError::Closed(String::new()));
        }
        self.pending.extend((self.handler)(line));
        Ok(())
    }

    fn recv(&mut self, _timeout: Duration) -> Result<String, TransportError> {
        match self.pending.pop_front() {
            Some(line) => Ok(line),
            None if self.closed => Err(TransportError::Closed(String::new())),
            // Nothing queued and nothing coming: a real transport would block
            // until the deadline, so report the same outcome without waiting.
            None => Err(TransportError::Timeout),
        }
    }

    fn diagnostics(&self) -> String {
        self.stderr.clone()
    }

    fn shutdown(&mut self) {
        self.closed = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parent() -> BTreeMap<String, String> {
        [
            ("PATH", "/usr/bin"),
            ("ANTHROPIC_API_KEY", "sk-secret"),
            ("BRAVE_API_KEY", "brave-secret"),
            ("RANDOM_THING", "x"),
        ]
        .into_iter()
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect()
    }

    #[test]
    fn a_server_inherits_nothing_it_was_not_given() {
        let environment = child_environment(&[], &parent());
        assert_eq!(
            environment.get("PATH").map(String::as_str),
            Some("/usr/bin")
        );
        assert!(
            !environment.contains_key("ANTHROPIC_API_KEY"),
            "a credential must not reach a tool server by accident: {environment:?}"
        );
        assert!(!environment.contains_key("RANDOM_THING"));
    }

    #[test]
    fn pass_env_names_a_variable_without_carrying_its_value() {
        let environment = child_environment(&["BRAVE_API_KEY".to_string()], &parent());
        assert_eq!(
            environment.get("BRAVE_API_KEY").map(String::as_str),
            Some("brave-secret")
        );
        assert!(!environment.contains_key("ANTHROPIC_API_KEY"));
    }

    #[test]
    fn naming_a_variable_the_operator_does_not_have_is_not_an_error() {
        let environment = child_environment(&["ABSENT".to_string()], &parent());
        assert!(!environment.contains_key("ABSENT"));
    }

    #[test]
    fn the_loopback_transport_replies_to_what_it_is_sent() {
        let mut transport = LoopbackTransport::new(|line| {
            assert!(line.contains("ping"));
            vec!["pong".to_string()]
        });
        transport.send("ping").unwrap();
        assert_eq!(transport.recv(Duration::from_millis(1)).unwrap(), "pong");
        assert_eq!(
            transport.recv(Duration::from_millis(1)),
            Err(TransportError::Timeout)
        );
    }

    #[test]
    fn a_closed_loopback_transport_refuses_to_send() {
        let mut transport = LoopbackTransport::new(|_| vec![]);
        transport.shutdown();
        assert!(matches!(
            transport.send("x"),
            Err(TransportError::Closed(_))
        ));
    }

    #[test]
    fn spawning_something_that_does_not_exist_says_so() {
        let error = ChildTransport::spawn("ingot-definitely-not-a-real-program", &[], None, &[])
            .unwrap_err();
        assert!(
            matches!(error, TransportError::Spawn(ref reason) if reason.contains("ingot-definitely-not-a-real-program")),
            "{error}"
        );
    }
}
