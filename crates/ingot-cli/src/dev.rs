//! The event-driven `ingot dev` check-build-run loop.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use anyhow::{Context, Result};
use ingot_diagnostics::ColorChoice as RenderColor;
use notify::{Event, EventKind, RecursiveMode, Watcher};

use crate::manifest::{resolve_target, Target, MANIFEST_NAME};
use crate::run::{EventFormat, ProviderChoice, RunConfig};

/// Editors commonly produce several filesystem events for one save. Wait for
/// the burst to finish rather than compiling every rename and metadata update.
/// This blocks on the event channel; it never polls the filesystem.
const DEBOUNCE: Duration = Duration::from_millis(80);

pub struct DevConfig {
    pub run: bool,
    pub inputs: Vec<String>,
    pub provider: ProviderChoice,
    pub cassette: Option<PathBuf>,
    pub agent: Option<String>,
    pub events: EventFormat,
    pub yes: bool,
    pub max_steps: u32,
    pub color: RenderColor,
}

/// Compile immediately, then watch the source and manifest until interrupted.
pub fn watch(path: Option<&Path>, initial: Target, config: &DevConfig) -> Result<u8> {
    let (sender, receiver) = mpsc::channel();
    let mut watcher = notify::recommended_watcher(sender).context("creating filesystem watcher")?;
    let watch_root = if initial.manifest.is_some() {
        initial.root.clone()
    } else {
        initial
            .entry
            .parent()
            .unwrap_or(Path::new("."))
            .to_path_buf()
    };
    watcher
        .watch(
            &watch_root,
            if initial.manifest.is_some() {
                RecursiveMode::Recursive
            } else {
                RecursiveMode::NonRecursive
            },
        )
        .with_context(|| format!("watching {}", watch_root.display()))?;

    eprintln!("dev  watching {}", watch_root.display());
    eprintln!("     press Ctrl+C to stop");

    let mut revision = 1u64;
    let mut last_good = None;
    let mut target = cycle(initial, revision, &mut last_good, config);

    loop {
        let event = receiver
            .recv()
            .context("the filesystem watcher stopped unexpectedly")?
            .context("watching project files")?;
        if !is_relevant(&event, &target) {
            continue;
        }

        drain_burst(&receiver, &target)?;
        revision += 1;
        target = match resolve_target(path) {
            Ok(latest) => cycle(latest, revision, &mut last_good, config),
            Err(error) => {
                eprintln!("[dev {revision}] failed: {error:#}");
                kept(last_good, &target);
                target
            }
        };
    }
}

fn cycle(target: Target, revision: u64, last_good: &mut Option<u64>, config: &DevConfig) -> Target {
    eprintln!("[dev {revision}] checking {}", target.entry.display());
    let compilation = match crate::compile(&target) {
        Ok(compilation) => compilation,
        Err(error) => {
            eprintln!("[dev {revision}] failed: {error:#}");
            kept(*last_good, &target);
            return target;
        }
    };
    crate::report(&compilation, config.color);
    if compilation.has_errors() {
        eprintln!("[dev {revision}] failed; this revision was not built or run");
        kept(*last_good, &target);
        return target;
    }

    if let Err(error) = std::fs::create_dir_all(&target.out_dir)
        .with_context(|| format!("creating {}", target.out_dir.display()))
        .and_then(|_| crate::build_ir(&compilation, &target).map(|_| ()))
    {
        eprintln!("[dev {revision}] build failed: {error:#}");
        kept(*last_good, &target);
        return target;
    }

    *last_good = Some(revision);
    eprintln!(
        "[dev {revision}] ready: built {} agent(s) in {}",
        compilation.agents.len(),
        target.out_dir.display()
    );

    if config.run {
        eprintln!("[dev {revision}] running (runs are serialized)");
        let run = RunConfig {
            inputs: config.inputs.clone(),
            provider: config.provider,
            cassette: config.cassette.clone(),
            record: None,
            model: None,
            effort: None,
            agent: config.agent.clone(),
            out_dir: Some(target.out_dir.join("dev-run")),
            events: config.events,
            yes: config.yes,
            max_steps: config.max_steps,
            root: target.root.clone(),
            mcp: target.mcp(),
            no_tools: false,
            sandbox: false,
            sandbox_allow_unenforced: false,
            workspace: match crate::workspace(None, &target) {
                Ok(workspace) => workspace,
                Err(error) => {
                    eprintln!("[dev {revision}] run skipped: {error:#}");
                    return target;
                }
            },
            models: target.model(),
            contained: false,
            supervised: false,
            image: None,
            timeout_seconds: None,
        };
        match crate::run::execute(&compilation, &run) {
            Ok(crate::EXIT_OK) => eprintln!("[dev {revision}] run complete"),
            Ok(_) => eprintln!("[dev {revision}] run failed; watching for the next revision"),
            Err(error) => eprintln!("[dev {revision}] run failed: {error:#}"),
        }
    }

    target
}

fn kept(last_good: Option<u64>, target: &Target) {
    match last_good {
        Some(revision) => eprintln!(
            "     keeping revision {revision} artifacts in {}",
            target.out_dir.display()
        ),
        None => eprintln!("     no successful artifact exists yet"),
    }
}

fn drain_burst(receiver: &Receiver<notify::Result<Event>>, target: &Target) -> Result<()> {
    loop {
        match receiver.recv_timeout(DEBOUNCE) {
            Ok(Ok(event)) if is_relevant(&event, target) => continue,
            Ok(Ok(_)) => continue,
            Ok(Err(error)) => return Err(error).context("watching project files"),
            Err(mpsc::RecvTimeoutError::Timeout) => return Ok(()),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                anyhow::bail!("the filesystem watcher stopped unexpectedly")
            }
        }
    }
}

fn is_relevant(event: &Event, target: &Target) -> bool {
    if matches!(event.kind, EventKind::Access(_)) {
        return false;
    }
    let entry = absolute(&target.entry);
    let manifest = target
        .manifest
        .as_ref()
        .map(|_| absolute(&target.root.join(MANIFEST_NAME)));
    event.paths.iter().any(|path| {
        let changed = absolute(path);
        changed == entry
            || manifest
                .as_ref()
                .is_some_and(|manifest| changed == *manifest)
    })
}

fn absolute(path: &Path) -> PathBuf {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    };
    path.canonicalize().unwrap_or(path)
}
