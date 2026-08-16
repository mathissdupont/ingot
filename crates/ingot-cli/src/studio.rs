//! `ingot studio` — the routes behind the local surface.
//!
//! [`ingot_studio`] is a socket, a guard and a page; this is the half that
//! knows what an Ingot project is. Every route here calls the same function a
//! subcommand calls — [`crate::doctor::report`] is `ingot doctor`,
//! [`crate::sandbox::plan_all`] is `ingot sandbox`, [`compile_path`] is
//! `ingot check` — so a page and a terminal cannot disagree about a project.
//!
//! # Nothing is remembered that can be re-read
//!
//! Docker has a daemon holding a list. Ingot has no such thing: a project is a
//! directory, a provider is an environment variable, a run is a process. The
//! studio keeps exactly two things, and is deliberately awkward about both:
//!
//! * **A bookmark file**, holding paths and nothing else. Every fact about a
//!   project is read from the project when it is asked for, so losing the file
//!   loses bookmarks and no information.
//! * **Run records**, written by `ingot run` itself into the project's build
//!   directory — see [`crate::runs`]. The studio reads them; it does not own
//!   them, and a project with no studio still has its history.
//!
//! # Nothing is written that a person did not write
//!
//! The studio has no field to type a credential into and no route that would
//! accept one. Connecting a model service means naming the environment
//! variable it reads, in a manifest, by hand. The page shows the shape to
//! write and where it goes; it does not write it. A surface that edited a
//! hand-written manifest by re-serializing it would lose the comments in it,
//! and that is the same mistake as regenerating a source file from a diagram.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use ingot_compiler::compile_path;
use ingot_diagnostics::Severity;
use ingot_language_service::canvas::{apply as apply_edit, canvas_of, CanvasEdit};
use ingot_language_service::LanguageService;
use ingot_studio::{Answers, Head, Method, Reply, Studio};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::launch;
use crate::manifest::{resolve_target, MANIFEST_NAME};
use crate::runs;

const STUDIO_SCHEMA_VERSION: u32 = 1;

/// The port `ingot studio` asks for first.
///
/// A fixed one so the URL is the same every day, and one nothing else is known
/// to want. When it is taken, [`serve`] falls back to whatever is free rather
/// than refusing to start.
const PREFERRED_PORT: u16 = 7317;

pub struct StudioConfig {
    /// Address to listen on. Must be loopback; `ingot-studio` refuses anything
    /// else rather than trusting this to have checked.
    pub bind: Option<String>,
}

/// Start the studio and block until the process is interrupted.
pub fn serve(config: &StudioConfig) -> Result<u8> {
    let bind = match &config.bind {
        Some(address) => address
            .parse::<SocketAddr>()
            .with_context(|| format!("`{address}` is not an address:port"))?,
        None => SocketAddr::from(([127, 0, 0, 1], PREFERRED_PORT)),
    };

    let routes = Arc::new(Routes::default());
    let mut studio = match Studio::start(bind, routes.clone()) {
        Ok(studio) => studio,
        Err(error) if config.bind.is_none() && error.kind() == std::io::ErrorKind::AddrInUse => {
            // Something already has 7317. Take any free port rather than making
            // the person find out what and free it.
            Studio::start(SocketAddr::from(([127, 0, 0, 1], 0)), routes)
                .context("starting the studio")?
        }
        Err(error) => return Err(error).context("starting the studio"),
    };

    // The URL goes to standard output and everything else to standard error, so
    // the one thing a script wants is the one thing a pipe gets. It carries the
    // token, which is why nothing else prints it.
    println!("{}", studio.url());
    eprintln!("Ingot Studio is serving on {}", studio.address());
    eprintln!("open the URL above; the token in it is this process's and is stored nowhere");
    eprintln!("press Ctrl-C to stop");
    studio.wait();
    Ok(crate::EXIT_OK)
}

// --- routing ---------------------------------------------------------------

#[derive(Default)]
struct Routes {
    /// The runs this studio started, for as long as it is running. See
    /// [`crate::launch`] for why a launch is not the same thing as a record.
    launcher: launch::Launcher,
}

