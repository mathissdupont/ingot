//! Recursive-descent parser for the Ingot agent language.
//!
//! The parser never gives up on the first error. Every construct has a
//! synchronization point (the next item keyword, the next statement, or the
//! enclosing `}`), so a single run reports as many independent problems as the
//! source contains. Unparseable statements and expressions become explicit
//! error nodes, which keeps `ingot fmt` and the future language server usable on
//! half-written files.

use ingot_diagnostics::{codes, Diagnostic, DiagnosticBag};
use ingot_lexer::{tokenize, Keyword, Token, TokenKind};
use ingot_source::{SourceFile, Span};
use ingot_syntax::*;

mod strings;

/// Language versions this compiler implements.
pub const SUPPORTED_LANGUAGE_VERSIONS: &[(u32, u32)] = &[(0, 1), (0, 2)];

#[derive(Debug)]
pub struct ParseResult {
    pub program: Program,
    pub diagnostics: DiagnosticBag,
}

/// Tokenize and parse one source file.
pub fn parse(file: &SourceFile) -> ParseResult {
    let lexed = tokenize(file);
    let mut parser = Parser {
        text: file.text(),
        tokens: lexed.tokens,
        position: 0,
        diagnostics: lexed.diagnostics,
        eof_span: Span::empty_at(file.id(), file.text().len() as u32),
    };
    let program = parser.parse_program();
    ParseResult {
        program,
        diagnostics: parser.diagnostics,
    }
}

struct Parser<'a> {
    text: &'a str,
    tokens: Vec<Token>,
    position: usize,
    diagnostics: DiagnosticBag,
    eof_span: Span,
}

impl<'a> Parser<'a> {
    // --- cursor -----------------------------------------------------------

    fn peek(&self) -> &Token {
        &self.tokens[self.position.min(self.tokens.len() - 1)]
    }

    fn peek_at(&self, ahead: usize) -> &Token {
        &self.tokens[(self.position + ahead).min(self.tokens.len() - 1)]
    }

    fn span(&self) -> Span {
        self.peek().span
    }

    fn at_eof(&self) -> bool {
        matches!(self.peek().kind, TokenKind::Eof)
    }

    fn at(&self, kind: &TokenKind) -> bool {
        &self.peek().kind == kind
    }

    fn at_keyword(&self, keyword: Keyword) -> bool {
        self.peek().keyword() == Some(keyword)
    }

    fn bump(&mut self) -> Token {
        let token = self.peek().clone();
        if !self.at_eof() {
            self.position += 1;
        }
        token
    }

