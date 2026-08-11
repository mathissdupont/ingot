# Security Policy

## Reporting a vulnerability

Do not open a public issue.

Email **contact@sametunsal.com** with a description, reproduction steps and
the affected version. Expect an acknowledgement within three working days and an
assessment within ten. We will agree a disclosure timeline with you and credit
you in the release notes unless you prefer otherwise.

## What counts as a vulnerability here

Ingot is a compiler, so its security surface is mostly about **what a compiled
artifact is allowed to reach**. Treat the following as security issues:

* **Capability escape** — a program reaches an effect its `policy` block does
  not grant, or the checker fails to reject a call that needs a denied effect.
* **Default-deny bypass** — an effect with no policy rule is treated as
  permitted anywhere in the pipeline.
* **Approval bypass** — a call whose effects are marked `require approval`
  reaches the IR without a preceding `approval` node.
* **Secret exposure** — any path by which a secret value could be written into
  source, the IR, an artifact, a lockfile or a log.
* **Path traversal** — a filesystem allowlist that can be escaped, for example
  through `..` or a symlink.
* **Lowering divergence** — the IR grants more than the source states, or a
  backend can silently ignore a restriction.
* **Tool host leakage** — a tool server started by Ingot receiving an
  environment variable that its `pass-env` does not name, or a way to write a
  literal credential into a manifest.
* **Tool routing confusion** — a call reaching a server the operator did not map
  it to, or an ambiguous route resolved silently instead of refused.
* **Compiler denial of service** — input that makes the compiler hang, exhaust
  memory, or crash. The parser is designed never to loop; a counterexample is a
  bug.
* **Supply chain** — a dependency compromise affecting released binaries.

## What does not count

* An agent producing wrong or harmful *content*. That is a model behaviour
  concern, not a compiler one.
* A tool doing something harmful when it was explicitly granted the capability.
  Ingot's job is to make the grant visible and required, not to second-guess it.
* An MCP server exceeding what Ingot asked it to do. A server is a program the
  operator chose to run, with the arguments and environment the operator gave
  it; Ingot cannot sandbox an arbitrary process and does not claim to. Reports
  about `ingot-mcp-fs` specifically — the server this repository ships — *do*
  count, including any way past its `--root`.
* Warnings that do not fail the build, when documented as warnings.

## Design commitments

These are properties we treat as invariants; a report that breaks one is
actionable regardless of severity.

**Default-deny.** An effect is available only when the policy grants it. An
absent rule is a denial with its own diagnostic (`ING4007`), never a permission.

**Secrets never enter an artifact.** Secret values do not appear in source, IR,
lockfiles or OCI layers. Only references and schemas do. `ingot build` and
`ingot package` scan source, the compiled IR and every cassette for
credential-shaped values and refuse rather than warn
([Ingot Package 0.1 §8](specs/image/v0.1.md)). The scan is a check on the author:
what makes the commitment hold is that there is no *path* from the environment
into an artifact, and the scanner does not replace that.

**Effects are explicit.** Every tool declares what it can do. A call's effects
are checked at the call site. A sub-agent cannot exceed the union of the effects
of the tools it grants.

**Bounds are static.** Loops carry a maximum, recursion is rejected, and a flow
that cannot fit its step budget is rejected. Unbounded cost is a safety problem,
not just an economic one.

**Backends reject rather than skip.** An unknown node kind, an unsupported
policy decision or an unimplemented IR major version must stop a backend. A tool
no host serves must stop the run rather than being skipped. Skipping a node is
how a restriction gets lost.

**A tool server is told, never asked.** A server starts with a cleared
environment plus the platform essentials and whatever `pass-env` names. There is
no configuration key that takes a literal environment value, because a manifest
is committed. Where a tool comes from is the operator's decision and is recorded
outside the artifact.

## Supported versions

Pre-1.0. Only the latest release receives fixes. Once 1.0 ships, this section
will state a support window per minor version.

## Dependencies

Dependencies are kept few and permissive. `cargo deny check` runs in CI over
advisories, licences, sources and duplicate versions. Adding a dependency to a
core crate needs justification in the pull request, including its licence and
its transitive footprint.
