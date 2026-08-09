//! Tokenizer for the Ingot agent language.
//!
//! The lexer never fails: malformed input produces a diagnostic plus an
//! [`TokenKind::Error`] token so that the parser can keep going and report more
//! than one problem per run. Trivia (whitespace and comments) is skipped, but
//! doc comments (`///`) are attached to the token that follows them.

use ingot_diagnostics::{codes, Diagnostic, DiagnosticBag};
use ingot_source::{FileId, SourceFile, Span};

mod token;

pub use token::{Keyword, Token, TokenKind, KEYWORDS};

/// Result of tokenizing one file.
#[derive(Debug)]
pub struct LexResult {
    pub tokens: Vec<Token>,
    pub diagnostics: DiagnosticBag,
}

/// Tokenize a source file.
pub fn tokenize(file: &SourceFile) -> LexResult {
    Lexer::new(file).run()
}

struct Lexer<'a> {
    file_id: FileId,
    text: &'a str,
    bytes: &'a [u8],
    offset: usize,
    tokens: Vec<Token>,
    diagnostics: DiagnosticBag,
    pending_doc: Vec<String>,
}

impl<'a> Lexer<'a> {
    fn new(file: &'a SourceFile) -> Self {
        Lexer {
            file_id: file.id(),
            text: file.text(),
            bytes: file.text().as_bytes(),
            offset: 0,
            tokens: Vec::new(),
            diagnostics: DiagnosticBag::new(),
            pending_doc: Vec::new(),
        }
    }

    fn run(mut self) -> LexResult {
        loop {
            self.skip_trivia();
            let start = self.offset;
            let Some(ch) = self.peek_char() else { break };

            let kind = match ch {
                c if is_ident_start(c) => self.lex_ident_or_keyword(),
                c if c.is_ascii_digit() => self.lex_number(),
                '"' => self.lex_string(),
                _ => self.lex_symbol(ch),
            };

            let span = self.span_from(start);
            let doc = if self.pending_doc.is_empty() {
                None
            } else {
                Some(std::mem::take(&mut self.pending_doc).join("\n"))
            };
            self.tokens.push(Token { kind, span, doc });
        }

        let eof = Span::empty_at(self.file_id, self.text.len() as u32);
        self.tokens.push(Token {
            kind: TokenKind::Eof,
            span: eof,
            doc: None,
        });

        LexResult {
            tokens: self.tokens,
            diagnostics: self.diagnostics,
        }
    }

    // --- character helpers ------------------------------------------------

    fn peek_char(&self) -> Option<char> {
        self.text[self.offset..].chars().next()
    }

    fn peek_byte(&self, ahead: usize) -> Option<u8> {
        self.bytes.get(self.offset + ahead).copied()
    }

    fn bump(&mut self) -> Option<char> {
        let ch = self.peek_char()?;
        self.offset += ch.len_utf8();
        Some(ch)
    }

    fn eat(&mut self, expected: u8) -> bool {
        if self.peek_byte(0) == Some(expected) {
            self.offset += 1;
            true
        } else {
            false
        }
    }

    fn span_from(&self, start: usize) -> Span {
        Span::new(self.file_id, start as u32, self.offset as u32)
    }

    // --- trivia -----------------------------------------------------------

    fn skip_trivia(&mut self) {
        loop {
            match self.peek_byte(0) {
                Some(b) if (b as char).is_whitespace() => {
                    self.offset += 1;
                }
                Some(b'/') if self.peek_byte(1) == Some(b'/') => self.skip_line_comment(),
                Some(b'/') if self.peek_byte(1) == Some(b'*') => self.skip_block_comment(),
                _ => return,
            }
        }
    }

