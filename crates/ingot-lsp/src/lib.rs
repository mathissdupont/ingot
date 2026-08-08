//! Stdio LSP adapter for Ingot source.
//!
//! All parsing, checking and formatting is delegated to
//! `ingot-language-service`; this crate only speaks the Language Server
//! Protocol and caches open editor buffers.

use std::collections::BTreeMap;

use anyhow::{anyhow, Context};
use ingot_diagnostics::Severity;
use ingot_language_service::{
    EditorDiagnostic, EditorPosition, EditorRange, LanguageService, SourceRange,
};
use lsp_server::{
    Connection, ErrorCode, Message, Notification as ServerNotification, Request as ServerRequest,
    RequestId, Response,
};
use lsp_types::{notification::Notification as _, request::Request as _};
use lsp_types::{
    notification::{DidChangeTextDocument, DidOpenTextDocument, PublishDiagnostics},
    request::Formatting,
    Diagnostic, DiagnosticSeverity, DidChangeTextDocumentParams, DidOpenTextDocumentParams,
    DocumentFormattingParams, NumberOrString, OneOf, Position, PositionEncodingKind,
    PublishDiagnosticsParams, Range, ServerCapabilities, TextDocumentSyncKind, TextEdit, Uri,
};

const SOURCE: &str = "ingot";

pub fn run_stdio() -> anyhow::Result<()> {
    let (connection, io_threads) = Connection::stdio();
    let init_params = connection
        .initialize(serde_json::to_value(server_capabilities())?)
        .context("initializing LSP connection")?;
    let _ = init_params;

    main_loop(connection).context("running LSP event loop")?;
    io_threads.join()?;
    Ok(())
}

pub fn server_capabilities() -> ServerCapabilities {
    ServerCapabilities {
        position_encoding: Some(PositionEncodingKind::UTF16),
        text_document_sync: Some(TextDocumentSyncKind::FULL.into()),
        document_formatting_provider: Some(OneOf::Left(true)),
        ..Default::default()
    }
}

pub fn publish_diagnostics_for_text(
    uri: Uri,
    name: &str,
    text: &str,
    version: Option<i32>,
) -> PublishDiagnosticsParams {
    let service = LanguageService::new();
    let result = service.check_source(name, text);
    PublishDiagnosticsParams {
        uri: uri.clone(),
        diagnostics: result
            .diagnostics
            .iter()
            .map(|diagnostic| lsp_diagnostic(&uri, diagnostic))
            .collect(),
        version,
    }
}

pub fn format_text_document(name: &str, text: &str) -> Option<Vec<TextEdit>> {
    let service = LanguageService::new();
    let result = service.format_source(name, text);
    if !result.diagnostics.is_empty() || result.edits.is_empty() {
        return None;
    }

    Some(
        result
            .edits
            .into_iter()
            .map(|edit| TextEdit {
                range: lsp_range(edit.range.range),
                new_text: edit.new_text,
            })
            .collect(),
    )
}

fn main_loop(connection: Connection) -> anyhow::Result<()> {
    let mut state = ServerState::default();
    for message in &connection.receiver {
        match message {
            Message::Request(request) => {
                if connection
                    .handle_shutdown(&request)
                    .context("handling shutdown")?
                {
                    break;
                }
                handle_request(&connection, &mut state, request)?;
            }
            Message::Notification(notification) => {
                handle_notification(&connection, &mut state, notification)?;
            }
            Message::Response(_) => {}
        }
    }
    Ok(())
}

#[derive(Debug, Default)]
struct ServerState {
    documents: BTreeMap<Uri, OpenDocument>,
}

#[derive(Debug, Clone)]
struct OpenDocument {
    name: String,
    text: String,
    version: Option<i32>,
}

impl ServerState {
    fn open(&mut self, params: DidOpenTextDocumentParams) -> PublishDiagnosticsParams {
        let uri = params.text_document.uri;
        let document = OpenDocument {
            name: uri.as_str().to_string(),
            text: params.text_document.text,
            version: Some(params.text_document.version),
        };
        let diagnostics = publish_diagnostics_for_text(
            uri.clone(),
            &document.name,
            &document.text,
            document.version,
        );
        self.documents.insert(uri, document);
        diagnostics
    }

    fn change(&mut self, params: DidChangeTextDocumentParams) -> Option<PublishDiagnosticsParams> {
        let uri = params.text_document.uri;
        let text = params
            .content_changes
            .into_iter()
            .last()
            .map(|change| change.text)?;

        let document = self.documents.entry(uri.clone()).or_insert(OpenDocument {
            name: uri.as_str().to_string(),
            text: String::new(),
            version: None,
        });
        document.text = text;
        document.version = Some(params.text_document.version);

        Some(publish_diagnostics_for_text(
            uri,
            &document.name,
            &document.text,
            document.version,
        ))
    }

