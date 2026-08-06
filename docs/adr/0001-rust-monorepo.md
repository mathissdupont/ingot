# ADR-0001: Rust monorepo with one responsibility per crate

- Status: Accepted
- Date: 2026-08-06

## Context

The toolchain is a compiler that ships as a single binary and, later, as a
library other tools embed: a language server, editor extensions, possibly
foreign-function bindings. It needs fast startup, predictable memory behaviour
and a strong parsing and data-modelling story. Go was the main alternative,
being closer to the OCI and container ecosystem the packaging milestone will
touch.

## Decision

Rust, in a single Cargo workspace, with one crate per compiler responsibility.

Crate boundaries are enforced, not decorative:

| Crate | Knows about | Must not know about |
|-------|-------------|---------------------|
| `ingot-source` | files, spans | syntax |
| `ingot-diagnostics` | diagnostics, rendering | any compiler phase |
| `ingot-lexer` | tokens | the AST |
| `ingot-syntax` | the AST, printing | types, targets |
| `ingot-parser` | syntax to AST | types, targets |
| `ingot-types` | types, effects, policy | syntax, targets |
| `ingot-semantic` | resolution and checking | any specific runtime |
| `ingot-ir` | the IR model and encoding | providers, syntax |
| `ingot-compiler` | driving and lowering | provider SDKs |
| `ingot-cli` | the command line | compiler internals |

## Rationale

**Rust over Go.** The differentiator is the compiler, not the container
integration. Algebraic data types and exhaustive matching make an AST and an IR
pleasant to model and refactor; adding a node kind produces a compile error at
every place that must handle it. A single static binary with no runtime keeps
distribution simple. WASI is a natural target for the plugin sandbox in the
long-term plan.

**Monorepo over split repositories.** The language, the IR and the backends move
together during the early milestones. An IR change should be one reviewable
commit that updates the spec, the model, the lowering and the golden files at
once. Splitting them now would buy version-skew problems and no isolation.

**Many small crates over one.** The boundaries make architectural drift a
compile error rather than a code review argument. If `ingot-parser` wants to
know whether a target supports typed state, the dependency does not exist and
the design problem surfaces immediately. It also keeps compile times and test
runs proportional to what changed.

**Not two implementations.** The research document floated prototyping in
TypeScript and porting later. Two implementations of an unstable language would
diverge, and the second would inherit the first's accidents. One implementation,
with a specification written alongside it, is the cheaper path.

## Consequences

- Contributors need Rust. That narrows the contributor pool; the specification
  and the JSON schema exist so that backend authors do not.
- Adding a crate has ceremony: a manifest, a workspace entry, doc comments. This
  is intentional friction against ad-hoc modules.
- Cross-crate refactors touch more files than they would in one crate.
- On Windows, `target/` inside a synced folder such as OneDrive is painfully
  slow. `CARGO_TARGET_DIR` is documented in the README and CONTRIBUTING.