    fn skip_line_comment(&mut self) {
        let is_doc = self.peek_byte(2) == Some(b'/') && self.peek_byte(3) != Some(b'/');
        self.offset += if is_doc { 3 } else { 2 };
        let start = self.offset;
        while let Some(b) = self.peek_byte(0) {
            if b == b'\n' {
                break;
            }
            self.offset += 1;
        }
        if is_doc {
            self.pending_doc
                .push(self.text[start..self.offset].trim().to_string());
        }
    }

    fn skip_block_comment(&mut self) {
        let start = self.offset;
        self.offset += 2;
        let mut depth = 1usize;
        while depth > 0 {
            match self.peek_byte(0) {
                None => {
                    self.diagnostics.push(
                        Diagnostic::error(
                            codes::UNTERMINATED_BLOCK_COMMENT,
                            "unterminated block comment",
                        )
                        .with_primary(self.span_from(start), "comment starts here")
                        .with_help("close the comment with `*/`"),
                    );
                    return;
                }
                Some(b'/') if self.peek_byte(1) == Some(b'*') => {
                    self.offset += 2;
                    depth += 1;
                }
                Some(b'*') if self.peek_byte(1) == Some(b'/') => {
                    self.offset += 2;
                    depth -= 1;
                }
                Some(_) => self.offset += 1,
            }
        }
    }

    // --- tokens -----------------------------------------------------------

    fn lex_ident_or_keyword(&mut self) -> TokenKind {
        let start = self.offset;
        while let Some(ch) = self.peek_char() {
            if is_ident_continue(ch) {
                self.offset += ch.len_utf8();
            } else {
                break;
            }
        }
        let text = &self.text[start..self.offset];
        match Keyword::lookup(text) {
            Some(keyword) => TokenKind::Keyword(keyword),
            None => TokenKind::Ident(text.to_string()),
        }
    }

    /// Integers, floats, and integers with a binary size suffix (`128k`, `2m`).
    ///
    /// The suffix multiplies by 1024 and 1024*1024 respectively, matching how
    /// context windows are quoted in practice (`context >= 128k` is 131072).
    fn lex_number(&mut self) -> TokenKind {
        let start = self.offset;
        while matches!(self.peek_byte(0), Some(b) if b.is_ascii_digit() || b == b'_') {
            self.offset += 1;
        }

        let is_float = self.peek_byte(0) == Some(b'.')
            && matches!(self.peek_byte(1), Some(b) if b.is_ascii_digit());
        if is_float {
            self.offset += 1;
            while matches!(self.peek_byte(0), Some(b) if b.is_ascii_digit() || b == b'_') {
                self.offset += 1;
            }
            let raw: String = self.text[start..self.offset].replace('_', "");
            return match raw.parse::<f64>() {
                Ok(value) => TokenKind::Float(value),
                Err(_) => self.number_error(start, &raw),
            };
        }

        let digits: String = self.text[start..self.offset].replace('_', "");
        let multiplier = match self.peek_byte(0) {
            Some(b'k') | Some(b'K') if !self.suffix_continues(1) => {
                self.offset += 1;
                1024u64
            }
            Some(b'm') | Some(b'M') if !self.suffix_continues(1) => {
                self.offset += 1;
                1024 * 1024
            }
            _ => 1,
        };

        match digits
            .parse::<u64>()
            .ok()
            .and_then(|value| value.checked_mul(multiplier))
        {
            Some(value) if value <= i64::MAX as u64 => TokenKind::Int(value as i64),
            _ => self.number_error(start, &digits),
        }
    }

    /// True when the byte after a size suffix would make it part of an identifier.
    fn suffix_continues(&self, ahead: usize) -> bool {
        self.text[self.offset + ahead..]
            .chars()
            .next()
            .is_some_and(is_ident_continue)
    }

    fn number_error(&mut self, start: usize, raw: &str) -> TokenKind {
        self.diagnostics.push(
            Diagnostic::error(
                codes::INVALID_NUMBER,
                format!("invalid numeric literal `{raw}`"),
            )
            .with_primary(self.span_from(start), "cannot be represented"),
        );
        TokenKind::Error
    }