impl Answers for Routes {
    fn answer(&self, request: &Head, body: &[u8]) -> Reply {
        let route = request.path.strip_prefix("/api/").unwrap_or_default();
        let result = match (request.method, route) {
            (Method::Get, "projects") => bookmarked(),
            (Method::Post, "projects") => with_path(request, |path| bookmark(path, true)),
            (Method::Delete, "projects") => with_path(request, |path| bookmark(path, false)),
            (Method::Get, "project") => with_path(request, project),
            (Method::Get, "canvas") => with_path(request, |path| canvas(request, path)),
            (Method::Post, "canvas") => with_path(request, |path| edit_canvas(path, body)),
            (Method::Get, "runs") => with_path(request, |path| self.run_list(path)),
            (Method::Get, "run") => with_id(request, run_detail),
            (Method::Delete, "run") => with_id(request, |path, id| self.run_delete(path, id)),
            (Method::Post, "run") => with_path(request, |path| self.start(path, body)),
            (Method::Delete, "launch") => with_path(request, |path| self.stop(request, path)),
            (Method::Post, "approval") => {
                with_path(request, |path| self.answer(request, path, body))
            }
            (Method::Post, "launches") => with_path(request, |path| {
                self.launcher.clear(path);
                self.run_list(path)
            }),
            (Method::Get, "machine") => machine(),
            _ => return Reply::Unknown,
        };
        match result {
            Ok(document) => Reply::Json(document),
            // Every route here reads a directory the caller named, so a failure
            // is nearly always "that is not a project" — the caller's business
            // rather than the server's.
            Err(error) => Reply::Refused(format!("{error:#}")),
        }
    }
}

impl Routes {
    /// The durable half and the transient half of a project's runs.
    ///
    /// One route rather than two because the page needs both to say anything
    /// true: a record is what a run *did*, and a launch is what this studio
    /// started — including the ones that failed before a record existed.
    fn run_list(&self, path: &Path) -> Result<String> {
        let target = resolve_target(Some(path))?;
        Ok(serde_json::to_string(&json!({
            "schemaVersion": STUDIO_SCHEMA_VERSION,
            "runs": runs::list(&target.out_dir),
            "launches": self.launcher.of(&resolve(path)),
        }))?)
    }

    fn start(&self, path: &Path, body: &[u8]) -> Result<String> {
        // The project has to be one this studio can resolve before anything is
        // spawned: `ingot run` would say the same thing, but from a process
        // whose failure the page would have to go looking for.
        let target = resolve_target(Some(path))?;
        let request: launch::StartRequest = if body.is_empty() {
            serde_json::from_str("{}")?
        } else {
            serde_json::from_slice(body).context("reading the run request")?
        };
        let project = resolve(&target.root);
        self.launcher.start(&project, &request)?;
        self.run_list(path)
    }

    fn run_delete(&self, path: &Path, id: &str) -> Result<String> {
        let target = resolve_target(Some(path))?;
        runs::delete(&target.out_dir, id)?;
        self.run_list(path)
    }

    fn stop(&self, request: &Head, path: &Path) -> Result<String> {
        let Some(pid) = request.param("pid").and_then(|pid| pid.parse::<u32>().ok()) else {
            bail!("this route needs a numeric `pid` parameter");
        };
        self.launcher.stop(&resolve(path), pid)?;
        self.run_list(path)
    }

    /// Answer the one gate a run is stopped at.
    ///
    /// The body names the node, and the launcher refuses an answer for any node
    /// but the outstanding one — which is what a page left open in another tab
    /// would send. See [RFC-0020](../../../rfcs/0020-a-person-in-the-loop.md).
    fn answer(&self, request: &Head, path: &Path, body: &[u8]) -> Result<String> {
        let Some(pid) = request.param("pid").and_then(|pid| pid.parse::<u32>().ok()) else {
            bail!("this route needs a numeric `pid` parameter");
        };
        let answer: launch::AnswerRequest =
            serde_json::from_slice(body).context("reading the answer")?;
        self.launcher.answer(&resolve(path), pid, &answer)?;
        self.run_list(path)
    }
}

fn with_path(request: &Head, then: impl Fn(&Path) -> Result<String>) -> Result<String> {
    let Some(path) = request.param("path") else {
        bail!("this route needs a `path` parameter");
    };
    then(Path::new(path))
}

fn with_id(request: &Head, then: impl Fn(&Path, &str) -> Result<String>) -> Result<String> {
    let (Some(path), Some(id)) = (request.param("path"), request.param("id")) else {
        bail!("this route needs `path` and `id` parameters");
    };
    then(Path::new(path), id)
}

// --- the bookmark file ------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProjectEntry {
    path: String,
    name: String,
    version: Option<String>,
    description: Option<String>,
    /// Why this bookmark cannot be read right now, if it cannot.
    problem: Option<String>,
    runs: usize,
}

