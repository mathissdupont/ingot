//! Starting a run from the studio, and being honest about the gap before the
//! record exists.
//!
//! The studio shows runs; `ingot run` performs them. This module is the seam:
//! it spawns the same command a person would type, and then gets out of the
//! way. It does not interpret an artifact, choose a provider, or decide what an
//! effect means — the child does all of that, exactly as it does from a
//! terminal.
//!
//! # Why a launch is a separate thing from a run
//!
//! A run record ([`crate::runs`]) is written by the child, and only once the
//! interpreter reaches `runStarted`. Between the button and that moment the
//! child is compiling, resolving tools and building a provider — and it may
//! fail there, in which case **no record is ever written**. A surface that only
//! read records would show nothing at all for a run that failed to compile, and
//! a button that appears to do nothing is worse than an error.
//!
//! So a *launch* is what this studio started: a process id, a start time, and
//! eventually an exit status with what the child said. It lives in memory for
//! as long as the studio does. The record is the durable half and outlives it.
//!
//! A launch is matched to its record by process id — the record's identifier
//! ends in the pid of the process that wrote it — rather than by asking the
//! child to report one. Nothing new has to cross between them.
//!
//! # Answering a gate
//!
//! The child runs with `--events json --approvals stdin`, so an approval gate
//! it reaches arrives here as an `approvalRequested` event and the answer goes
//! back as one line on its standard input. See
//! [RFC-0020](../../../rfcs/0020-a-person-in-the-loop.md).
//!
//! **This does not weaken what [RFC-0015](../../../rfcs/0015-ingot-studio.md)
//! refused.** That decision was about `--yes` — a blanket answer, given before
//! the run, to gates nobody has seen — on the grounds that a button in the same
//! flow as building gets clicked the way a notification prompt gets clicked.
//! `--yes` is still not in the argv this builds and there is still no field
//! that would put it there. What arrives here is the opposite thing: **one
//! gate, at the moment it is reached, with the effect and the reason in front
//! of the person answering it.** The next gate asks again.
//!
//! Only one gate can be outstanding at a time, because the run blocks on it —
//! so an answer names the node it answers, and one naming any other node is
//! refused rather than applied. A tab left open cannot answer a gate it was
//! never shown.
//!
//! # What this deliberately cannot do
//!
//! **Keep no record.** `--no-history` is absent from the argv. A studio-started
//! run that wrote nothing down would be a run the studio could never show
//! again.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

/// The most output kept from one child, per stream.
///
/// Bounded because a run's trace is unbounded and this is held in memory. The
/// durable account of a run is its record; this is the part a record cannot
/// hold — what the process printed around the event stream.
const MAX_CAPTURE: usize = 64 * 1024;

/// How often a waiter asks whether its child has finished.
///
/// The child is behind a lock so that stopping it is possible, which rules out
/// a blocking `wait`. Slow enough to cost nothing, fast enough that a finished
/// run appears on the next poll of the page.
const REAP_INTERVAL: Duration = Duration::from_millis(200);

/// Providers the studio will pass on. Not a copy of the CLI's list — a subset
/// of it, because a value that is not on it is refused here rather than
/// producing an argument the child does not understand.
const OFFERED_PROVIDERS: &[&str] = &["auto", "anthropic", "google", "openai", "replay"];

/// What the page asked for.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StartRequest {
    /// Which agent, when the program declares several.
    #[serde(default)]
    pub agent: Option<String>,
    /// One of [`OFFERED_PROVIDERS`].
    #[serde(default = "default_provider")]
    pub provider: String,
    /// A cassette to replay, relative to the project.
    #[serde(default)]
    pub cassette: Option<String>,
    /// Declared input name to value.
    #[serde(default)]
    pub inputs: std::collections::BTreeMap<String, String>,
}

fn default_provider() -> String {
    "auto".to_string()
}

/// A gate the run is stopped at, waiting for a person.
///
/// Held rather than merely shown: the run is blocked until this is answered, so
/// a launch carrying one is a launch that will make no further progress. That
/// is why [`LaunchView`] surfaces it — a run waiting on a person must not look
/// the same as a run that is working.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GateView {
    /// The node the gate is in front of. An answer names it, which is what
    /// keeps a stale page from answering a gate it was never shown.
    pub node: String,
    pub effects: Vec<String>,
    /// The label the compiler attached, naming what is about to happen.
    pub reason: String,
}