    /// String literals are single-line and may contain `${...}` interpolations.
    ///
    /// The token carries the *raw* text between the quotes. Escape resolution and
    /// interpolation splitting happen in the parser, which needs exact byte
    /// offsets to give every `${...}` placeholder its own span.
    fn lex_string(&mut self) -> TokenKind {
        let start = self.offset;
        self.offset += 1; // opening quote
        let inner_start = self.offset;

        loop {
            match self.peek_byte(0) {
                None | Some(b'\n') => {
                    self.diagnostics.push(
                        Diagnostic::error(
                            codes::UNTERMINATED_STRING,
                            "unterminated string literal",
                        )
                        .with_primary(self.span_from(start), "string starts here")
                        .with_help("string literals may not span lines; use `\\n` instead"),
                    );
                    return TokenKind::Error;
                }
                Some(b'"') => {
                    let raw = self.text[inner_start..self.offset].to_string();
                    self.offset += 1;
                    return TokenKind::Str(raw);
                }
                Some(b'\\') => {
                    let escape_start = self.offset;
                    self.offset += 1;
                    match self.bump() {
                        Some('n') | Some('r') | Some('t') | Some('\\') | Some('"') | Some('$') => {}
                        Some(other) => {
                            self.diagnostics.push(
                                Diagnostic::error(
                                    codes::UNEXPECTED_CHARACTER,
                                    format!("unknown escape sequence `\\{other}`"),
                                )
                                .with_primary(self.span_from(escape_start), "not a valid escape")
                                .with_note("valid escapes are \\n \\r \\t \\\\ \\\" \\$"),
                            );
                        }
                        None => {}
                    }
                }
                Some(b'$') if self.peek_byte(1) == Some(b'{') => {
                    let interp_start = self.offset;
                    self.offset += 2;
                    loop {
                        match self.peek_byte(0) {
                            Some(b'}') => {
                                self.offset += 1;
                                break;
                            }
                            None | Some(b'\n') | Some(b'"') => {
                                self.diagnostics.push(
                                    Diagnostic::error(
                                        codes::UNTERMINATED_INTERPOLATION,
                                        "unterminated interpolation in string literal",
                                    )
                                    .with_primary(
                                        self.span_from(interp_start),
                                        "interpolation starts here",
                                    )
                                    .with_help("close the placeholder with `}`"),
                                );
                                return TokenKind::Error;
                            }
                            Some(_) => self.offset += 1,
                        }
                    }
                }
                Some(_) => {
                    self.bump();
                }
            }
        }
    }

    fn lex_symbol(&mut self, ch: char) -> TokenKind {
        let start = self.offset;
        self.offset += ch.len_utf8();
        match ch {
            '(' => TokenKind::LParen,
            ')' => TokenKind::RParen,
            '{' => TokenKind::LBrace,
            '}' => TokenKind::RBrace,
            '[' => TokenKind::LBracket,
            ']' => TokenKind::RBracket,
            ',' => TokenKind::Comma,
            ':' => TokenKind::Colon,
            ';' => TokenKind::Semicolon,
            '.' => TokenKind::Dot,
            '=' => {
                if self.eat(b'=') {
                    TokenKind::EqEq
                } else {
                    TokenKind::Eq
                }
            }
            '!' => {
                if self.eat(b'=') {
                    TokenKind::BangEq
                } else {
                    TokenKind::Bang
                }
            }
            '<' => {
                if self.eat(b'=') {
                    TokenKind::Le
                } else {
                    TokenKind::Lt
                }
            }
            '>' => {
                if self.eat(b'=') {
                    TokenKind::Ge
                } else {
                    TokenKind::Gt
                }
            }
            '-' => {
                if self.eat(b'>') {
                    TokenKind::Arrow
                } else {
                    TokenKind::Minus
                }
            }
            '+' => TokenKind::Plus,
            '?' => TokenKind::Question,
            '&' if self.eat(b'&') => TokenKind::AmpAmp,
            '|' => {
                if self.eat(b'|') {
                    TokenKind::PipePipe
                } else {
                    TokenKind::Pipe
                }
            }
            other => {
                self.diagnostics.push(
                    Diagnostic::error(
                        codes::UNEXPECTED_CHARACTER,
                        format!("unexpected character `{other}`"),
                    )
                    .with_primary(self.span_from(start), "not valid in Ingot source"),
                );
                TokenKind::Error
            }
        }
    }
}