    fn format(&self, params: DocumentFormattingParams) -> anyhow::Result<Option<Vec<TextEdit>>> {
        let uri = params.text_document.uri;
        let document = self
            .documents
            .get(&uri)
            .ok_or_else(|| anyhow!("document is not open: {}", uri.as_str()))?;
        Ok(format_text_document(&document.name, &document.text))
    }
}

fn handle_notification(
    connection: &Connection,
    state: &mut ServerState,
    notification: ServerNotification,
) -> anyhow::Result<()> {
    match notification.method.as_str() {
        DidOpenTextDocument::METHOD => {
            let params: DidOpenTextDocumentParams =
                serde_json::from_value(notification.params).context("decoding didOpen params")?;
            publish(connection, state.open(params))?;
        }
        DidChangeTextDocument::METHOD => {
            let params: DidChangeTextDocumentParams =
                serde_json::from_value(notification.params).context("decoding didChange params")?;
            if let Some(diagnostics) = state.change(params) {
                publish(connection, diagnostics)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn handle_request(
    connection: &Connection,
    state: &mut ServerState,
    request: ServerRequest,
) -> anyhow::Result<()> {
    match request.method.as_str() {
        Formatting::METHOD => {
            let id = request.id;
            let params: DocumentFormattingParams =
                serde_json::from_value(request.params).context("decoding formatting params")?;
            let edits = state.format(params)?;
            send_ok(connection, id, &edits)?;
        }
        _ => send_err(
            connection,
            request.id,
            ErrorCode::MethodNotFound,
            "method is not implemented by ingot-lsp",
        )?,
    }
    Ok(())
}

fn publish(connection: &Connection, params: PublishDiagnosticsParams) -> anyhow::Result<()> {
    connection
        .sender
        .send(Message::Notification(ServerNotification::new(
            PublishDiagnostics::METHOD.to_string(),
            params,
        )))?;
    Ok(())
}

fn send_ok<T: serde::Serialize>(
    connection: &Connection,
    id: RequestId,
    result: &T,
) -> anyhow::Result<()> {
    connection.sender.send(Message::Response(Response::new_ok(
        id,
        serde_json::to_value(result)?,
    )))?;
    Ok(())
}

fn send_err(
    connection: &Connection,
    id: RequestId,
    code: ErrorCode,
    message: impl Into<String>,
) -> anyhow::Result<()> {
    connection.sender.send(Message::Response(Response::new_err(
        id,
        code as i32,
        message.into(),
    )))?;
    Ok(())
}

fn lsp_diagnostic(document_uri: &Uri, diagnostic: &EditorDiagnostic) -> Diagnostic {
    let mut message = diagnostic.message.clone();
    for note in &diagnostic.notes {
        message.push_str("\nnote: ");
        message.push_str(note);
    }
    if let Some(help) = &diagnostic.help {
        message.push_str("\nhelp: ");
        message.push_str(help);
    }

    Diagnostic {
        range: lsp_range(diagnostic.range.range),
        severity: Some(lsp_severity(diagnostic.severity)),
        code: Some(NumberOrString::String(diagnostic.code.clone())),
        code_description: None,
        source: Some(SOURCE.to_string()),
        message,
        related_information: related_information(document_uri, diagnostic),
        tags: None,
        data: Some(serde_json::json!({
            "schemaVersion": 1,
            "primarySpan": span_data(&diagnostic.range),
            "labels": diagnostic.labels.iter().map(|label| {
                serde_json::json!({
                    "primary": label.primary,
                    "message": label.message,
                    "span": span_data(&label.range),
                })
            }).collect::<Vec<_>>(),
        })),
    }
}

fn related_information(
    document_uri: &Uri,
    diagnostic: &EditorDiagnostic,
) -> Option<Vec<lsp_types::DiagnosticRelatedInformation>> {
    let related: Vec<_> = diagnostic
        .labels
        .iter()
        .filter(|label| !label.primary)
        .map(|label| lsp_types::DiagnosticRelatedInformation {
            location: lsp_types::Location {
                uri: label
                    .range
                    .file
                    .parse()
                    .unwrap_or_else(|_| document_uri.clone()),
                range: lsp_range(label.range.range),
            },
            message: label.message.clone(),
        })
        .collect();
    (!related.is_empty()).then_some(related)
}

fn lsp_severity(severity: Severity) -> DiagnosticSeverity {
    match severity {
        Severity::Error => DiagnosticSeverity::ERROR,
        Severity::Warning => DiagnosticSeverity::WARNING,
        Severity::Note => DiagnosticSeverity::INFORMATION,
    }
}

fn lsp_range(range: EditorRange) -> Range {
    Range::new(lsp_position(range.start), lsp_position(range.end))
}

fn lsp_position(position: EditorPosition) -> Position {
    Position::new(position.line, position.character)
}

fn span_data(range: &SourceRange) -> serde_json::Value {
    serde_json::json!({
        "file": range.file,
        "startByte": range.start_byte,
        "endByte": range.end_byte,
    })
}

#[cfg(test)]
mod tests {
    use std::{
        path::{Path, PathBuf},
        str::FromStr,
    };

    use ingot_language_service::LanguageService;
    use lsp_types::{DiagnosticSeverity, NumberOrString};

    use super::*;

    const BROKEN_SOURCE: &str = r#"language 0.1
package demo

agent Brief(topic: string) -> report<markdown> {
  flow {
    emit report = ask<markdown>("Write about ${topci}")
  }
}
"#;

    fn repo_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .expect("the crate must live two levels below the repository root")
            .to_path_buf()
    }

    fn uri(value: &str) -> Uri {
        Uri::from_str(value).expect("test URI must parse")
    }

    #[test]
    fn capabilities_advertise_full_sync_formatting_and_utf16_positions() {
        let capabilities = server_capabilities();
        assert_eq!(
            capabilities.position_encoding,
            Some(PositionEncodingKind::UTF16)
        );
        assert!(capabilities.text_document_sync.is_some());
        assert_eq!(
            capabilities.document_formatting_provider,
            Some(OneOf::Left(true))
        );
    }

    #[test]
    fn lsp_diagnostics_preserve_cli_code_span_and_message() {
        let document_uri = uri("file:///broken.ing");
        let lsp = publish_diagnostics_for_text(
            document_uri.clone(),
            document_uri.as_str(),
            BROKEN_SOURCE,
            Some(7),
        );
        let service = LanguageService::new().check_source(document_uri.as_str(), BROKEN_SOURCE);

        assert_eq!(lsp.uri, document_uri);
        assert_eq!(lsp.version, Some(7));
        assert_eq!(lsp.diagnostics.len(), service.diagnostics.len());

        let actual = &lsp.diagnostics[0];
        let expected = &service.diagnostics[0];
        assert_eq!(
            actual.code,
            Some(NumberOrString::String(expected.code.clone()))
        );
        assert_eq!(
            actual.message.lines().next(),
            Some(expected.message.as_str())
        );
        assert_eq!(actual.range, lsp_range(expected.range.range));
        assert_eq!(actual.severity, Some(DiagnosticSeverity::ERROR));
        assert_eq!(actual.source.as_deref(), Some("ingot"));

        let data = actual.data.as_ref().expect("span data must be carried");
        assert_eq!(
            data["primarySpan"]["startByte"].as_u64(),
            Some(expected.range.start_byte as u64)
        );
        assert_eq!(
            data["primarySpan"]["endByte"].as_u64(),
            Some(expected.range.end_byte as u64)
        );
    }

    #[test]
    fn formatting_returns_lsp_text_edits() {
        let source = r#"language 0.1
package demo

agent Brief(topic:string)->report<markdown>{flow{emit report=ask<markdown>("ok")}}
"#;

        let edits = format_text_document("file:///main.ing", source).expect("source is parseable");

        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].range.start, Position::new(0, 0));
        assert!(edits[0].new_text.contains("agent Brief(topic: string)"));
    }

    #[test]
    fn syntax_errors_return_no_format_edit() {
        assert!(format_text_document("file:///broken.ing", "language 0.1\nagent\n").is_none());
    }

    #[test]
    fn reference_examples_are_clean_through_the_lsp_surface() {
        for example in [
            "document-summarizer",
            "research-agent",
            "code-review-team",
            "repo-digest",
        ] {
            let entry = repo_root().join("examples").join(example).join("main.ing");
            let text = std::fs::read_to_string(&entry)
                .unwrap_or_else(|error| panic!("{} must be readable: {error}", entry.display()));
            let document_uri = uri(&format!("file:///examples/{example}/main.ing"));
            let diagnostics = publish_diagnostics_for_text(
                document_uri,
                &entry.display().to_string(),
                &text,
                None,
            );
            assert!(
                diagnostics
                    .diagnostics
                    .iter()
                    .all(|diagnostic| diagnostic.severity != Some(DiagnosticSeverity::ERROR)),
                "{example} should have no LSP errors"
            );
        }
    }
}
