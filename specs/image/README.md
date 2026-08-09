# Agent image specification

| Document | Status | Covers |
|----------|--------|--------|
| [`v0.1.md`](v0.1.md) | Draft, implemented | OCI artifact profile, media types, config, lockfile, reproducibility, the secret scan, image digest pinning |

The reasoning behind version 0.1 is
[RFC-0012](../../rfcs/0012-the-ingot-package.md). Two constraints were fixed
before it was written and still hold:

- No new registry. A package is a standard OCI image layout, pushed to
  OCI-compatible registries with existing tools.
  See [ADR-0002](../../docs/adr/0002-compiler-not-runtime.md).
- Secrets never enter an artifact. Only references and schemas do.
  See [SECURITY.md](../../SECURITY.md).

The reproducibility groundwork is
[ADR-0004](../../docs/adr/0004-canonical-ir-encoding.md): the IR has one
canonical encoding, and a package carries those bytes rather than re-encoding
them.