fn is_ident_start(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphabetic()
}

fn is_ident_continue(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphanumeric()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ingot_source::SourceMap;

    fn lex(text: &str) -> LexResult {
        let mut map = SourceMap::new();
        let file = map.add_virtual("test.ing", text);
        tokenize(map.file(file))
    }

    fn kinds(text: &str) -> Vec<TokenKind> {
        lex(text)
            .tokens
            .into_iter()
            .map(|token| token.kind)
            .collect()
    }

    #[test]
    fn separates_keywords_from_identifiers() {
        assert_eq!(
            kinds("agent Research"),
            vec![
                TokenKind::Keyword(Keyword::Agent),
                TokenKind::Ident("Research".into()),
                TokenKind::Eof
            ]
        );
    }

    #[test]
    fn reads_binary_size_suffixes() {
        assert_eq!(kinds("128k")[0], TokenKind::Int(131_072));
        assert_eq!(kinds("2m")[0], TokenKind::Int(2_097_152));
        // `kb` is an identifier, not a suffix, so `2kb` must not silently parse.
        assert_eq!(kinds("2kb")[0], TokenKind::Int(2));
        assert_eq!(kinds("2kb")[1], TokenKind::Ident("kb".into()));
    }

    #[test]
    fn keeps_interpolation_markers_inside_string_values() {
        let tokens = kinds("\"topic: ${topic}\"");
        assert_eq!(tokens[0], TokenKind::Str("topic: ${topic}".into()));
    }

    #[test]
    fn reports_unterminated_string_without_panicking() {
        let result = lex("\"oops\n");
        assert!(result.diagnostics.has_errors());
        assert_eq!(
            result.diagnostics.iter().next().unwrap().code,
            codes::UNTERMINATED_STRING
        );
    }

    #[test]
    fn reports_unterminated_interpolation() {
        let result = lex("\"hello ${name\"");
        assert_eq!(
            result.diagnostics.iter().next().unwrap().code,
            codes::UNTERMINATED_INTERPOLATION
        );
    }

    #[test]
    fn attaches_doc_comments_to_the_next_token() {
        let result = lex("/// A research agent.\nagent A");
        let first = &result.tokens[0];
        assert_eq!(first.kind, TokenKind::Keyword(Keyword::Agent));
        assert_eq!(first.doc.as_deref(), Some("A research agent."));
    }

    #[test]
    fn skips_nested_block_comments() {
        assert_eq!(
            kinds("/* outer /* inner */ still */ agent")[0].clone(),
            TokenKind::Keyword(Keyword::Agent)
        );
    }

    #[test]
    fn lexes_multi_character_operators() {
        assert_eq!(
            kinds("<= >= == != && | || ? ->"),
            vec![
                TokenKind::Le,
                TokenKind::Ge,
                TokenKind::EqEq,
                TokenKind::BangEq,
                TokenKind::AmpAmp,
                TokenKind::Pipe,
                TokenKind::PipePipe,
                TokenKind::Question,
                TokenKind::Arrow,
                TokenKind::Eof
            ]
        );
    }

    #[test]
    fn never_loops_forever_on_invalid_input() {
        let result = lex("agent # A €");
        assert!(result.diagnostics.has_errors());
        assert_eq!(result.tokens.last().unwrap().kind, TokenKind::Eof);
    }
}