    fn eat(&mut self, kind: &TokenKind) -> bool {
        if self.at(kind) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn eat_keyword(&mut self, keyword: Keyword) -> bool {
        if self.at_keyword(keyword) {
            self.bump();
            true
        } else {
            false
        }
    }

    /// Consume `kind` or report "expected X, found Y" without consuming.
    fn expect(&mut self, kind: TokenKind) -> bool {
        if self.at(&kind) {
            self.bump();
            return true;
        }
        let found = self.peek().kind.describe();
        let span = self.span();
        self.error(
            Diagnostic::error(
                codes::EXPECTED_TOKEN,
                format!("expected {}, found {found}", kind.describe()),
            )
            .with_primary(span, format!("expected {}", kind.describe())),
        );
        false
    }

    fn expect_keyword(&mut self, keyword: Keyword) -> bool {
        if self.eat_keyword(keyword) {
            return true;
        }
        let found = self.peek().kind.describe();
        let span = self.span();
        self.error(
            Diagnostic::error(
                codes::EXPECTED_TOKEN,
                format!("expected keyword `{}`, found {found}", keyword.as_str()),
            )
            .with_primary(span, format!("expected `{}`", keyword.as_str())),
        );
        false
    }

    fn error(&mut self, diagnostic: Diagnostic) {
        self.diagnostics.push(diagnostic);
    }

    /// A hard stop for every delimiter-bounded loop.
    ///
    /// Without it, a missing `)` or `}` lets a recovery loop swallow the rest of
    /// the file and hide every later declaration. Declaration keywords can only
    /// begin a top-level item, so they are safe anchors.
    fn at_barrier(&self) -> bool {
        self.at_eof()
            || matches!(
                self.peek().keyword(),
                Some(Keyword::Type)
                    | Some(Keyword::Import)
                    | Some(Keyword::Tool)
                    | Some(Keyword::Verifier)
                    | Some(Keyword::Agent)
            )
    }

    /// A plain identifier.
    fn expect_ident(&mut self, context: &str) -> Option<Ident> {
        if let TokenKind::Ident(text) = &self.peek().kind {
            let ident = Ident::new(text.clone(), self.span());
            self.bump();
            return Some(ident);
        }
        let found = self.peek().kind.describe();
        let span = self.span();
        self.error(
            Diagnostic::error(
                codes::EXPECTED_TOKEN,
                format!("expected {context}, found {found}"),
            )
            .with_primary(span, format!("expected {context}")),
        );
        None
    }

    /// An identifier, accepting reserved words spelled in a position where they
    /// are unambiguous (policy subjects, budget keys, transports, lifetimes).
    fn expect_ident_like(&mut self, context: &str) -> Option<Ident> {
        match &self.peek().kind {
            TokenKind::Ident(text) => {
                let ident = Ident::new(text.clone(), self.span());
                self.bump();
                Some(ident)
            }
            TokenKind::Keyword(keyword) => {
                let ident = Ident::new(keyword.as_str(), self.span());
                self.bump();
                Some(ident)
            }
            _ => {
                let found = self.peek().kind.describe();
                let span = self.span();
                self.error(
                    Diagnostic::error(
                        codes::EXPECTED_TOKEN,
                        format!("expected {context}, found {found}"),
                    )
                    .with_primary(span, format!("expected {context}")),
                );
                None
            }
        }
    }

    // --- program ----------------------------------------------------------

    fn parse_program(&mut self) -> Program {
        let start = self.span();
        let language = self.parse_language_decl();
        let package = self.parse_package_decl();

        let mut program = Program {
            language,
            package,
            imports: Vec::new(),
            types: Vec::new(),
            tools: Vec::new(),
            verifiers: Vec::new(),
            agents: Vec::new(),
            span: start,
        };

        while self.at_keyword(Keyword::Import) {
            if let Some(decl) = self.parse_import_decl() {
                program.imports.push(decl);
            }
        }
        if !program.imports.is_empty()
            && !matches!(
                program.language,
                Some(LanguageVersion {
                    major: 0,
                    minor: 2..,
                    ..
                })
            )
        {
            let span = program.imports[0].span;
            self.error(
                Diagnostic::error(
                    codes::UNSUPPORTED_LANGUAGE_VERSION,
                    "`import` requires language 0.2 or newer",
                )
                .with_primary(span, "not available in this language version")
                .with_help("change the file header to `language 0.2`"),
            );
        }

        while !self.at_eof() {
            let before = self.position;
            match self.peek().keyword() {
                Some(Keyword::Type) => {
                    if let Some(decl) = self.parse_type_decl() {
                        program.types.push(decl);
                    }
                }
                Some(Keyword::Tool) => {
                    if let Some(decl) = self.parse_tool_decl() {
                        program.tools.push(decl);
                    }
                }
                Some(Keyword::Verifier) => {
                    if let Some(decl) = self.parse_verifier_decl() {
                        program.verifiers.push(decl);
                    }
                }
                Some(Keyword::Agent) => {
                    if let Some(decl) = self.parse_agent_decl() {
                        program.agents.push(decl);
                    }
                }
                Some(Keyword::Language) => {
                    let span = self.span();
                    self.error(
                        Diagnostic::error(
                            codes::UNEXPECTED_TOKEN,
                            "`language` must be the first declaration in the file",
                        )
                        .with_primary(span, "move this to the top of the file"),
                    );
                    self.recover_to_item();
                }
                Some(Keyword::Package) => {
                    let span = self.span();
                    self.error(
                        Diagnostic::error(
                            codes::UNEXPECTED_TOKEN,
                            "`package` must appear once, directly after `language`",
                        )
                        .with_primary(span, "unexpected second package declaration"),
                    );
                    self.recover_to_item();
                }
                Some(Keyword::Import) => {
                    let span = self.span();
                    self.error(
                        Diagnostic::error(
                            codes::UNEXPECTED_TOKEN,
                            "`import` must appear after `package` and before declarations",
                        )
                        .with_primary(span, "move this import above the declarations"),
                    );
                    self.recover_to_item();
                }
                _ => {
                    let found = self.peek().kind.describe();
                    let span = self.span();
                    self.error(
                        Diagnostic::error(
                            codes::UNEXPECTED_TOKEN,
                            format!("expected a declaration, found {found}"),
                        )
                        .with_primary(
                            span,
                            "expected `import`, `type`, `tool`, `verifier` or `agent`",
                        ),
                    );
                    self.recover_to_item();
                }
            }
            // Guarantee forward progress even if a sub-parser bailed immediately.
            if self.position == before {
                self.bump();
            }
        }

        if !language_supports_v0_2_types(program.language) {
            if let Some(span) = first_v0_2_type_span(&program) {
                self.error(
                    Diagnostic::error(
                        codes::UNSUPPORTED_LANGUAGE_VERSION,
                        "optional and union types require language 0.2 or newer",
                    )
                    .with_primary(span, "not available in this language version")
                    .with_help("change the file header to `language 0.2`"),
                );
            }
        }

        program.span = program.span.merge(self.eof_span);
        program
    }

    /// Skip tokens until the next declaration keyword or end of file.
    fn recover_to_item(&mut self) {
        while !self.at_eof() {
            if matches!(
                self.peek().keyword(),
                Some(Keyword::Type)
                    | Some(Keyword::Import)
                    | Some(Keyword::Tool)
                    | Some(Keyword::Verifier)
                    | Some(Keyword::Agent)
            ) {
                return;
            }
            self.bump();
        }
    }

    fn parse_language_decl(&mut self) -> Option<LanguageVersion> {
        if !self.at_keyword(Keyword::Language) {
            let span = self.span();
            self.error(
                Diagnostic::error(
                    codes::MISSING_LANGUAGE_DECLARATION,
                    "missing `language` declaration",
                )
                .with_primary(span, "expected `language 0.1` here")
                .with_help("every Ingot source file pins the language version it was written for"),
            );
            return None;
        }
        let keyword_span = self.span();
        self.bump();

        // The lexer reads `0.1` as a float; recover the exact digits from source
        // so that `0.10` and `0.1` stay distinguishable.
        let token_span = self.span();
        let raw = self.raw_text(token_span).to_string();
        let is_number = matches!(self.peek().kind, TokenKind::Float(_) | TokenKind::Int(_));
        if !is_number {
            let found = self.peek().kind.describe();
            self.error(
                Diagnostic::error(
                    codes::EXPECTED_TOKEN,
                    format!("expected a language version such as `0.1`, found {found}"),
                )
                .with_primary(token_span, "expected MAJOR.MINOR"),
            );
            return None;
        }
        self.bump();

        let span = keyword_span.merge(token_span);
        let (major, minor) = match raw.split_once('.') {
            Some((major, minor)) => (major.parse::<u32>().ok(), minor.parse::<u32>().ok()),
            None => (raw.parse::<u32>().ok(), Some(0)),
        };
        let (Some(major), Some(minor)) = (major, minor) else {
            self.error(
                Diagnostic::error(
                    codes::UNSUPPORTED_LANGUAGE_VERSION,
                    format!("`{raw}` is not a valid language version"),
                )
                .with_primary(span, "expected MAJOR.MINOR"),
            );
            return None;
        };

        if !SUPPORTED_LANGUAGE_VERSIONS.contains(&(major, minor)) {
            let supported = SUPPORTED_LANGUAGE_VERSIONS
                .iter()
                .map(|(major, minor)| format!("{major}.{minor}"))
                .collect::<Vec<_>>()
                .join(", ");
            self.error(
                Diagnostic::error(
                    codes::UNSUPPORTED_LANGUAGE_VERSION,
                    format!("unsupported language version `{major}.{minor}`"),
                )
                .with_primary(span, "this compiler cannot guarantee these semantics")
                .with_note(format!("supported versions: {supported}")),
            );
        }

        Some(LanguageVersion { major, minor, span })
    }

    fn parse_package_decl(&mut self) -> Option<DottedName> {
        if !self.eat_keyword(Keyword::Package) {
            return None;
        }
        self.parse_dotted_name("a package name")
    }

    fn parse_dotted_name(&mut self, context: &str) -> Option<DottedName> {
        let first = self.expect_ident(context)?;
        let mut span = first.span;
        let mut parts = vec![first];
        while self.at(&TokenKind::Dot) {
            self.bump();
            let Some(part) = self.expect_ident("a name segment") else {
                break;
            };
            span = span.merge(part.span);
            parts.push(part);
        }
        Some(DottedName { parts, span })
    }

    fn raw_text(&self, span: Span) -> &'a str {
        let start = (span.start as usize).min(self.text.len());
        let end = (span.end as usize).min(self.text.len());
        &self.text[start..end]
    }

    // --- declarations -----------------------------------------------------