/// A question the run is stopped at, waiting for a person to answer it.
///
/// The sibling of [`GateView`], and deliberately a separate shape: a gate is
/// answered with a decision and this is answered with a **string the flow then
/// reads**. Nothing here has a safe default — that is why `--yes` can approve a
/// gate and cannot answer a question — so a surface that offers this must offer
/// it as a question and not as something to dismiss.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuestionView {
    /// The node the question is at. An answer names it, for the same reason a
    /// gate's answer does.
    pub node: String,
    /// Which consultation this is within the run, counting from zero — the same
    /// number a cassette matches by.
    pub index: usize,
    pub question: String,
    /// The offered answers, when the program offered any. Empty means the
    /// question takes free text.
    pub choices: Vec<String>,
}

/// What a run has stopped for, when it has stopped for a person.
///
/// One value rather than two optional fields, because the interpreter reaches
/// one of these at a time and waits there: a launch cannot be at a gate *and*
/// at a question, and a shape that could say it was would eventually be made to
/// say it.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "waitingFor", rename_all = "camelCase")]
pub enum Waiting {
    /// An effect the policy gates. Answered with a decision.
    Approval(GateView),
    /// A `consult` in the flow. Answered with one of its choices, or free text.
    Question(QuestionView),
}

impl Waiting {
    /// The node this is waiting at, whichever kind it is.
    fn node(&self) -> &str {
        match self {
            Waiting::Approval(gate) => &gate.node,
            Waiting::Question(question) => &question.node,
        }
    }
}

/// What the page sends back.
///
/// Both fields are optional and exactly one is expected, because **what is
/// legal depends on what the run is waiting at** rather than on what the page
/// felt like sending. The launcher matches them against the outstanding
/// [`Waiting`] and refuses a mismatch — which is the same rule as refusing an
/// answer that names another node, and catches the same thing: a page showing a
/// view the run has moved on from.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AnswerRequest {
    pub node: String,
    /// A decision, for an approval gate.
    #[serde(default)]
    pub allowed: Option<bool>,
    /// An answer, for a question. Never a default: there is no safe side to
    /// guess, so an absent one is refused rather than filled in.
    #[serde(default)]
    pub answer: Option<String>,
}

/// A process this studio started.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchView {
    pub pid: u32,
    pub agent: Option<String>,
    pub provider: String,
    pub started_unix: u64,
    /// `running`, `exited`, or `failed`.
    pub state: &'static str,
    pub exit_code: Option<i32>,
    /// What the child printed to standard output — the agent's artifacts, when
    /// it produced any. Empty while it is still going.
    pub output: String,
    /// The tail of what the child printed to standard error.
    pub log: String,
    /// Whether either capture hit its ceiling.
    pub truncated: bool,
    /// What this run is stopped for, when it is stopped for a person.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pending: Option<Waiting>,
}

struct Launch {
    project: PathBuf,
    pid: u32,
    agent: Option<String>,
    provider: String,
    started_unix: u64,
    child: Arc<Mutex<Child>>,
    finished: Arc<AtomicBool>,
    exit_code: Arc<Mutex<Option<i32>>>,
    output: Arc<Mutex<Capture>>,
    log: Arc<Mutex<Capture>>,
    /// What the run is blocked on, set when `approvalRequested` or
    /// `consultationAsked` arrives and cleared when the matching decision or
    /// answer does. At most one: the interpreter reaches one and waits there.
    pending: Arc<Mutex<Option<Waiting>>>,
    /// The answering end. Taken once the run finishes, so a write to a dead
    /// child is a refusal rather than a broken pipe.
    stdin: Arc<Mutex<Option<ChildStdin>>>,
}

/// A bounded copy of one stream.
#[derive(Default)]
struct Capture {
    text: String,
    truncated: bool,
}