/// The bookmark file, which holds paths and nothing else.
///
/// Not TOML like a manifest: this is not a thing anyone edits by hand, and
/// nothing about a machine's own list belongs in a project's configuration.
fn bookmarks_path() -> Result<PathBuf> {
    // The override exists so a test can have its own list, and so someone who
    // keeps their configuration somewhere specific can say where.
    if let Some(directory) = std::env::var_os("INGOT_CONFIG_DIR") {
        return Ok(PathBuf::from(directory).join("projects.json"));
    }
    let base = if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .map(|home| home.join("Library").join("Application Support"))
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .map(|home| home.join(".config"))
            })
    };
    let Some(base) = base else {
        bail!(
            "no configuration directory could be found; set INGOT_CONFIG_DIR to say where the \
             project list should live"
        );
    };
    Ok(base.join("ingot").join("projects.json"))
}

fn load_bookmarks() -> Vec<PathBuf> {
    let Ok(path) = bookmarks_path() else {
        return Vec::new();
    };
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|value| {
            value
                .get("projects")?
                .as_array()?
                .iter()
                .map(|entry| entry.as_str().map(PathBuf::from))
                .collect::<Option<Vec<_>>>()
        })
        .unwrap_or_default()
}

fn save_bookmarks(projects: &[PathBuf]) -> Result<()> {
    let path = bookmarks_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let document = json!({
        "schemaVersion": STUDIO_SCHEMA_VERSION,
        "projects": projects.iter().map(|path| path.display().to_string()).collect::<Vec<_>>(),
    });
    std::fs::write(
        &path,
        format!("{}\n", serde_json::to_string_pretty(&document)?),
    )
    .with_context(|| format!("writing {}", path.display()))
}

fn bookmarked() -> Result<String> {
    let entries: Vec<ProjectEntry> = load_bookmarks()
        .iter()
        .map(PathBuf::as_path)
        .map(describe)
        .collect();
    Ok(serde_json::to_string(&json!({
        "schemaVersion": STUDIO_SCHEMA_VERSION,
        "projects": entries,
    }))?)
}

/// Add or remove one bookmark, then answer with the whole list.
fn bookmark(path: &Path, add: bool) -> Result<String> {
    let mut projects = load_bookmarks();
    if add {
        let manifest = path.join(MANIFEST_NAME);
        if !manifest.is_file() {
            bail!(
                "{} holds no {MANIFEST_NAME}\n  a project is the directory the manifest is in; \
                 `ingot init <name>` creates one",
                path.display()
            );
        }
        // Canonicalised so the same directory reached two ways is one entry,
        // and so a relative path typed into the page keeps working after the
        // studio's own working directory stops being relevant.
        let resolved = resolve(path);
        if !projects.contains(&resolved) {
            projects.push(resolved);
        }
    } else {
        let resolved = resolve(path);
        projects.retain(|entry| entry != path && entry != &resolved);
    }
    save_bookmarks(&projects)?;
    bookmarked()
}

/// An absolute path a person would recognise as theirs.
///
/// `canonicalize` on Windows returns the extended-length form — `\\?\C:\…` —
/// which is a correct path and a wrong-looking one. It goes into the bookmark
/// file, comes back out in a URL and ends up on screen, so the prefix is
/// removed for the ordinary drive case. A verbatim UNC path (`\\?\UNC\…`) is
/// *not* the same path with its prefix removed, so it is left alone.
fn resolve(path: &Path) -> PathBuf {
    let resolved = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    if cfg!(windows) {
        let text = resolved.display().to_string();
        if let Some(rest) = text.strip_prefix(r"\\?\") {
            if rest.as_bytes().get(1) == Some(&b':') {
                return PathBuf::from(rest);
            }
        }
    }
    resolved
}

/// What can be said about a bookmark without compiling anything.
///
/// The list is read on every page load, so it stops at the manifest. Anything
/// that costs a compile belongs in [`project`], which is asked for one project
/// at a time.
fn describe(path: &Path) -> ProjectEntry {
    let display = path.display().to_string();
    let fallback = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| display.clone());

    match resolve_target(Some(path)) {
        Ok(target) => {
            let manifest = target.manifest.as_ref();
            ProjectEntry {
                path: display,
                name: manifest
                    .map(|manifest| manifest.project.name.clone())
                    .unwrap_or(fallback),
                version: manifest.map(|manifest| manifest.project.version.clone()),
                description: manifest.and_then(|manifest| manifest.project.description.clone()),
                problem: None,
                runs: runs::count(&target.out_dir),
            }
        }
        Err(error) => ProjectEntry {
            path: display,
            name: fallback,
            version: None,
            description: None,
            problem: Some(format!("{error:#}")),
            runs: 0,
        },
    }
}