    fn parse_import_decl(&mut self) -> Option<ImportDecl> {
        let start = self.span();
        self.bump(); // `import`

        let path_span = self.span();
        let TokenKind::Str(raw) = self.peek().kind.clone() else {
            let found = self.peek().kind.describe();
            self.error(
                Diagnostic::error(
                    codes::EXPECTED_TOKEN,
                    format!("expected an import path, found {found}"),
                )
                .with_primary(path_span, "expected a string literal path"),
            );
            self.recover_to_item();
            return None;
        };
        self.bump();
        let path = self.build_string_literal(&raw, path_span);
        if !path.is_plain() {
            self.error(
                Diagnostic::error(
                    codes::UNEXPECTED_TOKEN,
                    "import paths must be plain string literals",
                )
                .with_primary(path_span, "interpolation is not allowed here"),
            );
        }

        if !self.expect(TokenKind::LBrace) {
            self.recover_to_item();
            return Some(ImportDecl {
                path,
                items: Vec::new(),
                span: start.merge(self.previous_span()),
            });
        }

        let mut items = Vec::new();
        while !self.at(&TokenKind::RBrace) && !self.at_eof() {
            let before = self.position;
            if let Some(item) = self.parse_import_item() {
                items.push(item);
            }
            self.eat(&TokenKind::Comma);
            if self.position == before {
                self.bump();
            }
        }
        let end = self.span();
        self.expect(TokenKind::RBrace);
        Some(ImportDecl {
            path,
            items,
            span: start.merge(end),
        })
    }

    fn parse_import_item(&mut self) -> Option<ImportItem> {
        let start = self.span();
        let kind = match self.peek().keyword() {
            Some(Keyword::Type) => {
                self.bump();
                ImportKind::Type
            }
            Some(Keyword::Tool) => {
                self.bump();
                ImportKind::Tool
            }
            Some(Keyword::Verifier) => {
                self.bump();
                ImportKind::Verifier
            }
            _ => {
                let found = self.peek().kind.describe();
                self.error(
                    Diagnostic::error(
                        codes::UNEXPECTED_TOKEN,
                        format!("expected an import item, found {found}"),
                    )
                    .with_primary(start, "expected `type`, `tool` or `verifier`"),
                );
                self.recover_in_import_block();
                return None;
            }
        };

        let name = match kind {
            ImportKind::Tool => self.parse_dotted_name("a tool name")?,
            ImportKind::Type => {
                let ident = self.expect_ident("a type name")?;
                DottedName {
                    span: ident.span,
                    parts: vec![ident],
                }
            }
            ImportKind::Verifier => {
                let ident = self.expect_ident("a verifier name")?;
                DottedName {
                    span: ident.span,
                    parts: vec![ident],
                }
            }
        };

        Some(ImportItem {
            kind,
            span: start.merge(name.span),
            name,
        })
    }

    fn recover_in_import_block(&mut self) {
        while !self.at_eof() && !self.at(&TokenKind::RBrace) {
            if matches!(
                self.peek().keyword(),
                Some(Keyword::Type) | Some(Keyword::Tool) | Some(Keyword::Verifier)
            ) {
                return;
            }
            self.bump();
        }
    }

    fn parse_type_decl(&mut self) -> Option<TypeDecl> {
        let doc = self.peek().doc.clone();
        let start = self.span();
        self.bump(); // `type`
        let name = self.expect_ident("a type name")?;
        let fields = self.parse_field_block()?;
        let span = start.merge(self.previous_span());
        Some(TypeDecl {
            name,
            fields,
            doc,
            span,
        })
    }

    fn previous_span(&self) -> Span {
        if self.position == 0 {
            self.eof_span
        } else {
            self.tokens[self.position - 1].span
        }
    }

    /// `{ name: type  name: type }` with optional separating commas.
    fn parse_field_block(&mut self) -> Option<Vec<FieldDecl>> {
        if !self.expect(TokenKind::LBrace) {
            return None;
        }
        let mut fields = Vec::new();
        while !self.at(&TokenKind::RBrace) && !self.at_barrier() {
            let before = self.position;
            if let Some(field) = self.parse_field() {
                fields.push(field);
            }
            self.eat(&TokenKind::Comma);
            if self.position == before {
                self.bump();
            }
        }
        self.expect(TokenKind::RBrace);
        Some(fields)
    }

    fn parse_field(&mut self) -> Option<FieldDecl> {
        let name = self.expect_ident("a field name")?;
        self.expect(TokenKind::Colon);
        let ty = self.parse_type()?;
        let span = name.span.merge(ty.span());
        Some(FieldDecl { name, ty, span })
    }

    fn parse_type(&mut self) -> Option<TypeExpr> {
        let first = self.parse_type_postfix()?;
        let mut span = first.span();
        let mut options = vec![first];
        while self.at(&TokenKind::Pipe) {
            self.bump();
            let Some(option) = self.parse_type_postfix() else {
                break;
            };
            span = span.merge(option.span());
            options.push(option);
        }
        if options.len() == 1 {
            options.pop()
        } else {
            Some(TypeExpr::Union { options, span })
        }
    }

    fn parse_type_postfix(&mut self) -> Option<TypeExpr> {
        let mut ty = self.parse_type_primary()?;
        loop {
            if self.at(&TokenKind::LBracket) {
                let start = self.span();
                self.bump();
                let end = self.span();
                self.expect(TokenKind::RBracket);
                ty = TypeExpr::List {
                    span: ty.span().merge(start).merge(end),
                    element: Box::new(ty),
                };
            } else if self.at(&TokenKind::Question) {
                let question = self.span();
                self.bump();
                ty = TypeExpr::Optional {
                    span: ty.span().merge(question),
                    inner: Box::new(ty),
                };
            } else {
                return Some(ty);
            }
        }
    }

    fn parse_type_primary(&mut self) -> Option<TypeExpr> {
        if self.eat(&TokenKind::LParen) {
            let inner = self.parse_type();
            self.expect(TokenKind::RParen);
            return inner;
        }
        self.expect_ident("a type").map(TypeExpr::Named)
    }

    fn parse_params(&mut self) -> Vec<FieldDecl> {
        let mut params = Vec::new();
        if !self.expect(TokenKind::LParen) {
            return params;
        }
        while !self.at(&TokenKind::RParen) && !self.at_barrier() {
            let before = self.position;
            if let Some(param) = self.parse_field() {
                params.push(param);
            }
            if !self.eat(&TokenKind::Comma) && !self.at(&TokenKind::RParen) {
                // Missing comma: report once and continue with the next parameter.
                if self.position != before {
                    let span = self.span();
                    self.error(
                        Diagnostic::error(codes::EXPECTED_TOKEN, "expected `,` between parameters")
                            .with_primary(span, "expected `,` or `)`"),
                    );
                }
            }
            if self.position == before {
                self.bump();
            }
        }
        self.expect(TokenKind::RParen);
        params
    }

    fn parse_tool_decl(&mut self) -> Option<ToolDecl> {
        let doc = self.peek().doc.clone();
        let start = self.span();
        self.bump(); // `tool`
        let name = self.parse_dotted_name("a tool name")?;
        let params = self.parse_params();
        self.expect(TokenKind::Arrow);
        let ret = self.parse_type()?;

        let mut effects = Vec::new();
        while self.at(&TokenKind::Bang) {
            self.bump();
            let Some(effect) = self.expect_ident_like("an effect name") else {
                break;
            };
            effects.push(effect);
        }

        let span = start.merge(self.previous_span());
        Some(ToolDecl {
            name,
            params,
            ret,
            effects,
            doc,
            span,
        })
    }