impl Capture {
    fn push(&mut self, chunk: &str) {
        if self.text.len() >= MAX_CAPTURE {
            self.truncated = true;
            return;
        }
        let room = MAX_CAPTURE - self.text.len();
        if chunk.len() <= room {
            self.text.push_str(chunk);
        } else {
            // Cut on a character boundary; the text is shown to a person.
            let mut cut = room;
            while cut > 0 && !chunk.is_char_boundary(cut) {
                cut -= 1;
            }
            self.text.push_str(&chunk[..cut]);
            self.truncated = true;
        }
    }
}

/// Every run this studio has started, for as long as it is running.
#[derive(Default)]
pub struct Launcher {
    launches: Mutex<Vec<Arc<Launch>>>,
}

impl Launcher {
    /// Spawn `ingot run` for this project and remember the process.
    pub fn start(&self, project: &Path, request: &StartRequest) -> Result<u32> {
        if !OFFERED_PROVIDERS.contains(&request.provider.as_str()) {
            bail!(
                "`{}` is not a provider the studio offers ({})",
                request.provider,
                OFFERED_PROVIDERS.join(", ")
            );
        }

        let mut command = Command::new(own_binary()?);
        command
            .arg("run")
            .arg(project)
            .arg("--provider")
            .arg(&request.provider)
            .arg("--color")
            .arg("never");

        if let Some(agent) = &request.agent {
            if agent.starts_with('-') {
                bail!("`{agent}` is not an agent name");
            }
            command.arg("--agent").arg(agent);
        }

        if let Some(cassette) = &request.cassette {
            let path = cassette_inside(project, cassette)?;
            command.arg("--cassette").arg(path);
        } else if request.provider == "replay" {
            bail!("replaying needs a cassette; name one relative to the project");
        }

        for (name, value) in &request.inputs {
            // `--input name=value` is one argument, so a value may hold
            // anything. A name may not, or the split would land elsewhere than
            // where the page meant it to.
            if name.is_empty() || name.contains('=') || name.starts_with('-') {
                bail!("`{name}` is not an input name");
            }
            command.arg("--input").arg(format!("{name}={value}"));
        }

        // The gate leaves on the event stream and the answer comes back on
        // standard input, so both are asked for together: `--approvals stdin`
        // without `--events json` is refused by the child, and rightly.
        command
            .arg("--events")
            .arg("json")
            .arg("--approvals")
            .arg("stdin");

        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("starting `ingot run`")?;

        let pid = child.id();
        let output = Arc::new(Mutex::new(Capture::default()));
        let log = Arc::new(Mutex::new(Capture::default()));
        let finished = Arc::new(AtomicBool::new(false));
        let exit_code = Arc::new(Mutex::new(None));
        let pending = Arc::new(Mutex::new(None));
        let stdin = Arc::new(Mutex::new(child.stdin.take()));

        drain(child.stdout.take(), Arc::clone(&output));
        drain_events(child.stderr.take(), Arc::clone(&log), Arc::clone(&pending));

        let launch = Arc::new(Launch {
            project: project.to_path_buf(),
            pid,
            agent: request.agent.clone(),
            provider: request.provider.clone(),
            started_unix: now(),
            child: Arc::new(Mutex::new(child)),
            finished: Arc::clone(&finished),
            exit_code: Arc::clone(&exit_code),
            output,
            log,
            pending: Arc::clone(&pending),
            stdin: Arc::clone(&stdin),
        });

        // Reaped on its own thread so a finished child does not linger as one,
        // and so the page can see an exit status without asking for it.
        {
            let child = Arc::clone(&launch.child);
            std::thread::spawn(move || loop {
                let status = child
                    .lock()
                    .ok()
                    .and_then(|mut child| child.try_wait().ok().flatten());
                if let Some(status) = status {
                    *exit_code.lock().expect("a poisoned lock") = status.code();
                    // Both dropped before the launch is marked finished, so the
                    // page can never see a gate offered on a run that has
                    // already stopped being able to answer it.
                    pending.lock().expect("a poisoned lock").take();
                    stdin.lock().expect("a poisoned lock").take();
                    finished.store(true, Ordering::SeqCst);
                    return;
                }
                std::thread::sleep(REAP_INTERVAL);
            });
        }

        self.launches.lock().expect("a poisoned lock").push(launch);
        Ok(pid)
    }

