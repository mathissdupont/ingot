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
//! # What this deliberately cannot do
//!
//! **Approve an effect.** The child is spawned with no terminal on its standard
//! input, so [`crate::run`] selects `ApprovalMode::Deny`: an artifact that asks
//! for a human does not get a silent yes. `--yes` is not in the argv this
//! builds and there is no field that would put it there.
//!
//! **Keep no record.** `--no-history` is likewise absent. A studio-started run
//! that wrote nothing down would be a run the studio could never show again.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
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

        let mut child = command
            // No terminal on standard input, which is what makes the child
            // deny an effect that asks for a human rather than assume one.
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("starting `ingot run`")?;

        let pid = child.id();
        let output = Arc::new(Mutex::new(Capture::default()));
        let log = Arc::new(Mutex::new(Capture::default()));
        let finished = Arc::new(AtomicBool::new(false));
        let exit_code = Arc::new(Mutex::new(None));

        drain(child.stdout.take(), Arc::clone(&output));
        drain(child.stderr.take(), Arc::clone(&log));

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
                }
            })
            .collect()
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