    fn parse_verifier_decl(&mut self) -> Option<VerifierDecl> {
        let doc = self.peek().doc.clone();
        let start = self.span();
        self.bump(); // `verifier`
        let name = self.expect_ident("a verifier name")?;
        let params = self.parse_params();
        let span = start.merge(self.previous_span());
        Some(VerifierDecl {
            name,
            params,
            doc,
            span,
        })
    }

    fn parse_agent_decl(&mut self) -> Option<AgentDecl> {
        let doc = self.peek().doc.clone();
        let start = self.span();
        self.bump(); // `agent`
        let name = self.expect_ident("an agent name")?;
        let params = self.parse_params();

        let output = if self.eat(&TokenKind::Arrow) {
            self.parse_output_decl()
        } else {
            None
        };

        let mut agent = AgentDecl {
            name,
            params,
            output,
            model: None,
            tools: None,
            memory: None,
            budget: None,
            policy: None,
            flow: None,
            doc,
            span: start,
        };

        if !self.expect(TokenKind::LBrace) {
            self.recover_to_item();
            agent.span = start.merge(self.previous_span());
            return Some(agent);
        }

        while !self.at(&TokenKind::RBrace) && !self.at_barrier() {
            let before = self.position;
            match self.peek().keyword() {
                Some(Keyword::Model) => {
                    let block = self.parse_model_block();
                    self.set_section(&mut agent.model, block, "model");
                }
                Some(Keyword::Tools) => {
                    let block = self.parse_tools_block();
                    self.set_section(&mut agent.tools, block, "tools");
                }
                Some(Keyword::Memory) => {
                    let block = self.parse_memory_block();
                    self.set_section(&mut agent.memory, block, "memory");
                }
                Some(Keyword::Budget) => {
                    let block = self.parse_budget_block();
                    self.set_section(&mut agent.budget, block, "budget");
                }
                Some(Keyword::Policy) => {
                    let block = self.parse_policy_block();
                    self.set_section(&mut agent.policy, block, "policy");
                }
                Some(Keyword::Flow) => {
                    let block = self.parse_flow_block();
                    self.set_section(&mut agent.flow, block, "flow");
                }
                _ => {
                    let found = self.peek().kind.describe();
                    let span = self.span();
                    self.error(
                        Diagnostic::error(
                            codes::UNEXPECTED_TOKEN,
                            format!("expected an agent section, found {found}"),
                        )
                        .with_primary(
                            span,
                            "expected `model`, `tools`, `memory`, `budget`, `policy` or `flow`",
                        ),
                    );
                    self.recover_to_agent_section();
                }
            }
            if self.position == before {
                self.bump();
            }
        }
        self.expect(TokenKind::RBrace);

        agent.span = start.merge(self.previous_span());
        Some(agent)
    }

    /// Store a section, reporting a duplicate instead of silently overwriting.
    fn set_section<T>(&mut self, slot: &mut Option<T>, parsed: Option<T>, name: &str)
    where
        T: HasSpan,
    {
        let Some(parsed) = parsed else { return };
        match slot {
            Some(existing) => {
                let first = existing.span();
                self.error(
                    Diagnostic::error(
                        codes::DUPLICATE_SECTION,
                        format!("duplicate `{name}` section"),
                    )
                    .with_primary(parsed.span(), "second declaration")
                    .with_secondary(first, "first declared here")
                    .with_help(format!("an agent may declare `{name}` at most once")),
                );
            }
            None => *slot = Some(parsed),
        }
    }

    fn recover_to_agent_section(&mut self) {
        while !self.at_eof() && !self.at(&TokenKind::RBrace) {
            if matches!(
                self.peek().keyword(),
                Some(Keyword::Model)
                    | Some(Keyword::Tools)
                    | Some(Keyword::Memory)
                    | Some(Keyword::Budget)
                    | Some(Keyword::Policy)
                    | Some(Keyword::Flow)
            ) {
                return;
            }
            self.bump();
        }
    }

    fn parse_output_decl(&mut self) -> Option<OutputDecl> {
        let name = self.expect_ident("an output name")?;
        self.expect(TokenKind::Lt);
        let content = self.expect_ident_like("an artifact content type")?;
        let end = self.span();
        self.expect(TokenKind::Gt);
        Some(OutputDecl {
            span: name.span.merge(end),
            name,
            content,
        })
    }

    // --- agent sections ---------------------------------------------------

    fn parse_model_block(&mut self) -> Option<ModelBlock> {
        let start = self.span();
        self.bump(); // `model`

        if self.eat_keyword(Keyword::Exact) {
            let reference_span = self.span();
            let TokenKind::Str(raw) = self.peek().kind.clone() else {
                let found = self.peek().kind.describe();
                self.error(
                    Diagnostic::error(
                        codes::EXPECTED_TOKEN,
                        format!("expected a model reference string, found {found}"),
                    )
                    .with_primary(reference_span, "expected something like \"openai/gpt-5\""),
                );
                return None;
            };
            self.bump();
            return Some(ModelBlock::Exact {
                reference: strings::unescape(&raw),
                reference_span,
                span: start.merge(reference_span),
            });
        }

        self.expect_keyword(Keyword::Requires);
        if !self.expect(TokenKind::LBrace) {
            return None;
        }

        let mut requirements = Vec::new();
        while !self.at(&TokenKind::RBrace) && !self.at_barrier() {
            let before = self.position;
            let Some(name) = self.expect_ident_like("a model capability") else {
                self.bump();
                continue;
            };
            if self.at(&TokenKind::Ge) {
                self.bump();
                let value_span = self.span();
                match self.peek().kind {
                    TokenKind::Int(tokens) => {
                        self.bump();
                        requirements.push(ModelRequirement::ContextAtLeast {
                            tokens,
                            span: name.span.merge(value_span),
                        });
                    }
                    _ => {
                        let found = self.peek().kind.describe();
                        self.error(
                            Diagnostic::error(
                                codes::EXPECTED_TOKEN,
                                format!("expected a token count, found {found}"),
                            )
                            .with_primary(value_span, "expected an integer such as `128k`"),
                        );
                    }
                }
            } else {
                requirements.push(ModelRequirement::Capability(name));
            }
            self.eat(&TokenKind::Comma);
            if self.position == before {
                self.bump();
            }
        }
        let end = self.span();
        self.expect(TokenKind::RBrace);
        Some(ModelBlock::Requires {
            requirements,
            span: start.merge(end),
        })
    }

    fn parse_tools_block(&mut self) -> Option<ToolsBlock> {
        let start = self.span();
        self.bump(); // `tools`
        if !self.expect(TokenKind::LBrace) {
            return None;
        }
        let mut grants = Vec::new();
        while !self.at(&TokenKind::RBrace) && !self.at_barrier() {
            let before = self.position;
            if let Some(transport) = self.expect_ident_like("a tool transport such as `mcp`") {
                if let Some(name) = self.parse_dotted_name("a tool name") {
                    grants.push(ToolGrant {
                        span: transport.span.merge(name.span),
                        transport,
                        name,
                    });
                }
            }
            self.eat(&TokenKind::Comma);
            if self.position == before {
                self.bump();
            }
        }
        let end = self.span();
        self.expect(TokenKind::RBrace);
        Some(ToolsBlock {
            grants,
            span: start.merge(end),
        })
    }

