# Agent image specification

Not started. Planned for milestone M6.

Will define the OCI artifact profile: manifest and config media types, the layer
layout (IR, generated targets, source, prompts, skills, schemas, tests), the
lockfile format, and how a reproducible digest is computed.

Two constraints are already fixed:

- No new registry. Artifacts are pushed to OCI-compatible registries.
  See [ADR-0002](../../docs/adr/0002-compiler-not-runtime.md).
- Secrets never enter an artifact. Only references and schemas do.
  See [SECURITY.md](../../SECURITY.md).

The reproducibility groundwork exists: the IR already has a single canonical
encoding. See [ADR-0004](../../docs/adr/0004-canonical-ir-encoding.md).