    /// Every launch for this project, oldest first.
    pub fn of(&self, project: &Path) -> Vec<LaunchView> {
        self.launches
            .lock()
            .expect("a poisoned lock")
            .iter()
            .filter(|launch| launch.project == project)
            .map(|launch| {
                let output = launch.output.lock().expect("a poisoned lock");
                let log = launch.log.lock().expect("a poisoned lock");
                let done = launch.finished.load(Ordering::SeqCst);
                let exit_code = *launch.exit_code.lock().expect("a poisoned lock");
                LaunchView {
                    pid: launch.pid,
                    agent: launch.agent.clone(),
                    provider: launch.provider.clone(),
                    started_unix: launch.started_unix,
                    state: match (done, exit_code) {
                        (false, _) => "running",
                        (true, Some(0)) => "exited",
                        (true, _) => "failed",
                    },
                    exit_code,
                    output: output.text.clone(),
                    log: log.text.clone(),
                    truncated: output.truncated || log.truncated,
                    pending: launch.pending.lock().expect("a poisoned lock").clone(),
                }
            })
            .collect()
    }

    /// Answer the gate one run is stopped at.
    ///
    /// `node` is the gate being answered, and it has to be the one outstanding.
    /// The run blocks on one gate at a time, so an answer naming any other is a
    /// page that has been looking at a stale view — and applying it would decide
    /// the gate in front of the run with the intent of one already settled. The
    /// child keeps the same rule on its own side, and both keeping it is the
    /// point of a boundary rather than duplication.
    pub fn answer(&self, project: &Path, pid: u32, answer: &AnswerRequest) -> Result<()> {
        let launches = self.launches.lock().expect("a poisoned lock");
        let Some(launch) = launches
            .iter()
            .find(|launch| launch.pid == pid && launch.project == project)
        else {
            bail!("this studio did not start process {pid}");
        };

        // Cloned rather than held, so the line is built without this lock open
        // across the write to the child.
        let waiting = launch.pending.lock().expect("a poisoned lock").clone();
        let node = serde_json::to_string(&answer.node).expect("a string is always serializable");
        let line = match waiting {
            Some(waiting) if waiting.node() != answer.node => bail!(
                "this run is waiting at `{}` and the answer named `{}`; reload and answer what it \
                 is actually waiting for",
                waiting.node(),
                answer.node
            ),
            Some(Waiting::Approval(_)) => {
                let Some(allowed) = answer.allowed else {
                    bail!("this run is at an approval gate, which is answered with a decision");
                };
                if answer.answer.is_some() {
                    bail!("this run is at an approval gate and the answer carried text as well");
                }
                format!(r#"{{"node":{node},"allowed":{allowed}}}"#)
            }
            Some(Waiting::Question(question)) => {
                let Some(text) = answer.answer.as_deref() else {
                    bail!("this run is at a question, which is answered with a string");
                };
                if answer.allowed.is_some() {
                    bail!("this run is at a question and the answer carried a decision as well");
                }
                // Empty is refused rather than sent on, for the reason the
                // terminal refuses it: the flow *reads* this value, so an empty
                // answer is a question silently answered with nothing.
                if text.trim().is_empty() {
                    bail!("a question cannot be answered with nothing");
                }
                // A question that offered choices takes one of them. Checked
                // here as well as in the child because the child's recourse is
                // to ask a terminal again, and there is no terminal to ask.
                if !question.choices.is_empty()
                    && !question.choices.iter().any(|choice| choice == text)
                {
                    bail!(
                        "`{text}` is not one of the answers this question offers: {}",
                        question.choices.join(", ")
                    );
                }
                let text = serde_json::to_string(text).expect("a string is always serializable");
                format!(r#"{{"node":{node},"answer":{text}}}"#)
            }
            None => bail!("this run is not waiting for anybody"),
        };

        let mut handle = launch.stdin.lock().expect("a poisoned lock");
        let Some(stdin) = handle.as_mut() else {
            bail!("this run has finished and cannot be answered");
        };
        writeln!(stdin, "{line}")
            .and_then(|()| stdin.flush())
            .context("answering the run")?;

        // Cleared here rather than waiting for `approvalDecided` or
        // `consultationAnswered` to come back, so the answer cannot be sent
        // twice for one gate or question while the child is still working
        // through it. The event confirms it; this prevents the second write.
        launch.pending.lock().expect("a poisoned lock").take();
        Ok(())
    }

    /// Stop one running child.
    pub fn stop(&self, project: &Path, pid: u32) -> Result<()> {
        let launches = self.launches.lock().expect("a poisoned lock");
        let Some(launch) = launches
            .iter()
            .find(|launch| launch.pid == pid && launch.project == project)
        else {
            bail!("this studio did not start process {pid}");
        };
        if launch.finished.load(Ordering::SeqCst) {
            return Ok(());
        }
        launch
            .child
            .lock()
            .expect("a poisoned lock")
            .kill()
            .context("stopping the run")?;
        Ok(())
    }

    /// Forget the launches that have finished, keeping those still going.
    ///
    /// A launch is the transient half of a run; once its record exists the
    /// record is the account of it. Clearing is a person saying they have read
    /// the ones that failed before a record existed.
    pub fn clear(&self, project: &Path) {
        self.launches
            .lock()
            .expect("a poisoned lock")
            .retain(|launch| launch.project != project || !launch.finished.load(Ordering::SeqCst));
    }
}

/// The `ingot` this studio is part of, so a studio run and a terminal run are
/// the same build.
///
/// Not `"ingot"` from the path: a person testing a build from a checkout would
/// otherwise start whichever one is installed, which is the version of this
/// mistake that is hardest to notice.
fn own_binary() -> Result<PathBuf> {
    std::env::current_exe().context("finding this ingot binary")
}

/// A cassette path that is inside the project, or a refusal.
///
/// The page supplies this, so it is checked rather than trusted. Resolved
/// first, then compared, because `project/../../secrets.json` is only outside
/// once it has been resolved.
fn cassette_inside(project: &Path, cassette: &str) -> Result<PathBuf> {
    let root = project
        .canonicalize()
        .with_context(|| format!("resolving {}", project.display()))?;
    let path = root.join(cassette);
    let resolved = path
        .canonicalize()
        .with_context(|| format!("resolving {}", path.display()))?;
    if !resolved.starts_with(&root) {
        bail!(
            "{} is outside the project; a cassette is named relative to it",
            resolved.display()
        );
    }
    Ok(resolved)
}

/// Copy one child stream into a bounded buffer, on its own thread.
///
/// A child whose pipe fills up stops, so both streams are read whether or not
/// anybody is looking at them yet.
fn drain<R: Read + Send + 'static>(stream: Option<R>, into: Arc<Mutex<Capture>>) {
    let Some(mut stream) = stream else { return };
    std::thread::spawn(move || {
        let mut buffer = [0u8; 8192];
        loop {
            match stream.read(&mut buffer) {
                Ok(0) | Err(_) => return,
                Ok(read) => {
                    let chunk = String::from_utf8_lossy(&buffer[..read]);
                    into.lock().expect("a poisoned lock").push(&chunk);
                }
            }
        }
    });
}

/// The same, for the stream the events arrive on.
///
/// Line-based rather than chunk-based because a gate is a line: under
/// `--events json` every event is one JSON object per line, and the run blocks
/// after emitting `approvalRequested` until somebody answers. Reading by chunk
/// would leave a gate half-parsed in a buffer with nothing further coming to
/// complete it — the one case where waiting for more input never ends.
///
/// Event lines are kept out of `log`. They are already in the run record,
/// verbatim and durable, and duplicating them into a bounded in-memory buffer
/// would push out what only that buffer holds: what the process said *around*
/// the event stream, which is the whole reason it exists.
fn drain_events<R: Read + Send + 'static>(
    stream: Option<R>,
    into: Arc<Mutex<Capture>>,
    pending: Arc<Mutex<Option<Waiting>>>,
) {
    let Some(stream) = stream else { return };
    std::thread::spawn(move || {
        for line in BufReader::new(stream).lines() {
            let Ok(line) = line else { return };
            match gate_event(&line) {
                Some(Event::Requested(waiting)) => {
                    *pending.lock().expect("a poisoned lock") = Some(waiting);
                }
                Some(Event::Decided) => {
                    pending.lock().expect("a poisoned lock").take();
                }
                Some(Event::Other) => {}
                // Not an event: a warning, a hint, a failure, or the model's
                // text arriving live. This is what the capture is for.
                None => {
                    let mut capture = into.lock().expect("a poisoned lock");
                    capture.push(&line);
                    capture.push("\n");
                }
            }
        }
    });
}