    fn parse_memory_block(&mut self) -> Option<MemoryBlock> {
        let start = self.span();
        self.bump(); // `memory`
        if !self.expect(TokenKind::LBrace) {
            return None;
        }
        let mut working = None;
        while !self.at(&TokenKind::RBrace) && !self.at_barrier() {
            let before = self.position;
            if self.at_keyword(Keyword::Working) {
                let working_start = self.span();
                self.bump();
                if let Some(lifetime) =
                    self.expect_ident_like("a memory lifetime such as `ephemeral`")
                {
                    let fields = if self.at(&TokenKind::LBrace) {
                        self.parse_field_block().unwrap_or_default()
                    } else {
                        Vec::new()
                    };
                    working = Some(WorkingMemory {
                        span: working_start.merge(self.previous_span()),
                        lifetime,
                        fields,
                    });
                }
            } else {
                let found = self.peek().kind.describe();
                let span = self.span();
                self.error(
                    Diagnostic::error(
                        codes::UNEXPECTED_TOKEN,
                        format!("expected `working`, found {found}"),
                    )
                    .with_primary(span, "only `working` memory exists in language 0.1"),
                );
                self.bump();
            }
            if self.position == before {
                self.bump();
            }
        }
        let end = self.span();
        self.expect(TokenKind::RBrace);
        Some(MemoryBlock {
            working,
            span: start.merge(end),
        })
    }

    fn parse_budget_block(&mut self) -> Option<BudgetBlock> {
        let start = self.span();
        self.bump(); // `budget`
        if !self.expect(TokenKind::LBrace) {
            return None;
        }
        let mut limits = Vec::new();
        while !self.at(&TokenKind::RBrace) && !self.at_barrier() {
            let before = self.position;
            if let Some(key) = self.expect_ident_like("a budget key") {
                self.expect(TokenKind::Le);
                let value_span = self.span();
                let value = match self.peek().kind {
                    TokenKind::Int(value) => {
                        self.bump();
                        Some(NumberLit::Int(value))
                    }
                    TokenKind::Float(value) => {
                        self.bump();
                        Some(NumberLit::Float(value))
                    }
                    _ => {
                        let found = self.peek().kind.describe();
                        self.error(
                            Diagnostic::error(
                                codes::EXPECTED_TOKEN,
                                format!("expected a budget value, found {found}"),
                            )
                            .with_primary(value_span, "expected a number"),
                        );
                        None
                    }
                };
                if let Some(value) = value {
                    // An identifier here is a unit (`5 usd`) unless it is the key
                    // of the next limit, which `<=` gives away.
                    let unit = match &self.peek().kind {
                        TokenKind::Ident(text) if !self.peek_at(1).is(&TokenKind::Le) => {
                            let unit = Ident::new(text.clone(), self.span());
                            self.bump();
                            Some(unit)
                        }
                        _ => None,
                    };
                    limits.push(BudgetLimit {
                        span: key.span.merge(self.previous_span()),
                        key,
                        value,
                        unit,
                    });
                }
            }
            self.eat(&TokenKind::Comma);
            if self.position == before {
                self.bump();
            }
        }
        let end = self.span();
        self.expect(TokenKind::RBrace);
        Some(BudgetBlock {
            limits,
            span: start.merge(end),
        })
    }

    fn parse_policy_block(&mut self) -> Option<PolicyBlock> {
        let start = self.span();
        self.bump(); // `policy`
        if !self.expect(TokenKind::LBrace) {
            return None;
        }
        let mut rules = Vec::new();
        while !self.at(&TokenKind::RBrace) && !self.at_barrier() {
            let before = self.position;
            if let Some(subject) = self.expect_ident_like("a policy subject") {
                if let Some(action) = self.parse_policy_action() {
                    rules.push(PolicyRule {
                        span: subject.span.merge(action.span()),
                        subject,
                        action,
                    });
                }
            }
            self.eat(&TokenKind::Comma);
            if self.position == before {
                self.bump();
            }
        }
        let end = self.span();
        self.expect(TokenKind::RBrace);
        Some(PolicyBlock {
            rules,
            span: start.merge(end),
        })
    }

    fn parse_policy_action(&mut self) -> Option<PolicyAction> {
        let start = self.span();
        if self.eat_keyword(Keyword::Allow) {
            let mut values = Vec::new();
            let mut qualifier = None;
            if self.at(&TokenKind::LBracket) {
                values = self.parse_string_list();
            } else {
                qualifier = self.parse_policy_qualifier();
            }
            return Some(PolicyAction::Allow {
                qualifier,
                values,
                span: start.merge(self.previous_span()),
            });
        }
        if self.eat_keyword(Keyword::Deny) {
            let qualifier = self.parse_policy_qualifier();
            return Some(PolicyAction::Deny {
                qualifier,
                span: start.merge(self.previous_span()),
            });
        }
        if self.eat_keyword(Keyword::Require) {
            self.expect_keyword(Keyword::Approval);
            return Some(PolicyAction::RequireApproval {
                span: start.merge(self.previous_span()),
            });
        }

        let found = self.peek().kind.describe();
        self.error(
            Diagnostic::error(
                codes::INVALID_POLICY_ACTION,
                format!("expected a policy action, found {found}"),
            )
            .with_primary(start, "expected `allow`, `deny` or `require approval`"),
        );
        None
    }

    /// An identifier trailing a policy action, as in `secrets deny export`.
    ///
    /// Policy rules are newline-separated but the grammar is newline-insensitive,
    /// so the identifier that follows `deny` could equally be the subject of the
    /// next rule. An action keyword after it settles the question.
    fn parse_policy_qualifier(&mut self) -> Option<Ident> {
        let TokenKind::Ident(text) = &self.peek().kind else {
            return None;
        };
        let starts_next_rule = matches!(
            self.peek_at(1).keyword(),
            Some(Keyword::Allow) | Some(Keyword::Deny) | Some(Keyword::Require)
        );
        if starts_next_rule {
            return None;
        }
        let qualifier = Ident::new(text.clone(), self.span());
        self.bump();
        Some(qualifier)
    }

