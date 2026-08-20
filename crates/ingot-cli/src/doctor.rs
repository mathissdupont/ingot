//! Read-only project readiness for `ingot doctor`.
//!
//! This module deliberately stops at metadata: it compiles source, validates
//! declarations, checks environment-variable *presence*, and asks a container
//! daemon about its local image store. It never starts a provider, MCP server,
//! or container.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::Result;
use ingot_compiler::Compilation;
use ingot_ir::{ModelRequirement, NodeKind};
use ingot_runtime::{ModelConfig, ProviderKind};
use serde::Serialize;

use crate::manifest::{Target, MANIFEST_NAME};
use crate::run::{required_tools, BUILT_IN_PROVIDERS};

const REPORT_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Pass,
    Warn,
    Fail,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Check {
    id: String,
    status: Status,
    summary: String,
    location: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    fix: Option<String>,
}

impl Check {
    fn new(
        id: impl Into<String>,
        status: Status,
        summary: impl Into<String>,
        location: impl Into<String>,
        fix: Option<String>,
    ) -> Self {
        Self {
            id: id.into(),
            status,
            summary: summary.into(),
            location: location.into(),
            fix,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DoctorReport {
    pub schema_version: u32,
    pub ready: bool,
    pub source: PathBuf,
    pub manifest: Option<PathBuf>,
    pub checks: Vec<Check>,
}

/// Inspect every prerequisite that can be learned without executing the agent.
pub fn inspect(target: &Target, compilation: &Compilation, json: bool) -> Result<u8> {
    let report = report(target, compilation);

    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        render(&report);
    }

    Ok(if report.ready {
        crate::EXIT_OK
    } else {
        crate::EXIT_DIAGNOSTICS
    })
}

/// The same inspection as a value, for a caller that is not a terminal.
///
/// `ingot studio` shows this report rather than one of its own. Two readiness
/// answers about one project would be a second source of truth about whether it
/// is ready, which is exactly the thing a surface must not become.
pub fn report(target: &Target, compilation: &Compilation) -> DoctorReport {
    let manifest = target
        .manifest
        .as_ref()
        .map(|_| target.root.join(MANIFEST_NAME));
    let mut checks = Vec::new();

    checks.push(if compilation.has_errors() {
        Check::new(
            "source.compile",
            Status::Fail,
            format!(
                "source has {} error(s) and cannot produce a runnable artifact",
                compilation.error_count()
            ),
            target.entry.display().to_string(),
            Some("run `ingot check` and fix every reported diagnostic".to_string()),
        )
    } else {
        Check::new(
            "source.compile",
            Status::Pass,
            format!("{} agent(s) compile successfully", compilation.agents.len()),
            target.entry.display().to_string(),
            None,
        )
    });

    inspect_providers(target, compilation, &mut checks);
    inspect_tools(target, compilation, &mut checks);
    inspect_containment(target, &mut checks);

    DoctorReport {
        schema_version: REPORT_SCHEMA_VERSION,
        ready: !checks.iter().any(|check| check.status == Status::Fail),
        source: target.entry.clone(),
        manifest,
        checks,
    }
}

fn inspect_providers(target: &Target, compilation: &Compilation, checks: &mut Vec<Check>) {
    let models = target.model();
    let location = manifest_location(target);

    if let Err(reason) = models.validate(BUILT_IN_PROVIDERS) {
        checks.push(Check::new(
            "provider.config",
            Status::Fail,
            reason,
            location.clone(),
            Some("fix the `[model]` declarations in ingot.toml".to_string()),
        ));
        return;
    }

    checks.push(Check::new(
        "provider.config",
        Status::Pass,
        "model provider declarations are valid",
        location.clone(),
        None,
    ));

    let needs_model = compilation.agents.iter().any(|agent| {
        agent
            .nodes
            .iter()
            .any(|node| node.kind == NodeKind::LlmCall)
    });
    if !needs_model {
        checks.push(Check::new(
            "provider.route",
            Status::Pass,
            "the program makes no model calls",
            target.entry.display().to_string(),
            None,
        ));
        return;
    }

    let ready = provider_readiness(&models, checks, &location);
    let exact: BTreeSet<String> = compilation
        .agents
        .iter()
        .filter_map(|agent| match &agent.requirements.model {
            ModelRequirement::Exact { reference } => reference
                .split_once('/')
                .map(|(vendor, _)| vendor.to_string()),
            _ => None,
        })
        .collect();
    let needs_default = compilation.agents.iter().any(|agent| {
        !matches!(agent.requirements.model, ModelRequirement::Exact { .. })
            && agent
                .nodes
                .iter()
                .any(|node| node.kind == NodeKind::LlmCall)
    });

    for vendor in exact {
        if ready.get(&vendor).copied().unwrap_or(false) {
            checks.push(Check::new(
                format!("provider.route.{vendor}"),
                Status::Pass,
                format!("pinned model calls can route to `{vendor}`"),
                target.entry.display().to_string(),
                None,
            ));
        } else {
            checks.push(Check::new(
                format!("provider.route.{vendor}"),
                Status::Fail,
                format!("the artifact pins `{vendor}/...`, but `{vendor}` is not ready"),
                target.entry.display().to_string(),
                Some(format!(
                    "export the credential required by `{vendor}`, or declare that provider in {MANIFEST_NAME}"
                )),
            ));
        }
    }

    if needs_default {
        let candidates: Vec<&str> = ready
            .iter()
            .filter_map(|(name, is_ready)| is_ready.then_some(name.as_str()))
            .collect();
        let fallback = models
            .default
            .as_deref()
            .filter(|name| ready.get(*name).copied().unwrap_or(false))
            .or(match candidates.as_slice() {
                [only] => Some(*only),
                _ => None,
            });
        match fallback {
            Some(name) => checks.push(Check::new(
                "provider.default",
                Status::Pass,
                format!("unpinned model calls use `{name}`"),
                location,
                None,
            )),
            None if candidates.is_empty() => checks.push(Check::new(
                "provider.default",
                Status::Fail,
                "the program makes model calls, but no provider is ready",
                location,
                Some(
                    "export ANTHROPIC_API_KEY, OPENAI_API_KEY or GEMINI_API_KEY, or declare a reachable `[[model.provider]]`"
                        .to_string(),
                ),
            )),
            None => checks.push(Check::new(
                "provider.default",
                Status::Fail,
                format!(
                    "the artifact names no vendor and no default is selected; ready providers: {}",
                    candidates.join(", ")
                ),
                location,
                Some("set `[model] default = \"<provider>\"` or pin `model exact`".to_string()),
            )),
        }
    }
}

/// Provider label to whether `auto` has a credential name available. The value
/// itself is never interpreted, retained, formatted or serialized.
fn provider_readiness(
    models: &ModelConfig,
    checks: &mut Vec<Check>,
    manifest: &str,
) -> BTreeMap<String, bool> {
    let declared: BTreeSet<&str> = models
        .providers
        .iter()
        .map(|provider| provider.name.as_str())
        .collect();
    let mut ready = BTreeMap::new();

    // A vendor may answer to more than one variable name. Recognising only the
    // first would leave a configured machine told it has no provider. The table
    // is `crate::run::BUILT_IN` and not a copy of it, so `ingot studio` and this
    // cannot disagree about which vendors exist.
    for built_in in crate::run::BUILT_IN {
        let (name, variables, included) = (built_in.name, built_in.variables, built_in.included);
        if declared.contains(name) {
            continue;
        }
        let found = variables
            .iter()
            .copied()
            .find(|variable| credential_is_present(variable));
        let present = included && found.is_some();
        ready.insert(name.to_string(), present);

        let primary = variables[0];
        let alternatives = match variables.len() {
            1 => String::new(),
            _ => format!(" (or `{}`)", variables[1..].join("`, `")),
        };

        checks.push(if !included {
            Check::new(
                format!("provider.{name}"),
                Status::Warn,
                format!("this build does not include the `{name}` provider"),
                "ingot binary",
                Some(format!("rebuild ingot with the `{name}` feature")),
            )
        } else if let Some(variable) = found {
            Check::new(
                format!("provider.{name}"),
                Status::Pass,
                format!("`{variable}` is set (value hidden)"),
                format!("environment:{variable}"),
                None,
            )
        } else {
            Check::new(
                format!("provider.{name}"),
                Status::Warn,
                format!("`{primary}`{alternatives} is not set"),
                format!("environment:{primary}"),
                Some(format!(
                    "export `{primary}` to use the built-in `{name}` provider"
                )),
            )
        });
    }

    for provider in &models.providers {
        let protocol_included = match provider.kind {
            ProviderKind::Anthropic => cfg!(feature = "anthropic"),
            ProviderKind::Openai => cfg!(feature = "openai"),
            ProviderKind::Google => cfg!(feature = "google"),
        };
        let required_variable = match provider.api_key_env.as_deref() {
            None if provider.kind.requires_authentication() => {
                ready.insert(provider.name.clone(), false);
                checks.push(Check::new(
                    format!("provider.{}", provider.name),
                    Status::Fail,
                    format!(
                        "provider `{}` speaks the {} protocol, which authenticates every \
                         request, but declares no `api-key-env`",
                        provider.name,
                        provider.kind.as_str()
                    ),
                    manifest,
                    Some("add `api-key-env = \"VARIABLE_NAME\"` to this provider".to_string()),
                ));
                continue;
            }
            variable => variable,
        };
        let credential_ready = required_variable.map(credential_is_present).unwrap_or(true);
        let is_ready = protocol_included && credential_ready;
        ready.insert(provider.name.clone(), is_ready);

        let (status, summary, fix) = if !protocol_included {
            (
                Status::Fail,
                format!(
                    "provider `{}` needs the `{}` protocol, which this build omits",
                    provider.name,
                    provider.kind.as_str()
                ),
                Some(format!(
                    "rebuild ingot with the `{}` feature",
                    provider.kind.as_str()
                )),
            )
        } else if let Some(variable) = required_variable.filter(|_| !credential_ready) {
            (
                Status::Fail,
                format!(
                    "provider `{}` needs `{variable}`, which is not set",
                    provider.name
                ),
                Some(format!("export `{variable}` before running the agent")),
            )
        } else if let Some(variable) = required_variable {
            (
                Status::Pass,
                format!(
                    "provider `{}` is configured and `{variable}` is set (value hidden)",
                    provider.name
                ),
                None,
            )
        } else {
            (
                Status::Pass,
                format!(
                    "provider `{}` is configured without authentication",
                    provider.name
                ),
                None,
            )
        };
        checks.push(Check::new(
            format!("provider.{}", provider.name),
            status,
            summary,
            manifest,
            fix,
        ));
    }

    ready
}

fn credential_is_present(variable: &str) -> bool {
    std::env::var_os(variable).is_some()
}

fn inspect_tools(target: &Target, compilation: &Compilation, checks: &mut Vec<Check>) {
    let config = target.mcp();
    let location = manifest_location(target);
    let required = required_tools(compilation);

    if let Err(reason) = config.validate() {
        checks.push(Check::new(
            "tools.config",
            Status::Fail,
            reason,
            location,
            Some("fix the `[[mcp.server]]` declarations in ingot.toml".to_string()),
        ));
        return;
    }

    checks.push(Check::new(
        "tools.config",
        Status::Pass,
        format!("{} MCP server(s) are declared", config.servers.len()),
        location.clone(),
        None,
    ));

    for server in &config.servers {
        let server_location = format!("{location}:[[mcp.server]] name={}", server.name);
        let cwd = server
            .cwd
            .as_deref()
            .map(|cwd| target.root.join(cwd))
            .unwrap_or_else(|| target.root.clone());
        if !cwd.is_dir() {
            checks.push(Check::new(
                format!("tools.server.{}.cwd", server.name),
                Status::Fail,
                format!(
                    "MCP server `{}` working directory does not exist",
                    server.name
                ),
                cwd.display().to_string(),
                Some("create the directory or correct this server's `cwd`".to_string()),
            ));
        }
        if command_available(&server.command, &cwd) {
            checks.push(Check::new(
                format!("tools.server.{}.command", server.name),
                Status::Pass,
                format!("MCP command `{}` is available", server.command),
                server_location.clone(),
                None,
            ));
        } else {
            checks.push(Check::new(
                format!("tools.server.{}.command", server.name),
                Status::Fail,
                format!("MCP command `{}` is not available", server.command),
                server_location.clone(),
                Some("install the command or set `command` to an executable path".to_string()),
            ));
        }
        for variable in &server.pass_env {
            checks.push(if std::env::var_os(variable).is_some() {
                Check::new(
                    format!("tools.server.{}.env.{variable}", server.name),
                    Status::Pass,
                    format!("`{variable}` is set for MCP forwarding (value hidden)"),
                    format!("environment:{variable}"),
                    None,
                )
            } else {
                Check::new(
                    format!("tools.server.{}.env.{variable}", server.name),
                    Status::Warn,
                    format!("`{variable}` is not set for MCP forwarding"),
                    format!("environment:{variable}"),
                    Some(format!(
                        "export `{variable}` if `{}` requires it",
                        server.name
                    )),
                )
            });
        }
    }

    if required.is_empty() {
        checks.push(Check::new(
            "tools.routes",
            Status::Pass,
            "the program declares no MCP tools",
            target.entry.display().to_string(),
            None,
        ));
        return;
    }

    if config.servers.is_empty() {
        for tool in required {
            checks.push(Check::new(
                format!("tools.route.{tool}"),
                Status::Fail,
                format!("no MCP server is configured for `{tool}`"),
                location.clone(),
                Some(format!(
                    "add a `[[mcp.server]]` that publishes `{tool}` to {MANIFEST_NAME}"
                )),
            ));
        }
        return;
    }

    for tool in required {
        let explicit: Vec<_> = config
            .servers
            .iter()
            .filter_map(|server| {
                server
                    .tools
                    .get(&tool)
                    .map(|remote| (server.name.as_str(), remote.as_str()))
            })
            .collect();
        match explicit.as_slice() {
            [] => checks.push(Check::new(
                format!("tools.route.{tool}"),
                Status::Warn,
                format!(
                    "`{tool}` has no explicit route; publication will be verified by `ingot tools`"
                ),
                location.clone(),
                Some(format!(
                    "run `ingot tools`, or map `{tool}` under `[mcp.server.tools]`"
                )),
            )),
            [(server, remote)] => checks.push(Check::new(
                format!("tools.route.{tool}"),
                Status::Warn,
                format!(
                    "`{tool}` is declared as `{server}:{remote}`; publication is not started by doctor"
                ),
                location.clone(),
                Some("run `ingot tools` to verify the server publishes the remote name".to_string()),
            )),
            routes => checks.push(Check::new(
                format!("tools.route.{tool}"),
                Status::Fail,
                format!(
                    "`{tool}` has ambiguous explicit routes: {}",
                    routes
                        .iter()
                        .map(|(server, remote)| format!("{server}:{remote}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                location.clone(),
                Some("keep exactly one explicit route for this Ingot tool name".to_string()),
            )),
        }
    }
}

fn command_available(command: &str, cwd: &Path) -> bool {
    let candidate = Path::new(command);
    if candidate.components().count() > 1 || candidate.is_absolute() {
        return if candidate.is_absolute() {
            candidate.is_file()
        } else {
            cwd.join(candidate).is_file()
        };
    }

    let extensions: Vec<String> = if cfg!(windows) {
        std::env::var("PATHEXT")
            .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_string())
            .split(';')
            .map(|extension| extension.to_ascii_lowercase())
            .collect()
    } else {
        vec![String::new()]
    };
    std::env::var_os("PATH")
        .map(|path| {
            std::env::split_paths(&path).any(|directory| {
                if directory.join(command).is_file() {
                    return true;
                }
                extensions
                    .iter()
                    .any(|extension| directory.join(format!("{command}{extension}")).is_file())
            })
        })
        .unwrap_or(false)
}

fn inspect_containment(target: &Target, checks: &mut Vec<Check>) {
    let expected = crate::image::reference_image();
    let location = manifest_location(target);
    let configured = target.image();

    let runtime = match ingot_sandbox::detect() {
        Ok(runtime) => {
            checks.push(Check::new(
                "container.runtime",
                Status::Pass,
                format!("{} {} is available", runtime.program, runtime.version),
                "PATH",
                None,
            ));
            Some(runtime)
        }
        Err(error) => {
            // Which advice depends on which case it is, and the runtime layer
            // already separated them: a command that is not there and a daemon
            // that is not answering need different things done about them.
            // Telling somebody to install what they have installed is how a
            // report loses their trust — and on Windows the part that is
            // actually missing is usually not the installation but the switch
            // to Linux containers ([GAP-020](../../../docs/gaps.md#gap-020)).
            let fix = match &error {
                ingot_sandbox::ExecutorError::NoRuntime => {
                    if cfg!(windows) {
                        concat!(
                            "install Docker Desktop or Podman, then set it to Linux containers; ",
                            "a read-only root filesystem and `--network none` are ",
                            "Linux-container features, so a Windows-container daemon cannot ",
                            "express the boundary",
                        )
                    } else {
                        "install Docker or Podman, then run `ingot doctor` again"
                    }
                }
                _ => {
                    if cfg!(windows) {
                        concat!(
                            "start it — on Windows that means Docker Desktop running *and* ",
                            "switched to Linux containers rather than Windows containers",
                        )
                    } else {
                        "start it, then run `ingot doctor` again"
                    }
                }
            };
            checks.push(Check::new(
                "container.runtime",
                Status::Fail,
                error.to_string(),
                "PATH",
                Some(fix.to_string()),
            ));
            None
        }
    };

    match configured.as_deref() {
        None => {
            checks.push(Check::new(
                "container.configured-image",
                Status::Pass,
                format!("no custom image is configured; contained runs select `{expected}`"),
                location.clone(),
                None,
            ));
            inspect_image(
                "container.reference-image",
                &expected,
                runtime.as_ref(),
                &location,
                true,
                checks,
            );
        }
        Some(image) => {
            if image.starts_with("ingot/run:") && image != expected {
                checks.push(Check::new(
                    "container.image-version",
                    Status::Fail,
                    format!(
                        "configured reference image `{image}` does not match this binary's `{expected}`"
                    ),
                    location.clone(),
                    Some(format!(
                        "remove the override and run `ingot image build`, or set `[run] image = \"{expected}\"`"
                    )),
                ));
            }
            inspect_image(
                "container.configured-image",
                image,
                runtime.as_ref(),
                &location,
                false,
                checks,
            );
        }
    }
}

fn inspect_image(
    id: &str,
    image: &str,
    runtime: Option<&ingot_sandbox::Runtime>,
    location: &str,
    reference: bool,
    checks: &mut Vec<Check>,
) {
    let Some(runtime) = runtime else {
        checks.push(Check::new(
            id,
            Status::Warn,
            format!("image `{image}` cannot be inspected without a container runtime"),
            location,
            Some("fix `container.runtime`, then run `ingot doctor` again".to_string()),
        ));
        return;
    };

    match ingot_sandbox::image_exists(runtime, image) {
        Ok(true) => checks.push(Check::new(
            id,
            Status::Pass,
            format!("image `{image}` is present locally"),
            location,
            None,
        )),
        Ok(false) => checks.push(Check::new(
            id,
            Status::Fail,
            format!("image `{image}` is not present locally"),
            location,
            Some(if reference {
                "run `ingot image build`; verified remote acquisition is deferred until M6"
                    .to_string()
            } else {
                "build or acquire the configured image explicitly; Ingot will not pull it automatically"
                    .to_string()
            }),
        )),
        Err(error) => checks.push(Check::new(
            id,
            Status::Fail,
            error.to_string(),
            location,
            Some("restore the container daemon and run `ingot doctor` again".to_string()),
        )),
    }
}

fn manifest_location(target: &Target) -> String {
    target
        .manifest
        .as_ref()
        .map(|_| target.root.join(MANIFEST_NAME).display().to_string())
        .unwrap_or_else(|| "no manifest (loose source file)".to_string())
}

fn render(report: &DoctorReport) {
    println!("Ingot doctor");
    println!("source    {}", report.source.display());
    println!(
        "manifest  {}",
        report
            .manifest
            .as_deref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "(none)".to_string())
    );
    println!();

    for check in &report.checks {
        let label = match check.status {
            Status::Pass => "PASS",
            Status::Warn => "WARN",
            Status::Fail => "FAIL",
        };
        println!("{label:<4} {:<32} {}", check.id, check.summary);
        println!("     at {}", check.location);
        if let Some(fix) = &check.fix {
            println!("     fix: {fix}");
        }
    }

    let failed = report
        .checks
        .iter()
        .filter(|check| check.status == Status::Fail)
        .count();
    let warnings = report
        .checks
        .iter()
        .filter(|check| check.status == Status::Warn)
        .count();
    println!();
    if report.ready {
        println!("ready ({warnings} warning(s))");
    } else {
        println!("not ready: {failed} failed check(s), {warnings} warning(s)");
    }
}