// --- one project ------------------------------------------------------------

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct EditorNote {
    code: String,
    severity: &'static str,
    message: String,
    /// `file:line:column`, the same anchor a terminal prints.
    location: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AgentView {
    name: String,
    inputs: Vec<Field>,
    outputs: Vec<Field>,
    effects: Vec<String>,
    tools: Vec<ToolView>,
    model: String,
    steps: usize,
}

#[derive(Debug, Serialize)]
struct Field {
    name: String,
    #[serde(rename = "type")]
    ty: String,
}

#[derive(Debug, Serialize)]
struct ToolView {
    name: String,
    /// The hosts and paths this tool's grants are narrowed to, flattened for
    /// display: `network arxiv.org`, `read src`.
    reach: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PlanView {
    server: String,
    agent: String,
    enforced: bool,
    /// The plan as `ingot sandbox` prints it, so the two cannot drift.
    rendered: String,
}

// --- the canvas -------------------------------------------------------------

/// What the page sends back to change a file.
///
/// `expected` is what makes this safe to accept from a page that read the file
/// some time ago. See [RFC-0016](../../../rfcs/0016-the-canvas.md).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EditRequest {
    start_byte: u32,
    end_byte: u32,
    expected: String,
    new_text: String,
}

/// Render one agent's flow as a canvas.
///
/// **The route names no file.** It renders the project's entry, resolved the way
/// every other route resolves it, so a page cannot ask this studio to read or
/// write something outside the project by naming a path.
fn canvas(request: &Head, path: &Path) -> Result<String> {
    let target = resolve_target(Some(path))?;
    let source = std::fs::read_to_string(&target.entry)
        .with_context(|| format!("reading {}", target.entry.display()))?;
    let compilation = compile_path(&target.entry)
        .with_context(|| format!("reading {}", target.entry.display()))?;

    let agent = request.param("agent");
    let Some(drawn) = canvas_of(&compilation.sources, &compilation.program, &source, agent) else {
        bail!("this project declares no agent with a flow to draw");
    };

    Ok(serde_json::to_string(&json!({
        "schemaVersion": STUDIO_SCHEMA_VERSION,
        "file": target.entry.display().to_string(),
        // The whole file, so the page can show a gesture as a diff of the lines
        // it will change *before* it changes them. Cheap here and nowhere else:
        // the canvas already has the exact range and the exact replacement,
        // because that is all an edit is.
        "source": source,
        "canvas": drawn,
        "agents": compilation
            .program
            .agents
            .iter()
            .map(|decl| decl.name.text.clone())
            .collect::<Vec<_>>(),
        "diagnostics": notes(&target.entry)?,
    }))?)
}

/// Apply one edit and hand back what the file became.
///
/// The order is the point: apply, write, then **compile**. The canvas is never
/// authoritative about correctness — it proposes an edit and `ingot check`
/// decides, so a bug here is a diagnostic rather than a corrupted program.
fn edit_canvas(path: &Path, body: &[u8]) -> Result<String> {
    let target = resolve_target(Some(path))?;
    let request: EditRequest = serde_json::from_slice(body).context("reading the edit")?;
    let source = std::fs::read_to_string(&target.entry)
        .with_context(|| format!("reading {}", target.entry.display()))?;

    let edited = apply_edit(
        &source,
        &CanvasEdit {
            start_byte: request.start_byte,
            end_byte: request.end_byte,
            expected: request.expected,
            new_text: request.new_text,
        },
    )
    .map_err(|refused| anyhow::anyhow!("{refused}"))?;

    std::fs::write(&target.entry, &edited)
        .with_context(|| format!("writing {}", target.entry.display()))?;

    // Re-rendered from the file that is now on disk rather than from the string
    // in hand, so the page's next gesture is computed against what a compiler
    // would read.
    let compilation = compile_path(&target.entry)
        .with_context(|| format!("reading {}", target.entry.display()))?;
    let drawn = canvas_of(&compilation.sources, &compilation.program, &edited, None);

    Ok(serde_json::to_string(&json!({
        "schemaVersion": STUDIO_SCHEMA_VERSION,
        "file": target.entry.display().to_string(),
        "source": edited,
        "canvas": drawn,
        "diagnostics": notes(&target.entry)?,
    }))?)
}

/// The diagnostics for one file, in the shape the page already renders.
fn notes(entry: &Path) -> Result<Vec<EditorNote>> {
    let service = LanguageService::new();
    Ok(service
        .check_file(entry)?
        .diagnostics
        .iter()
        .map(|diagnostic| EditorNote {
            code: diagnostic.code.clone(),
            severity: match diagnostic.severity {
                Severity::Error => "error",
                Severity::Warning => "warning",
                Severity::Note => "note",
            },
            message: diagnostic.message.clone(),
            location: format!(
                "{}:{}:{}",
                diagnostic.range.file,
                diagnostic.range.range.start.line + 1,
                diagnostic.range.range.start.character + 1
            ),
        })
        .collect())
}

fn project(path: &Path) -> Result<String> {
    let target = resolve_target(Some(path))?;
    let compilation = compile_path(&target.entry)
        .with_context(|| format!("reading {}", target.entry.display()))?;

    let service = LanguageService::new();
    let checked = service.check_file(&target.entry)?;
    let diagnostics: Vec<EditorNote> = checked
        .diagnostics
        .iter()
        .map(|diagnostic| EditorNote {
            code: diagnostic.code.clone(),
            severity: match diagnostic.severity {
                Severity::Error => "error",
                Severity::Warning => "warning",
                Severity::Note => "note",
            },
            message: diagnostic.message.clone(),
            location: format!(
                "{}:{}:{}",
                diagnostic.range.file,
                diagnostic.range.range.start.line + 1,
                diagnostic.range.range.start.character + 1
            ),
        })
        .collect();

    let agents: Vec<AgentView> = compilation
        .agents
        .iter()
        .map(|agent| AgentView {
            name: agent.agent.clone(),
            inputs: fields(&agent.inputs),
            outputs: fields(&agent.outputs),
            effects: agent.effects.clone(),
            tools: agent
                .tools
                .iter()
                .map(|tool| ToolView {
                    name: tool.name.clone(),
                    reach: tool
                        .scopes
                        .iter()
                        .flat_map(|(effect, values)| {
                            values.iter().map(move |value| format!("{effect} {value}"))
                        })
                        .collect(),
                })
                .collect(),
            model: describe_model(&agent.requirements.model),
            steps: agent.nodes.len(),
        })
        .collect();

    let readiness = crate::doctor::report(&target, &compilation);

    let (plans, problems) =
        match crate::sandbox::plan_all(&compilation, &target.mcp(), &target.workspace()) {
            Ok(plans) => (
                plans
                    .into_values()
                    .map(|plan| PlanView {
                        server: plan.server.clone(),
                        agent: plan.agent.clone(),
                        enforced: plan.is_fully_enforced(),
                        rendered: ingot_sandbox::render(&plan),
                    })
                    .collect::<Vec<_>>(),
                Vec::new(),
            ),
            Err(problems) => (Vec::new(), problems),
        };

    Ok(serde_json::to_string(&json!({
        "schemaVersion": STUDIO_SCHEMA_VERSION,
        "path": target.root.display().to_string(),
        "entry": target.entry.display().to_string(),
        "workspace": target.workspace().display().to_string(),
        "outDir": target.out_dir.display().to_string(),
        "compiles": !compilation.has_errors(),
        "diagnostics": diagnostics,
        "agents": agents,
        "readiness": readiness,
        "boundary": { "plans": plans, "problems": problems },
        "runs": runs::count(&target.out_dir),
    }))?)
}

fn fields(map: &std::collections::BTreeMap<String, String>) -> Vec<Field> {
    map.iter()
        .map(|(name, ty)| Field {
            name: name.clone(),
            ty: ty.clone(),
        })
        .collect()
}

fn describe_model(requirement: &ingot_ir::ModelRequirement) -> String {
    match requirement {
        ingot_ir::ModelRequirement::Exact { reference } => reference.clone(),
        other => serde_json::to_value(other)
            .ok()
            .and_then(|value| {
                value
                    .get("mode")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_else(|| "unspecified".to_string()),
    }
}

// --- runs -------------------------------------------------------------------

fn run_detail(path: &Path, id: &str) -> Result<String> {
    let target = resolve_target(Some(path))?;
    Ok(serde_json::to_string(&runs::read(&target.out_dir, id)?)?)
}

// --- the machine -------------------------------------------------------------

fn machine() -> Result<String> {
    let providers: Vec<serde_json::Value> = crate::run::BUILT_IN
        .iter()
        .map(|built_in| {
            let variables: Vec<serde_json::Value> = built_in
                .variables
                .iter()
                .map(|variable| {
                    // Presence, by name. The value is never read, and there is
                    // nowhere in this document for one to land.
                    json!({ "name": variable, "set": std::env::var_os(variable).is_some() })
                })
                .collect();
            let set = variables
                .iter()
                .any(|variable| variable["set"].as_bool().unwrap_or(false));
            json!({
                "name": built_in.name,
                "protocol": built_in.protocol,
                "included": built_in.included,
                "declared": false,
                "variables": variables,
                "ready": built_in.included && set,
            })
        })
        .collect();

    let runtime = match ingot_sandbox::detect() {
        Ok(runtime) => json!({
            "available": true,
            "program": runtime.program,
            "version": runtime.version,
            "error": serde_json::Value::Null,
        }),
        Err(error) => json!({
            "available": false,
            "program": serde_json::Value::Null,
            "version": serde_json::Value::Null,
            "error": error.to_string(),
        }),
    };

    let detected = ingot_sandbox::detect().ok();
    let images: Vec<serde_json::Value> = [
        (
            crate::image::reference_image(),
            "contained runs happen inside this",
        ),
        (
            ingot_sandbox::DEFAULT_EGRESS_IMAGE.to_string(),
            "a contained server's traffic is filtered by this",
        ),
    ]
    .into_iter()
    .map(|(reference, purpose)| {
        let present = detected
            .as_ref()
            .and_then(|runtime| ingot_sandbox::image_exists(runtime, &reference).ok())
            .unwrap_or(false);
        json!({ "reference": reference, "purpose": purpose, "present": present })
    })
    .collect();

    Ok(serde_json::to_string(&json!({
        "schemaVersion": STUDIO_SCHEMA_VERSION,
        "version": env!("CARGO_PKG_VERSION"),
        "providers": providers,
        "runtime": runtime,
        "images": images,
    }))?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::run::BUILT_IN_PROVIDERS;

    #[test]
    fn the_machine_page_lists_every_vendor_that_needs_no_declaring() {
        // Two lists that must not drift: the one `ingot run` routes by and the
        // one the studio shows. A vendor missing here is a vendor a person is
        // told they do not have.
        let shown: Vec<&str> = crate::run::BUILT_IN
            .iter()
            .map(|built_in| built_in.name)
            .collect();
        assert_eq!(shown, BUILT_IN_PROVIDERS.to_vec());
    }

    #[test]
    fn a_bookmarked_path_is_one_a_person_would_recognise() {
        // `canonicalize` on Windows answers `\\?\C:\…`, which is correct and
        // looks broken. It reaches the bookmark file, a URL and the screen.
        let here = resolve(Path::new("."));
        assert!(here.is_absolute());
        assert!(
            !here.display().to_string().starts_with(r"\\?\"),
            "{}",
            here.display()
        );
    }

    #[test]
    fn a_verbatim_unc_path_is_left_alone() {
        // `\\?\UNC\server\share` is not `UNC\server\share`, so the prefix may
        // only come off a drive path. Nothing here resolves, so this exercises
        // the fallback arm of `resolve` on the shape it must not damage.
        let unc = Path::new(r"\\?\UNC\server\share\project");
        assert_eq!(resolve(unc), unc.to_path_buf());
    }

    #[test]
    fn the_bookmark_file_holds_paths_and_nothing_else() {
        // The list is a list. Anything else in it would be a fact about a
        // project kept somewhere other than the project.
        let directory = std::env::temp_dir().join(format!("ingot-studio-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::env::set_var("INGOT_CONFIG_DIR", &directory);

        save_bookmarks(&[PathBuf::from("/tmp/one"), PathBuf::from("/tmp/two")])
            .expect("the list must save");
        let text = std::fs::read_to_string(bookmarks_path().expect("a path"))
            .expect("the list must read back");
        let value: serde_json::Value = serde_json::from_str(&text).expect("valid JSON");
        assert_eq!(
            value
                .as_object()
                .expect("an object")
                .keys()
                .collect::<Vec<_>>(),
            vec!["projects", "schemaVersion"]
        );
        assert_eq!(load_bookmarks().len(), 2);

        std::env::remove_var("INGOT_CONFIG_DIR");
        let _ = std::fs::remove_dir_all(&directory);
    }
}
