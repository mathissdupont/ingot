//! Live MCP discovery and source-schema preflight for `ingot tools`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use anyhow::Result;
use ingot_compiler::Compilation;
use ingot_ir::ToolSignature;
use ingot_mcp::{McpConfig, McpToolHost, ResolvedTool, ServerInfo, ToolDescriptor};
use serde::Serialize;
use serde_json::Value;

use crate::run::required_tools;

const SCHEMA_VERSION: u8 = 1;

pub struct ToolsConfig {
    pub root: PathBuf,
    pub mcp: McpConfig,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PreflightReport {
    schema_version: u8,
    ready: bool,
    required_environment: Vec<String>,
    servers: Vec<ServerReport>,
    declared_tools: Vec<DeclaredToolReport>,
    diagnostics: Vec<PreflightIssue>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ServerReport {
    manifest_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    server: Option<ServerInfo>,
    required_environment: Vec<String>,
    tools: Vec<ToolDescriptor>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DeclaredToolReport {
    name: String,
    signature: ToolSignature,
    #[serde(skip_serializing_if = "Option::is_none")]
    route: Option<RouteReport>,
    schema_compatibility: SchemaCompatibility,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RouteReport {
    server: String,
    remote: String,
    aliased: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SchemaCompatibility {
    status: CompatibilityStatus,
    issues: Vec<PreflightIssue>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum CompatibilityStatus {
    Match,
    Drift,
    Unverified,
    Unavailable,
}

#[derive(Debug, Clone, Serialize)]
struct PreflightIssue {
    code: &'static str,
    severity: IssueSeverity,
    message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
enum IssueSeverity {
    Error,
    Warning,
}

/// Discover configured MCP tools, compare their input schemas with the checked
/// source declarations, and render either the human view or the stable JSON
/// contract. This command starts only server processes already named by the
/// operator; it never installs software or reads credential values.
pub fn inspect(compilation: &Compilation, config: &ToolsConfig, json: bool) -> Result<u8> {
    let report = build_report(compilation, config);
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        render_human(&report);
    }
    Ok(if report.ready {
        super::EXIT_OK
    } else {
        super::EXIT_DIAGNOSTICS
    })
}

fn build_report(compilation: &Compilation, config: &ToolsConfig) -> PreflightReport {
    let declared = declared_tools(compilation);
    let required_environment = sorted_unique(
        config
            .mcp
            .servers
            .iter()
            .flat_map(|server| server.pass_env.iter().cloned()),
    );

    if config.mcp.is_empty() {
        let diagnostics = if declared.is_empty() {
            Vec::new()
        } else {
            vec![issue(
                "MCP_NO_SERVERS",
                IssueSeverity::Error,
                "no MCP server is configured",
            )]
        };
        let declared_tools = declared
            .into_iter()
            .map(|(name, signature)| unavailable(name, signature))
            .collect();
        return PreflightReport {
            schema_version: SCHEMA_VERSION,
            ready: diagnostics.is_empty(),
            required_environment,
            servers: Vec::new(),
            declared_tools,
            diagnostics,
        };
    }

    let mut host = match McpToolHost::connect_all(&config.mcp, &config.root) {
        Ok(host) => host,
        Err(error) => {
            let message = error.to_string();
            return PreflightReport {
                schema_version: SCHEMA_VERSION,
                ready: false,
                required_environment,
                servers: Vec::new(),
                declared_tools: declared
                    .into_iter()
                    .map(|(name, signature)| unavailable(name, signature))
                    .collect(),
                diagnostics: vec![issue("MCP_DISCOVERY_FAILED", IssueSeverity::Error, message)],
            };
        }
    };

    let inventory = host.inventory();
    let resolved = host
        .resolved()
        .into_iter()
        .map(|route| (route.tool.clone(), route))
        .collect::<BTreeMap<_, _>>();
    host.close();

    let servers = inventory
        .iter()
        .map(|(manifest_name, server, tools)| ServerReport {
            manifest_name: manifest_name.clone(),
            server: server.clone(),
            required_environment: config
                .mcp
                .servers
                .iter()
                .find(|configured| configured.name == *manifest_name)
                .map(|configured| sorted_unique(configured.pass_env.iter().cloned()))
                .unwrap_or_default(),
            tools: tools.clone(),
        })
        .collect();

    let declared_tools = declared
        .into_iter()
        .map(|(name, signature)| match resolved.get(&name) {
            Some(route) => {
                let descriptor = descriptor_for(&inventory, route);
                let compatibility = match descriptor {
                    Some(descriptor) => compare_input_schema(&signature, &descriptor.input_schema),
                    None => SchemaCompatibility {
                        status: CompatibilityStatus::Unverified,
                        issues: vec![issue(
                            "MCP_SCHEMA_UNAVAILABLE",
                            IssueSeverity::Warning,
                            "the routed tool did not include an input schema",
                        )],
                    },
                };
                DeclaredToolReport {
                    name,
                    signature,
                    route: Some(RouteReport {
                        server: route.server.clone(),
                        remote: route.remote.clone(),
                        aliased: route.aliased,
                    }),
                    schema_compatibility: compatibility,
                }
            }
            None => unavailable(name, signature),
        })
        .collect::<Vec<_>>();

    let ready = declared_tools.iter().all(|tool| {
        !matches!(
            tool.schema_compatibility.status,
            CompatibilityStatus::Drift | CompatibilityStatus::Unavailable
        )
    });

    PreflightReport {
        schema_version: SCHEMA_VERSION,
        ready,
        required_environment,
        servers,
        declared_tools,
        diagnostics: Vec::new(),
    }
}

fn declared_tools(compilation: &Compilation) -> BTreeMap<String, ToolSignature> {
    let required = required_tools(compilation);
    compilation
        .agents
        .iter()
        .flat_map(|agent| agent.tools.iter())
        .filter(|tool| required.contains(&tool.name))
        .map(|tool| (tool.name.clone(), tool.signature.clone()))
        .collect()
}

fn sorted_unique(values: impl Iterator<Item = String>) -> Vec<String> {
    values.collect::<BTreeSet<_>>().into_iter().collect()
}

fn descriptor_for<'a>(
    inventory: &'a [(String, Option<ServerInfo>, Vec<ToolDescriptor>)],
    route: &ResolvedTool,
) -> Option<&'a ToolDescriptor> {
    inventory
        .iter()
        .find(|(name, _, _)| name == &route.server)
        .and_then(|(_, _, tools)| tools.iter().find(|tool| tool.name == route.remote))
}

fn unavailable(name: String, signature: ToolSignature) -> DeclaredToolReport {
    DeclaredToolReport {
        name,
        signature,
        route: None,
        schema_compatibility: SchemaCompatibility {
            status: CompatibilityStatus::Unavailable,
            issues: vec![issue(
                "MCP_ROUTE_MISSING",
                IssueSeverity::Error,
                "no configured MCP server publishes or maps this tool",
            )],
        },
    }
}

fn compare_input_schema(signature: &ToolSignature, schema: &Value) -> SchemaCompatibility {
    let Some(object) = schema.as_object() else {
        return unverified(
            "MCP_SCHEMA_UNAVAILABLE",
            "the server published no object input schema",
        );
    };
    if object.get("type").and_then(Value::as_str) != Some("object") {
        return unverified(
            "MCP_SCHEMA_NOT_OBJECT",
            "the server input schema is not a plain JSON object schema",
        );
    }

    let properties = match object.get("properties") {
        Some(Value::Object(properties)) => Some(properties),
        Some(_) => {
            return unverified(
                "MCP_SCHEMA_PROPERTIES_UNSUPPORTED",
                "the server input schema has non-object `properties`",
            )
        }
        None => None,
    };
    let required = match object.get("required") {
        Some(Value::Array(names)) if names.iter().all(Value::is_string) => names
            .iter()
            .filter_map(Value::as_str)
            .collect::<BTreeSet<_>>(),
        Some(_) => {
            return unverified(
                "MCP_SCHEMA_REQUIRED_UNSUPPORTED",
                "the server input schema has a non-array `required` value",
            )
        }
        None => BTreeSet::new(),
    };
    let declared_names = signature
        .params
        .iter()
        .map(|param| param.name.as_str())
        .collect::<BTreeSet<_>>();
    let rejects_additional =
        object.get("additionalProperties").and_then(Value::as_bool) == Some(false);
    let mut issues = Vec::new();

    for remote in required.difference(&declared_names) {
        issues.push(issue(
            "MCP_SCHEMA_REQUIRED_PARAMETER_MISSING",
            IssueSeverity::Error,
            format!("server requires parameter `{remote}`, but the .ing declaration does not declare it"),
        ));
    }

    for param in &signature.params {
        let Some(param_schema) = properties.and_then(|properties| properties.get(&param.name))
        else {
            issues.push(issue(
                "MCP_SCHEMA_PARAMETER_UNDESCRIBED",
                if rejects_additional {
                    IssueSeverity::Error
                } else {
                    IssueSeverity::Warning
                },
                format!(
                    ".ing parameter `{}` is absent from the server schema",
                    param.name
                ),
            ));
            continue;
        };
        compare_type(&param.name, &param.ty, param_schema, &mut issues);
    }

    compatibility(issues)
}

fn compare_type(name: &str, declared: &str, schema: &Value, issues: &mut Vec<PreflightIssue>) {
    let Some(object) = schema.as_object() else {
        issues.push(issue(
            "MCP_SCHEMA_TYPE_UNVERIFIED",
            IssueSeverity::Warning,
            format!("parameter `{name}` has no inspectable JSON Schema type"),
        ));
        return;
    };
    let Some(remote) = object.get("type").and_then(Value::as_str) else {
        issues.push(issue(
            "MCP_SCHEMA_TYPE_UNVERIFIED",
            IssueSeverity::Warning,
            format!("parameter `{name}` uses a union, reference, or untyped schema that Language 0.1 cannot verify"),
        ));
        return;
    };

    let unsupported = [
        "$ref",
        "allOf",
        "anyOf",
        "oneOf",
        "not",
        "const",
        "enum",
        "pattern",
        "format",
        "minLength",
        "maxLength",
        "minimum",
        "maximum",
        "exclusiveMinimum",
        "exclusiveMaximum",
        "multipleOf",
        "minItems",
        "maxItems",
        "uniqueItems",
        "contains",
        "prefixItems",
    ]
    .into_iter()
    .filter(|keyword| object.contains_key(*keyword))
    .collect::<Vec<_>>();
    if !unsupported.is_empty() {
        issues.push(issue(
            "MCP_SCHEMA_CONSTRAINT_UNVERIFIED",
            IssueSeverity::Warning,
            format!(
                "parameter `{name}` uses schema constraint(s) this preflight does not verify: {}",
                unsupported.join(", ")
            ),
        ));
    }

    if let Some(element) = declared.strip_suffix("[]") {
        if remote != "array" {
            type_mismatch(name, declared, remote, issues);
            return;
        }
        match object.get("items") {
            Some(items) => compare_type(name, element, items, issues),
            None => issues.push(issue(
                "MCP_SCHEMA_TYPE_UNVERIFIED",
                IssueSeverity::Warning,
                format!("array parameter `{name}` has no `items` schema"),
            )),
        }
        return;
    }

    let compatible = match declared {
        "json" => {
            issues.push(issue(
                "MCP_SCHEMA_JSON_UNVERIFIED",
                IssueSeverity::Warning,
                format!("parameter `{name}` is unconstrained `json` in .ing but constrained by the server schema"),
            ));
            true
        }
        "string" | "text" | "markdown" | "bytes" | "file" => remote == "string",
        "int" => matches!(remote, "integer" | "number"),
        "float" => remote == "number",
        "bool" => remote == "boolean",
        // Any other checked type is a named Ingot record.
        _ => {
            if remote == "object" {
                issues.push(issue(
                    "MCP_SCHEMA_RECORD_FIELDS_UNVERIFIED",
                    IssueSeverity::Warning,
                    format!("record fields for parameter `{name}` are not compared in tools JSON version 1"),
                ));
                true
            } else {
                false
            }
        }
    };
    if !compatible {
        type_mismatch(name, declared, remote, issues);
    }
}

fn type_mismatch(name: &str, declared: &str, remote: &str, issues: &mut Vec<PreflightIssue>) {
    issues.push(issue(
        "MCP_SCHEMA_TYPE_MISMATCH",
        IssueSeverity::Error,
        format!("parameter `{name}` is `{declared}` in .ing but `{remote}` in the server schema"),
    ));
}

fn compatibility(issues: Vec<PreflightIssue>) -> SchemaCompatibility {
    let status = if issues
        .iter()
        .any(|problem| problem.severity == IssueSeverity::Error)
    {
        CompatibilityStatus::Drift
    } else if issues.is_empty() {
        CompatibilityStatus::Match
    } else {
        CompatibilityStatus::Unverified
    };
    SchemaCompatibility { status, issues }
}

fn unverified(code: &'static str, message: impl Into<String>) -> SchemaCompatibility {
    SchemaCompatibility {
        status: CompatibilityStatus::Unverified,
        issues: vec![issue(code, IssueSeverity::Warning, message)],
    }
}

fn issue(
    code: &'static str,
    severity: IssueSeverity,
    message: impl Into<String>,
) -> PreflightIssue {
    PreflightIssue {
        code,
        severity,
        message: message.into(),
    }
}

fn render_human(report: &PreflightReport) {
    if report.servers.is_empty() {
        if report.diagnostics.is_empty()
            || report
                .diagnostics
                .iter()
                .any(|problem| problem.code == "MCP_NO_SERVERS")
        {
            println!("no MCP server is configured");
        } else {
            println!("no MCP server is available");
        }
    }
    for server in &report.servers {
        match &server.server {
            Some(info) => println!(
                "{}  ({} {}, protocol {})",
                server.manifest_name, info.name, info.version, info.protocol_version
            ),
            None => println!("{}", server.manifest_name),
        }
        if !server.required_environment.is_empty() {
            println!("  environment: {}", server.required_environment.join(", "));
        }
        if server.tools.is_empty() {
            println!("  (publishes nothing)");
        }
        for tool in &server.tools {
            match &tool.description {
                Some(description) => println!("  {:<24}{}", tool.name, first_line(description)),
                None => println!("  {}", tool.name),
            }
        }
        println!();
    }

    if report.declared_tools.is_empty() {
        println!("this program declares no tools");
    } else {
        println!("declared tools");
        for tool in &report.declared_tools {
            match &tool.route {
                Some(route) => println!(
                    "  {:<24}-> {}:{}{}  [{}]",
                    tool.name,
                    route.server,
                    route.remote,
                    if route.aliased { "  (aliased)" } else { "" },
                    status_name(tool.schema_compatibility.status),
                ),
                None => println!("  {:<24}-> nothing serves it  [unavailable]", tool.name),
            }
            for problem in &tool.schema_compatibility.issues {
                println!(
                    "    {} {}: {}",
                    severity_name(problem.severity),
                    problem.code,
                    problem.message
                );
            }
        }
    }

    if report
        .diagnostics
        .iter()
        .any(|problem| problem.code == "MCP_NO_SERVERS")
    {
        println!();
        println!("add a server to {}:", super::MANIFEST_NAME);
        println!();
        println!("  [[mcp.server]]");
        println!("  name = \"files\"");
        println!("  command = \"ingot-mcp-fs\"");
        println!("  args = [\"--root\", \".\"]");
    }

    for problem in &report.diagnostics {
        eprintln!(
            "{} {}: {}",
            severity_name(problem.severity),
            problem.code,
            problem.message
        );
    }
    if !report.ready {
        eprintln!("hint: fix discovery, missing routes or schema drift before running this agent");
    }
}

fn status_name(status: CompatibilityStatus) -> &'static str {
    match status {
        CompatibilityStatus::Match => "schema match",
        CompatibilityStatus::Drift => "schema drift",
        CompatibilityStatus::Unverified => "schema unverified",
        CompatibilityStatus::Unavailable => "unavailable",
    }
}

fn severity_name(severity: IssueSeverity) -> &'static str {
    match severity {
        IssueSeverity::Error => "error",
        IssueSeverity::Warning => "warning",
    }
}

fn first_line(text: &str) -> &str {
    text.lines().next().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use ingot_ir::FieldType;
    use serde_json::json;

    use super::*;

    fn signature(params: &[(&str, &str)]) -> ToolSignature {
        ToolSignature {
            params: params
                .iter()
                .map(|(name, ty)| FieldType {
                    name: (*name).to_string(),
                    ty: (*ty).to_string(),
                })
                .collect(),
            result: "text".to_string(),
        }
    }

    #[test]
    fn matching_object_parameters_are_ready() {
        let result = compare_input_schema(
            &signature(&[("path", "string"), ("depth", "int")]),
            &json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "depth": {"type": "integer"},
                },
                "required": ["path", "depth"],
                "additionalProperties": false,
            }),
        );
        assert_eq!(result.status, CompatibilityStatus::Match);
        assert!(result.issues.is_empty());
    }

    #[test]
    fn a_required_remote_parameter_missing_from_source_is_drift() {
        let result = compare_input_schema(
            &signature(&[("path", "string")]),
            &json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "token": {"type": "string"},
                },
                "required": ["path", "token"],
            }),
        );
        assert_eq!(result.status, CompatibilityStatus::Drift);
        assert!(result
            .issues
            .iter()
            .any(|problem| problem.code == "MCP_SCHEMA_REQUIRED_PARAMETER_MISSING"));
    }

    #[test]
    fn a_type_mismatch_is_drift() {
        let result = compare_input_schema(
            &signature(&[("count", "int")]),
            &json!({
                "type": "object",
                "properties": {"count": {"type": "string"}},
                "required": ["count"],
            }),
        );
        assert_eq!(result.status, CompatibilityStatus::Drift);
        assert!(result
            .issues
            .iter()
            .any(|problem| problem.code == "MCP_SCHEMA_TYPE_MISMATCH"));
    }

    #[test]
    fn a_union_is_reported_as_unverified_not_compatible() {
        let result = compare_input_schema(
            &signature(&[("query", "string")]),
            &json!({
                "type": "object",
                "properties": {"query": {"type": ["string", "null"]}},
            }),
        );
        assert_eq!(result.status, CompatibilityStatus::Unverified);
    }

    #[test]
    fn an_unchecked_string_constraint_is_reported_as_unverified() {
        let result = compare_input_schema(
            &signature(&[("mode", "string")]),
            &json!({
                "type": "object",
                "properties": {"mode": {"type": "string", "enum": ["safe", "fast"]}},
            }),
        );
        assert_eq!(result.status, CompatibilityStatus::Unverified);
        assert!(result
            .issues
            .iter()
            .any(|problem| problem.code == "MCP_SCHEMA_CONSTRAINT_UNVERIFIED"));
    }
}
