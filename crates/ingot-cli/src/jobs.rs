//! One long command the page started, and its output as it goes.
//!
//! `ingot image build` is not a run. It has no agent, no policy, no budget and
//! no event stream, and nothing about it belongs in a run record. What it shares
//! with a run is only the awkward part: a child that takes minutes, prints while
//! it works, and has to be watchable and stoppable from a page that polls.
//!
//! So this is deliberately not a second launcher. It is the smallest thing that
//! can hold such a child — no standard input, no gate, no history — and it reuses
//! [`crate::launch`]'s bounded capture rather than inventing another one.
//!
//! # One at a time
//!
//! A single slot, and that is a decision rather than a simplification. Two
//! builds of the same tag race to the same name, and a page that can start a
//! second while the first is going is an invitation to do exactly that. A
//! finished job stays in the slot until something replaces it, because the
//! reason to look at a build is usually that it failed.

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde::Serialize;

use crate::launch::{drain, now, Capture};

/// How often the waiter asks whether the child has finished.
const REAP_INTERVAL: Duration = Duration::from_millis(300);

/// A command this studio started.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobView {
    /// What it is, in words a page can show: `building the image`.
    pub label: &'static str,
    pub pid: u32,
    pub started_unix: u64,
    /// `running`, `done`, or `failed`.
    pub state: &'static str,
    pub exit_code: Option<i32>,
    /// Everything it has printed, both streams together.
    pub log: String,
    pub truncated: bool,
}

struct Job {
    label: &'static str,
    pid: u32,
    started_unix: u64,
    child: Arc<Mutex<Child>>,
    finished: Arc<AtomicBool>,
    exit_code: Arc<Mutex<Option<i32>>>,
    log: Arc<Mutex<Capture>>,
}

/// The one slot.
#[derive(Default)]
pub struct Jobs {
    current: Mutex<Option<Arc<Job>>>,
}

impl Jobs {
    /// Start a command, refusing while one is already going.
    pub fn start(
        &self,
        label: &'static str,
        program: PathBuf,
        arguments: Vec<String>,
    ) -> Result<u32> {
        let mut slot = self.current.lock().expect("a poisoned lock");
        if let Some(job) = slot.as_ref() {
            if !job.finished.load(Ordering::SeqCst) {
                bail!(
                    "{} is already going (process {}); wait for it or stop it",
                    job.label,
                    job.pid
                );
            }
        }

        let mut child = Command::new(&program)
            .args(&arguments)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("starting {}", program.display()))?;

        let pid = child.id();
        let log = Arc::new(Mutex::new(Capture::default()));
        let finished = Arc::new(AtomicBool::new(false));
        let exit_code = Arc::new(Mutex::new(None));

        // Both streams into one buffer. A build's two streams are one account of
        // what happened, and separating them here would make a page show the
        // reason for a failure and the step that failed in two different boxes.
        // The interleaving is approximate — two pipes have no shared clock — and
        // that is the honest cost of it being readable.
        drain(child.stdout.take(), Arc::clone(&log));
        drain(child.stderr.take(), Arc::clone(&log));

        let job = Arc::new(Job {
            label,
            pid,
            started_unix: now(),
            child: Arc::new(Mutex::new(child)),
            finished: Arc::clone(&finished),
            exit_code: Arc::clone(&exit_code),
            log,
        });

        {
            let child = Arc::clone(&job.child);
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

        *slot = Some(job);
        Ok(pid)
    }

    /// What the slot holds, if anything.
    pub fn view(&self) -> Option<JobView> {
        let slot = self.current.lock().expect("a poisoned lock");
        let job = slot.as_ref()?;
        let log = job.log.lock().expect("a poisoned lock");
        let done = job.finished.load(Ordering::SeqCst);
        let exit_code = *job.exit_code.lock().expect("a poisoned lock");
        Some(JobView {
            label: job.label,
            pid: job.pid,
            started_unix: job.started_unix,
            state: match (done, exit_code) {
                (false, _) => "running",
                (true, Some(0)) => "done",
                (true, _) => "failed",
            },
            exit_code,
            log: log.text.clone(),
            truncated: log.truncated,
        })
    }

    /// Kill what is going, if anything is.
    ///
    /// The output stays: a build somebody stopped is a build whose last lines are
    /// the reason they stopped it.
    pub fn stop(&self) -> Result<()> {
        let job = {
            let slot = self.current.lock().expect("a poisoned lock");
            match slot.as_ref() {
                Some(job) => Arc::clone(job),
                None => bail!("nothing is going"),
            }
        };
        if job.finished.load(Ordering::SeqCst) {
            bail!("{} has already finished", job.label);
        }
        let mut child = job.child.lock().expect("a poisoned lock");
        child.kill().context("stopping the job")
    }
}