enum Event {
    Requested(Waiting),
    Decided,
    Other,
}

/// What one line of the event stream says about a person being waited on, if it
/// is an event at all.
///
/// A line with no `event` key is not one: under `--events json` the model's
/// text arrives live as lines without it, which is the documented shape rather
/// than a quirk to guess at.
///
/// The two halves are read the same way and kept in one place, so a run that
/// stops for a question cannot end up looking like a run that is working just
/// because only one of them was handled here.
fn gate_event(line: &str) -> Option<Event> {
    let value: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
    match value.get("event")?.as_str()? {
        "approvalRequested" => Some(Event::Requested(Waiting::Approval(GateView {
            node: value.get("node")?.as_str()?.to_string(),
            effects: value
                .get("effects")
                .and_then(|effects| effects.as_array())
                .map(|effects| {
                    effects
                        .iter()
                        .filter_map(|effect| effect.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default(),
            reason: value
                .get("reason")
                .and_then(|reason| reason.as_str())
                .unwrap_or_default()
                .to_string(),
        }))),
        // `choices` is omitted when the program offered none, so its absence is
        // a question taking free text rather than a malformed event.
        "consultationAsked" => Some(Event::Requested(Waiting::Question(QuestionView {
            node: value.get("node")?.as_str()?.to_string(),
            index: value
                .get("index")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or_default() as usize,
            question: value.get("question")?.as_str()?.to_string(),
            choices: value
                .get("choices")
                .and_then(|choices| choices.as_array())
                .map(|choices| {
                    choices
                        .iter()
                        .filter_map(|choice| choice.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default(),
        }))),
        "approvalDecided" | "consultationAnswered" => Some(Event::Decided),
        _ => Some(Event::Other),
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(provider: &str) -> StartRequest {
        StartRequest {
            agent: None,
            provider: provider.to_string(),
            cassette: None,
            inputs: Default::default(),
        }
    }

    #[test]
    fn a_provider_the_studio_does_not_offer_is_refused_rather_than_passed_on() {
        let launcher = Launcher::default();
        let error = launcher
            .start(Path::new("."), &request("something-else"))
            .expect_err("an unknown provider must be refused");
        assert!(error.to_string().contains("not a provider"), "{error}");
    }

    #[test]
    fn replaying_without_a_cassette_says_so_before_starting_anything() {
        let launcher = Launcher::default();
        let error = launcher
            .start(Path::new("."), &request("replay"))
            .expect_err("replay needs a cassette");
        assert!(error.to_string().contains("cassette"), "{error}");
    }

    #[test]
    fn an_input_name_cannot_carry_its_own_separator() {
        // `--input a=b=c` splits at the first `=`, so a name holding one would
        // put the value somewhere the page did not mean.
        let launcher = Launcher::default();
        let mut start = request("auto");
        start.inputs.insert("topic=other".into(), "x".into());
        let error = launcher
            .start(Path::new("."), &start)
            .expect_err("a name with a separator must be refused");
        assert!(error.to_string().contains("not an input name"), "{error}");

        let mut flagged = request("auto");
        flagged.inputs.insert("--yes".into(), "x".into());
        assert!(launcher.start(Path::new("."), &flagged).is_err());
    }

    #[test]
    fn a_cassette_outside_the_project_is_refused() {
        // The path comes from a page, so it is checked after resolution: `..`
        // is only outside once it has been followed.
        let root = std::env::temp_dir();
        let error = cassette_inside(&root, "../../etc/passwd")
            .expect_err("a path outside the project must be refused");
        let message = error.to_string();
        assert!(
            message.contains("outside the project") || message.contains("resolving"),
            "{message}"
        );
    }

    #[test]
    fn a_request_cannot_smuggle_a_field_the_studio_does_not_offer() {
        // `deny_unknown_fields`, asserted rather than assumed: `yes` and
        // `noHistory` are the two that would matter, and neither exists.
        let error = serde_json::from_str::<StartRequest>(r#"{"provider":"auto","yes":true}"#)
            .expect_err("an unknown field must be refused");
        assert!(error.to_string().contains("yes"), "{error}");
        assert!(
            serde_json::from_str::<StartRequest>(r#"{"provider":"auto","noHistory":true}"#)
                .is_err()
        );
    }

    #[test]
    fn a_question_in_the_event_stream_is_something_the_run_is_waiting_at() {
        // The event the studio could not see before, which is why a program
        // with a `consult` in it could be started here and never finished.
        let line = r#"{"event":"consultationAsked","node":"n7","index":0,
            "question":"Which framing?","choices":["technical","executive"]}"#;
        let Some(Event::Requested(Waiting::Question(question))) = gate_event(line) else {
            panic!("a question must be recognised as something to wait at");
        };
        assert_eq!(question.node, "n7");
        assert_eq!(question.question, "Which framing?");
        assert_eq!(question.choices, ["technical", "executive"]);
    }

    #[test]
    fn a_question_with_no_choices_takes_free_text_rather_than_being_malformed() {
        // `choices` is skipped when empty, so its absence has to read as "any
        // answer" and not as an event this cannot parse.
        let line = r#"{"event":"consultationAsked","node":"n1","index":0,"question":"Who for?"}"#;
        let Some(Event::Requested(Waiting::Question(question))) = gate_event(line) else {
            panic!("a question without choices is still a question");
        };
        assert!(question.choices.is_empty());
    }

    #[test]
    fn an_answered_question_stops_being_something_to_wait_at() {
        let line = r#"{"event":"consultationAnswered","node":"n7","index":0,"answer":"technical"}"#;
        assert!(matches!(gate_event(line), Some(Event::Decided)));
    }

    #[test]
    fn an_approval_is_still_read_as_an_approval() {
        // The two halves share one function now, so this asserts the older one
        // did not change shape on the way.
        let line = r#"{"event":"approvalRequested","node":"n2","effects":["network"],
            "reason":"reaching arxiv.org"}"#;
        let Some(Event::Requested(Waiting::Approval(gate))) = gate_event(line) else {
            panic!("an approval must still be an approval");
        };
        assert_eq!(gate.node, "n2");
        assert_eq!(gate.effects, ["network"]);
    }

    #[test]
    fn an_answer_carries_a_decision_or_a_string_and_nothing_else() {
        let decision: AnswerRequest =
            serde_json::from_str(r#"{"node":"n2","allowed":true}"#).expect("a decision");
        assert_eq!(decision.allowed, Some(true));
        assert!(decision.answer.is_none());

        let answered: AnswerRequest =
            serde_json::from_str(r#"{"node":"n7","answer":"technical"}"#).expect("an answer");
        assert_eq!(answered.answer.as_deref(), Some("technical"));
        assert!(answered.allowed.is_none());

        // `deny_unknown_fields`, asserted rather than assumed: a page cannot
        // invent a field here any more than it can on a start request.
        assert!(serde_json::from_str::<AnswerRequest>(r#"{"node":"n7","yes":true}"#).is_err());
    }

    #[test]
    fn a_capture_stops_at_its_ceiling_and_says_so() {
        let mut capture = Capture::default();
        capture.push(&"a".repeat(MAX_CAPTURE - 1));
        assert!(!capture.truncated);
        capture.push("bc");
        assert_eq!(capture.text.len(), MAX_CAPTURE);
        assert!(capture.truncated);
    }

    #[test]
    fn a_multibyte_character_is_not_cut_in_half() {
        let mut capture = Capture::default();
        capture.push(&"a".repeat(MAX_CAPTURE - 1));
        capture.push("ş");
        // No room for both bytes, so neither is written and the text stays
        // valid UTF-8 rather than ending in half a character.
        assert_eq!(capture.text.len(), MAX_CAPTURE - 1);
        assert!(capture.truncated);
    }
}