    fn parse_string_list(&mut self) -> Vec<StringLit> {
        let mut values = Vec::new();
        if !self.expect(TokenKind::LBracket) {
            return values;
        }
        while !self.at(&TokenKind::RBracket) && !self.at_barrier() {
            let before = self.position;
            if let TokenKind::Str(raw) = self.peek().kind.clone() {
                let span = self.span();
                self.bump();
                values.push(self.build_string_literal(&raw, span));
            } else {
                let found = self.peek().kind.describe();
                let span = self.span();
                self.error(
                    Diagnostic::error(
                        codes::EXPECTED_TOKEN,
                        format!("expected a string literal, found {found}"),
                    )
                    .with_primary(span, "expected a quoted value"),
                );
                self.bump();
            }
            self.eat(&TokenKind::Comma);
            if self.position == before {
                self.bump();
            }
        }
        self.expect(TokenKind::RBracket);
        values
    }

    // --- flow -------------------------------------------------------------

    fn parse_flow_block(&mut self) -> Option<FlowBlock> {
        let start = self.span();
        self.bump(); // `flow`
        let statements = self.parse_statement_block();
        Some(FlowBlock {
            statements,
            span: start.merge(self.previous_span()),
        })
    }

    fn parse_statement_block(&mut self) -> Vec<Stmt> {
        let mut statements = Vec::new();
        if !self.expect(TokenKind::LBrace) {
            return statements;
        }
        while !self.at(&TokenKind::RBrace) && !self.at_barrier() {
            let before = self.position;
            statements.push(self.parse_statement());
            self.eat(&TokenKind::Semicolon);
            if self.position == before {
                self.bump();
            }
        }
        self.expect(TokenKind::RBrace);
        statements
    }

    fn parse_statement(&mut self) -> Stmt {
        let start = self.span();
        match self.peek().keyword() {
            Some(Keyword::Emit) => {
                self.bump();
                let Some(output) = self.expect_ident("an output name") else {
                    return self.error_statement(start);
                };
                self.expect(TokenKind::Eq);
                let value = self.parse_expr();
                Stmt::Emit {
                    output,
                    value,
                    span: start.merge(self.previous_span()),
                }
            }
            Some(Keyword::Verify) => {
                self.bump();
                let Some(validator) = self.expect_ident("a verifier name") else {
                    return self.error_statement(start);
                };
                let args = self.parse_args();
                Stmt::Verify {
                    validator,
                    args,
                    span: start.merge(self.previous_span()),
                }
            }
            Some(Keyword::Checkpoint) => {
                self.bump();
                let span = self.span();
                let TokenKind::Str(raw) = self.peek().kind.clone() else {
                    let found = self.peek().kind.describe();
                    self.error(
                        Diagnostic::error(
                            codes::EXPECTED_TOKEN,
                            format!("expected a checkpoint label, found {found}"),
                        )
                        .with_primary(span, "expected a string literal"),
                    );
                    return self.error_statement(start);
                };
                self.bump();
                let label = self.build_string_literal(&raw, span);
                Stmt::Checkpoint {
                    label,
                    span: start.merge(span),
                }
            }
            Some(Keyword::If) => {
                self.bump();
                let condition = self.parse_expr();
                let then_branch = self.parse_statement_block();
                let else_branch = if self.eat_keyword(Keyword::Else) {
                    Some(self.parse_statement_block())
                } else {
                    None
                };
                Stmt::If {
                    condition,
                    then_branch,
                    else_branch,
                    span: start.merge(self.previous_span()),
                }
            }
            Some(Keyword::Loop) => {
                self.bump();
                let max = if self.eat_keyword(Keyword::Max) {
                    let span = self.span();
                    match self.peek().kind {
                        TokenKind::Int(value) => {
                            self.bump();
                            Some(LoopBound { value, span })
                        }
                        _ => {
                            let found = self.peek().kind.describe();
                            self.error(
                                Diagnostic::error(
                                    codes::EXPECTED_TOKEN,
                                    format!("expected an iteration bound, found {found}"),
                                )
                                .with_primary(span, "expected an integer"),
                            );
                            None
                        }
                    }
                } else {
                    None
                };
                let guard = if self.eat_keyword(Keyword::While) {
                    Some(self.parse_expr())
                } else {
                    None
                };
                let body = self.parse_statement_block();
                Stmt::Loop {
                    max,
                    guard,
                    body,
                    span: start.merge(self.previous_span()),
                }
            }
            Some(Keyword::State) if self.peek_at(1).is(&TokenKind::Dot) => {
                // `state.field = expr` is a write; anything else is a read expression.
                let is_write = matches!(self.peek_at(3).kind, TokenKind::Eq);
                if is_write {
                    self.bump(); // `state`
                    self.bump(); // `.`
                    let Some(field) = self.expect_ident("a state field name") else {
                        return self.error_statement(start);
                    };
                    self.expect(TokenKind::Eq);
                    let value = self.parse_expr();
                    Stmt::StateWrite {
                        field,
                        value,
                        span: start.merge(self.previous_span()),
                    }
                } else {
                    let value = self.parse_expr();
                    Stmt::Expr {
                        span: value.span(),
                        value,
                    }
                }
            }
            _ => {
                // `name = expr` binds; `name(...)` and everything else is an expression.
                let is_bind = matches!(self.peek().kind, TokenKind::Ident(_))
                    && matches!(self.peek_at(1).kind, TokenKind::Eq);
                if is_bind {
                    let TokenKind::Ident(text) = self.peek().kind.clone() else {
                        unreachable!()
                    };
                    let name = Ident::new(text, self.span());
                    self.bump();
                    self.bump(); // `=`
                    let value = self.parse_expr();
                    Stmt::Bind {
                        name,
                        value,
                        span: start.merge(self.previous_span()),
                    }
                } else if self.can_start_expr() {
                    let value = self.parse_expr();
                    Stmt::Expr {
                        span: value.span(),
                        value,
                    }
                } else {
                    let found = self.peek().kind.describe();
                    self.error(
                        Diagnostic::error(
                            codes::UNEXPECTED_TOKEN,
                            format!("expected a statement, found {found}"),
                        )
                        .with_primary(
                            start,
                            "expected a binding, a call, `emit`, `verify`, `if` or `loop`",
                        ),
                    );
                    self.bump();
                    Stmt::Error { span: start }
                }
            }
        }
    }

    fn error_statement(&mut self, span: Span) -> Stmt {
        self.recover_in_block();
        Stmt::Error {
            span: span.merge(self.previous_span()),
        }
    }

    /// Skip to a plausible next statement boundary inside the current block.
    fn recover_in_block(&mut self) {
        let mut depth = 0usize;
        while !self.at_eof() {
            match &self.peek().kind {
                TokenKind::LBrace => depth += 1,
                TokenKind::RBrace if depth == 0 => return,
                TokenKind::RBrace => depth -= 1,
                TokenKind::Semicolon if depth == 0 => {
                    self.bump();
                    return;
                }
                TokenKind::Keyword(keyword) if depth == 0 && starts_statement(*keyword) => return,
                _ => {}
            }
            self.bump();
        }
    }

