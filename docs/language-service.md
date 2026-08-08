# Ingot Language Service

The language service is the editor-neutral foundation for `.ing` authoring. It
does not implement an editor protocol itself. Instead, protocol adapters such as
an LSP server call this crate and translate the result into their own wire
format.

## Contract

`ingot-language-service` exposes these editor-facing surfaces:

- `check_source` and `check_file` compile with the existing compiler and return
  structured diagnostics.
- `format_source` uses the same canonical printer as `ingot fmt` and returns a
  full-document text edit when the source is parseable and not already formatted.
- `completion_items` returns language keywords, primitive types, built-ins,
  policy/model vocabulary and symbols collected from the parsed program.
- `hover` returns source-linked markdown for declarations and known language
  forms, including doc comments.
- `definition` maps a symbol use back to its declaration range when the target
  is known in the current file.

Diagnostics are projected from `ingot-diagnostics` without recomputing parser or
semantic facts. Each diagnostic keeps:

- the stable diagnostic code;
- severity and message;
- the primary source range;
- every primary and secondary label;
- notes and help text;
- original byte spans as well as zero-based UTF-16 editor positions.

The byte spans make exact CLI/editor equality testable. The UTF-16 positions are
ready for LSP `Position` values, while keeping LSP dependencies out of compiler
crates.

## LSP Adapter

`ingot-lsp` is the first protocol adapter on top of this crate. It is a stdio
language server with:

- full-document text synchronisation;
- UTF-16 position encoding;
- `textDocument/publishDiagnostics` after `didOpen` and `didChange`;
- `textDocument/formatting` backed by the canonical printer;
- `textDocument/completion` for declarations, language forms and built-in
  vocabulary;
- `textDocument/hover` for declaration detail and doc comments;
- `textDocument/definition` for same-file declaration navigation.

It deliberately advertises only the features it implements. The LSP crate keeps
protocol details at the edge; parser, semantic and formatting knowledge stays in
the language service and compiler crates.

Editor integrations should start the `ingot-lsp` binary on stdio and send full
document changes. `ingot-lsp --version` is available for installation checks;
without arguments the process speaks only LSP on stdin/stdout.

## Architecture

```text
.ing source
   |
   v
ingot-compiler
   |
   +-- ingot-diagnostics data
   +-- ingot-syntax canonical printer
   |
   v
ingot-language-service
   |
   +-- ingot-lsp stdio server
   +-- editors/vscode reference extension
   +-- future editor extensions
   +-- future model-assisted authoring repair loop
```

The direction is intentional: the service depends on compiler crates, and editor
integrations depend on the service. No editor extension should parse `.ing`,
type-check effects, or maintain a second diagnostic catalogue.

## Maintained Tests

The required M7 acceptance test is
`editor_and_cli_diagnostics_are_identical`. It compares the code, primary byte
span and message from the language service with the compiler output for the same
source.

`reference_examples_are_clean_through_the_language_service` also checks every
maintained example through the editor-facing surface. Completion, hover and
definition have focused tests that assert declared symbols, doc comments and
declaration navigation.

`ingot-lsp` adds a second check,
`reference_examples_are_clean_through_the_lsp_surface`, so the maintained
examples pass through the actual protocol projection without LSP errors. Its
unit tests also verify the advertised LSP capabilities and the conversion of
completion, hover and definition responses.

## Reference VS Code Extension

`editors/vscode` is the first editor integration. It contributes `.ing` language
detection, TextMate syntax highlighting, bracket/comment rules and an LSP client
that starts `ingot-lsp`.

For local development, build the server with `cargo build -p ingot-lsp`, install
the extension dependencies from `editors/vscode` with `npm install`, and set
`ingot.lsp.path` if `ingot-lsp` is not on `PATH`.
