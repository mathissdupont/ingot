# Ingot Language Service

The language service is the editor-neutral foundation for `.ing` authoring. It
does not implement an editor protocol itself. Instead, protocol adapters such as
an LSP server call this crate and translate the result into their own wire
format.

## Contract

`ingot-language-service` exposes two first surfaces:

- `check_source` and `check_file` compile with the existing compiler and return
  structured diagnostics.
- `format_source` uses the same canonical printer as `ingot fmt` and returns a
  full-document text edit when the source is parseable and not already formatted.

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
   +-- future LSP server
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
maintained example through the editor-facing surface. This is not the complete
M7 issue yet: grammar publishing, a real LSP server, completion, hover,
definition navigation and a reference extension remain tracked in issue #6.
