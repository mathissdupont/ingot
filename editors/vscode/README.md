# Ingot VS Code Extension

This is the reference editor integration for `.ing` source. It contributes:

- `.ing` language detection;
- TextMate syntax highlighting;
- bracket/comment rules;
- an LSP client that starts `ingot-lsp` over stdio.

## Local development

Build the Rust language server first:

```powershell
cargo build -p ingot-lsp
```

Then install the JavaScript dependencies from this directory:

```powershell
npm install
```

Open this `editors/vscode` directory in VS Code and run the extension host. If
`ingot-lsp` is not on `PATH`, set `ingot.lsp.path` to the built binary, for
example:

```json
{
  "ingot.lsp.path": "../../target/debug/ingot-lsp.exe"
}
```

When the extension is active, `.ing` files receive compiler-backed diagnostics,
formatting, completion, hover and go-to-definition from the shared
`ingot-language-service`/`ingot-lsp` stack.