    fn can_start_expr(&self) -> bool {
        match &self.peek().kind {
            TokenKind::Ident(_)
            | TokenKind::Int(_)
            | TokenKind::Float(_)
            | TokenKind::Str(_)
            | TokenKind::LBracket
            | TokenKind::LParen
            | TokenKind::Bang
            | TokenKind::Minus => true,
            TokenKind::Keyword(keyword) => matches!(
                keyword,
                Keyword::Ask
                    | Keyword::Call
                    | Keyword::Parallel
                    | Keyword::State
                    | Keyword::True
                    | Keyword::False
            ),
            _ => false,
        }
    }

    // --- expressions ------------------------------------------------------

    fn parse_expr(&mut self) -> Expr {
        self.parse_or()
    }

    fn parse_or(&mut self) -> Expr {
        let mut lhs = self.parse_and();
        while self.at(&TokenKind::PipePipe) {
            self.bump();
            let rhs = self.parse_and();
            lhs = Expr::Binary {
                op: BinaryOp::Or,
                span: lhs.span().merge(rhs.span()),
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            };
        }
        lhs
    }

    fn parse_and(&mut self) -> Expr {
        let mut lhs = self.parse_comparison();
        while self.at(&TokenKind::AmpAmp) {
            self.bump();
            let rhs = self.parse_comparison();
            lhs = Expr::Binary {
                op: BinaryOp::And,
                span: lhs.span().merge(rhs.span()),
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            };
        }
        lhs
    }

    /// Comparisons do not chain: `a < b < c` is a syntax error, not a surprise.
    fn parse_comparison(&mut self) -> Expr {
        let lhs = self.parse_additive();
        let op = match self.peek().kind {
            TokenKind::EqEq => BinaryOp::Eq,
            TokenKind::BangEq => BinaryOp::NotEq,
            TokenKind::Lt => BinaryOp::Lt,
            TokenKind::Le => BinaryOp::Le,
            TokenKind::Gt => BinaryOp::Gt,
            TokenKind::Ge => BinaryOp::Ge,
            _ => return lhs,
        };
        self.bump();
        let rhs = self.parse_additive();
        Expr::Binary {
            op,
            span: lhs.span().merge(rhs.span()),
            lhs: Box::new(lhs),
            rhs: Box::new(rhs),
        }
    }

    fn parse_additive(&mut self) -> Expr {
        let mut lhs = self.parse_unary();
        loop {
            let op = match self.peek().kind {
                TokenKind::Plus => BinaryOp::Add,
                TokenKind::Minus => BinaryOp::Sub,
                _ => return lhs,
            };
            self.bump();
            let rhs = self.parse_unary();
            lhs = Expr::Binary {
                op,
                span: lhs.span().merge(rhs.span()),
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            };
        }
    }

    fn parse_unary(&mut self) -> Expr {
        let start = self.span();
        let op = match self.peek().kind {
            TokenKind::Bang => UnaryOp::Not,
            TokenKind::Minus => UnaryOp::Neg,
            _ => return self.parse_primary(),
        };
        self.bump();
        let operand = self.parse_unary();
        Expr::Unary {
            op,
            span: start.merge(operand.span()),
            operand: Box::new(operand),
        }
    }

    fn parse_primary(&mut self) -> Expr {
        let start = self.span();
        match self.peek().kind.clone() {
            TokenKind::Str(raw) => {
                self.bump();
                Expr::Str(self.build_string_literal(&raw, start))
            }
            TokenKind::Int(value) => {
                self.bump();
                Expr::Int { value, span: start }
            }
            TokenKind::Float(value) => {
                self.bump();
                Expr::Float { value, span: start }
            }
            TokenKind::Keyword(Keyword::True) => {
                self.bump();
                Expr::Bool {
                    value: true,
                    span: start,
                }
            }
            TokenKind::Keyword(Keyword::False) => {
                self.bump();
                Expr::Bool {
                    value: false,
                    span: start,
                }
            }
            TokenKind::LParen => {
                self.bump();
                let inner = self.parse_expr();
                self.expect(TokenKind::RParen);
                inner
            }
            TokenKind::LBracket => {
                self.bump();
                let mut items = Vec::new();
                while !self.at(&TokenKind::RBracket) && !self.at_barrier() {
                    let before = self.position;
                    items.push(self.parse_expr());
                    self.eat(&TokenKind::Comma);
                    if self.position == before {
                        self.bump();
                    }
                }
                let end = self.span();
                self.expect(TokenKind::RBracket);
                Expr::List {
                    items,
                    span: start.merge(end),
                }
            }
            TokenKind::Keyword(Keyword::Ask) => {
                self.bump();
                self.expect(TokenKind::Lt);
                let Some(result) = self.parse_type() else {
                    return Expr::Error {
                        span: start.merge(self.previous_span()),
                    };
                };
                self.expect(TokenKind::Gt);
                let args = self.parse_args();
                Expr::Ask {
                    result,
                    args,
                    span: start.merge(self.previous_span()),
                }
            }
            TokenKind::Keyword(Keyword::Call) => {
                self.bump();
                let Some(callee) = self.parse_dotted_name("a tool or agent name") else {
                    return Expr::Error {
                        span: start.merge(self.previous_span()),
                    };
                };
                let args = self.parse_args();
                Expr::Call {
                    callee,
                    args,
                    span: start.merge(self.previous_span()),
                }
            }
            TokenKind::Keyword(Keyword::Parallel) => {
                self.bump();
                self.expect_keyword(Keyword::Map);
                let source = self.parse_expr();
                self.expect_keyword(Keyword::As);
                let Some(binder) = self.expect_ident("a loop variable name") else {
                    return Expr::Error {
                        span: start.merge(self.previous_span()),
                    };
                };
                let body = self.parse_statement_block();
                Expr::ParallelMap {
                    source: Box::new(source),
                    binder,
                    body,
                    span: start.merge(self.previous_span()),
                }
            }
            TokenKind::Keyword(Keyword::State) => {
                self.bump();
                let segments = self.parse_path_segments();
                Expr::Path(PathExpr {
                    root: PathRoot::State { span: start },
                    span: start.merge(self.previous_span()),
                    segments,
                })
            }
            TokenKind::Ident(text) => {
                let ident = Ident::new(text, start);
                self.bump();
                if self.at(&TokenKind::LParen) && BUILTINS.contains(&ident.text.as_str()) {
                    let args = self.parse_positional_args();
                    return Expr::Builtin {
                        name: ident,
                        args,
                        span: start.merge(self.previous_span()),
                    };
                }
                if self.at(&TokenKind::LParen) {
                    let span = self.span();
                    self.error(
                        Diagnostic::error(
                            codes::UNEXPECTED_TOKEN,
                            format!("`{}` is not a builtin function", ident.text),
                        )
                        .with_primary(span, "unexpected call")
                        .with_help(
                            "call tools and sub-agents with `call name(...)`; \
                             builtin functions are: len",
                        ),
                    );
                    let _ = self.parse_positional_args();
                    return Expr::Error {
                        span: start.merge(self.previous_span()),
                    };
                }
                let segments = self.parse_path_segments();
                Expr::Path(PathExpr {
                    span: start.merge(self.previous_span()),
                    root: PathRoot::Binding(ident),
                    segments,
                })
            }
            other => {
                self.error(
                    Diagnostic::error(
                        codes::UNEXPECTED_TOKEN,
                        format!("expected an expression, found {}", other.describe()),
                    )
                    .with_primary(start, "expected a value"),
                );
                self.bump();
                Expr::Error { span: start }
            }
        }
    }

    fn parse_path_segments(&mut self) -> Vec<Ident> {
        let mut segments = Vec::new();
        while self.at(&TokenKind::Dot) {
            self.bump();
            let Some(segment) = self.expect_ident("a field name") else {
                break;
            };
            segments.push(segment);
        }
        segments
    }

    fn parse_args(&mut self) -> Vec<Arg> {
        let mut args = Vec::new();
        if !self.expect(TokenKind::LParen) {
            return args;
        }
        while !self.at(&TokenKind::RParen) && !self.at_barrier() {
            let before = self.position;
            let start = self.span();
            let name = if matches!(self.peek().kind, TokenKind::Ident(_))
                && self.peek_at(1).is(&TokenKind::Colon)
            {
                let TokenKind::Ident(text) = self.peek().kind.clone() else {
                    unreachable!()
                };
                let ident = Ident::new(text, self.span());
                self.bump();
                self.bump(); // `:`
                Some(ident)
            } else {
                None
            };
            let value = self.parse_expr();
            args.push(Arg {
                name,
                span: start.merge(value.span()),
                value,
            });
            self.eat(&TokenKind::Comma);
            if self.position == before {
                self.bump();
            }
        }
        self.expect(TokenKind::RParen);
        args
    }

    fn parse_positional_args(&mut self) -> Vec<Expr> {
        let mut args = Vec::new();
        if !self.expect(TokenKind::LParen) {
            return args;
        }
        while !self.at(&TokenKind::RParen) && !self.at_barrier() {
            let before = self.position;
            args.push(self.parse_expr());
            self.eat(&TokenKind::Comma);
            if self.position == before {
                self.bump();
            }
        }
        self.expect(TokenKind::RParen);
        args
    }

    fn build_string_literal(&mut self, raw: &str, span: Span) -> StringLit {
        let (literal, diagnostics) = strings::parse_string_literal(raw, span);
        self.diagnostics.extend(diagnostics);
        literal
    }
}

/// Keywords that unambiguously begin a statement, used as recovery anchors.
fn starts_statement(keyword: Keyword) -> bool {
    matches!(
        keyword,
        Keyword::Emit
            | Keyword::Verify
            | Keyword::Checkpoint
            | Keyword::If
            | Keyword::Loop
            | Keyword::Call
            | Keyword::Ask
            | Keyword::Parallel
            | Keyword::State
    )
}

fn language_supports_v0_2_types(version: Option<LanguageVersion>) -> bool {
    matches!(
        version,
        Some(LanguageVersion {
            major: 0,
            minor: 2..,
            ..
        })
    )
}

fn first_v0_2_type_span(program: &Program) -> Option<Span> {
    fn from_type(ty: &TypeExpr) -> Option<Span> {
        match ty {
            TypeExpr::Named(_) => None,
            TypeExpr::List { element, .. } => from_type(element),
            TypeExpr::Optional { span, .. } | TypeExpr::Union { span, .. } => Some(*span),
        }
    }

    fn from_fields(fields: &[FieldDecl]) -> Option<Span> {
        fields.iter().find_map(|field| from_type(&field.ty))
    }

    fn from_expr(expr: &Expr) -> Option<Span> {
        match expr {
            Expr::Ask { result, args, .. } => {
                from_type(result).or_else(|| args.iter().find_map(|arg| from_expr(&arg.value)))
            }
            Expr::Call { args, .. } => args.iter().find_map(|arg| from_expr(&arg.value)),
            Expr::ParallelMap { source, body, .. } => {
                from_expr(source).or_else(|| from_statements(body))
            }
            Expr::List { items, .. } => items.iter().find_map(from_expr),
            Expr::Builtin { args, .. } => args.iter().find_map(from_expr),
            Expr::Unary { operand, .. } => from_expr(operand),
            Expr::Binary { lhs, rhs, .. } => from_expr(lhs).or_else(|| from_expr(rhs)),
            Expr::Str(_)
            | Expr::Int { .. }
            | Expr::Float { .. }
            | Expr::Bool { .. }
            | Expr::Path(_)
            | Expr::Error { .. } => None,
        }
    }

    fn from_statements(statements: &[Stmt]) -> Option<Span> {
        statements.iter().find_map(|statement| match statement {
            Stmt::Bind { value, .. }
            | Stmt::StateWrite { value, .. }
            | Stmt::Expr { value, .. }
            | Stmt::Emit { value, .. } => from_expr(value),
            Stmt::Verify { args, .. } => args.iter().find_map(|arg| from_expr(&arg.value)),
            Stmt::If {
                condition,
                then_branch,
                else_branch,
                ..
            } => from_expr(condition)
                .or_else(|| from_statements(then_branch))
                .or_else(|| else_branch.as_deref().and_then(from_statements)),
            Stmt::Loop { guard, body, .. } => guard
                .as_ref()
                .and_then(from_expr)
                .or_else(|| from_statements(body)),
            Stmt::Checkpoint { .. } | Stmt::Error { .. } => None,
        })
    }

    program
        .types
        .iter()
        .find_map(|decl| from_fields(&decl.fields))
        .or_else(|| {
            program
                .tools
                .iter()
                .find_map(|decl| from_fields(&decl.params).or_else(|| from_type(&decl.ret)))
        })
        .or_else(|| {
            program
                .verifiers
                .iter()
                .find_map(|decl| from_fields(&decl.params))
        })
        .or_else(|| {
            program.agents.iter().find_map(|decl| {
                from_fields(&decl.params)
                    .or_else(|| {
                        decl.memory
                            .as_ref()
                            .and_then(|memory| memory.working.as_ref())
                            .and_then(|working| from_fields(&working.fields))
                    })
                    .or_else(|| {
                        decl.flow
                            .as_ref()
                            .and_then(|flow| from_statements(&flow.statements))
                    })
            })
        })
}

/// Lets `set_section` report duplicates uniformly across section types.
trait HasSpan {
    fn span(&self) -> Span;
}

impl HasSpan for ModelBlock {
    fn span(&self) -> Span {
        ModelBlock::span(self)
    }
}

impl HasSpan for ToolsBlock {
    fn span(&self) -> Span {
        self.span
    }
}

impl HasSpan for MemoryBlock {
    fn span(&self) -> Span {
        self.span
    }
}

impl HasSpan for BudgetBlock {
    fn span(&self) -> Span {
        self.span
    }
}

impl HasSpan for PolicyBlock {
    fn span(&self) -> Span {
        self.span
    }
}

impl HasSpan for FlowBlock {
    fn span(&self) -> Span {
        self.span
    }
}

#[cfg(test)]
mod tests;
