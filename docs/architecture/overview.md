# Architecture

How a `.ing` file becomes an Agent IR document, and where each guarantee is
established.

## The pipeline

```
  main.ing
     │
     │  ingot-source        files, byte spans, line/column resolution
     ▼
  tokens                    ingot-lexer      never fails; emits an error token
     │
     │  ingot-parser        recursive descent with error recovery
     ▼
  AST                       ingot-syntax     also the canonical printer
     │
     │  ingot-semantic      four passes, see below
     ▼
  Analysis                  symbol tables + per-expression types and effects
     │
     │  ingot-compiler      lowering
     ▼
  Agent IR                  ingot-ir         canonical JSON
     │
     │  ingot-runtime       the reference interpreter
     ▼
  Execution                 providers, cassettes, events
     │
     │  ingot-mcp           tool calls, over stdio
     ▼
  MCP servers               child processes the operator configured
```

Each stage is a separate crate, and the dependency graph is acyclic in the
direction shown. `ingot-parser` cannot see types; `ingot-semantic` cannot see
targets; `ingot-ir` cannot see syntax. Those are compile-time facts, not
conventions — see [ADR-0001](../adr/0001-rust-monorepo.md).

The last two are the same discipline applied downwards. `ingot-runtime` knows
about `ToolHost`, a three-method trait, and nothing about MCP; `ingot-mcp`
depends on the runtime, never the reverse. A backend that hosts tools some other
way replaces one crate — see [ADR-0005](../adr/0005-mcp-over-stdio-only.md).

## Where guarantees come from

| Guarantee | Established by |
|-----------|----------------|
| Many errors reported per run | parser recovery: every delimiter loop stops at a declaration barrier and always consumes a token |
| One canonical formatting | `ingot-syntax::printer`, tested for idempotence |
| Types and tool signatures | `ingot-semantic` pass 4 |
| Default-deny capabilities | `check_effects_against_policy`, at each call site |
| Static bounds | `min_steps` and the loop-bound check |
| No recursion | `ingot-semantic` pass 3, over the agent call graph |
| Byte-stable artifacts | `ingot-ir`: sorted maps, decimal strings, fixed field order |

## The lexer never fails

Malformed input produces a diagnostic and an `Error` token, then lexing
continues. There is no error path that stops tokenizing, which is what lets the
parser see a whole file even when part of it is broken.

Two lexical details worth knowing:

- `128k` is `131072`. The suffix is recognised only when the next character
  cannot continue an identifier, so `2kb` is `2` followed by `kb`.
- A string token carries the **raw** text between the quotes. Escape resolution
  and `${...}` splitting happen in the parser, which needs exact byte offsets so
  every placeholder gets its own span.

## Parser recovery

Two mechanisms, both necessary:

**Barriers.** Every loop bounded by a delimiter also stops at `type`, `tool`,
`verifier` or `agent`. Without this, a missing `)` lets a recovery loop swallow
the rest of the file and hide every later declaration.

**Guaranteed progress.** Every loop records the cursor position and consumes a
token if a sub-parser did not. The parser cannot hang on malformed input, and a
test feeds it truncated files to prove it.

Unparseable statements and expressions become `Stmt::Error` and `Expr::Error`
rather than disappearing, so the formatter and the future language server keep
working on half-written files.

## The four semantic passes

Declaration order in the source never matters, which is why there are four:

1. **Records, tools, verifiers.** Record field types resolve in a second sweep so
   records may reference each other.
2. **Agent signatures.** Inputs, outputs, and an effect upper bound derived from
   the tools each agent grants. Having the bound before any flow is checked is
   what lets an agent call one declared later in the file.
3. **The call graph.** Recursion is detected and reported once, before any flow
   walks into it.
4. **Each agent.** Sections, then the flow.

### The analysis side tables

Rather than building a second typed tree, the checker records what it learned in
maps keyed by source span:

| Table | Contents |
|-------|----------|
| `exprs` | type and effects of every expression |
| `calls` | what each `call` resolved to, plus normalised argument order |
| `verifies` | the resolved verifier and its argument order |
| `interpolations` | the type of each resolved `${...}` placeholder |

Spans are unique per expression within a compilation, which makes them usable as
keys. Lowering reads these tables instead of re-deriving anything, so a
program's meaning is decided in exactly one place.

## Lowering

Five transformations turn a checked AST into IR:

**Flatten.** All nodes go into one array with `next` pointers. Regions — branch
arms, loop and map bodies — are referenced by the id of their first node and
terminate with `next: null`.

**Hoist.** `ask("...", context: call t(x))` becomes a `tool.call` bound to
`$tmp0` followed by an `llm.call` referring to it. A node's arguments are always
pure values.

**Inline pure bindings.** `x = topic` produces no node; uses of `x` resolve to
the input. Only nodes that do work survive.

**Make state explicit.** A read of `state.notes` emits a `state.read` node bound
to `$state.notes`, once per field per statement. State access is therefore
auditable in the artifact, and a backend without state support fails at a
precise point.

**Insert approvals.** A call whose effects include one the policy marks
`require approval` is preceded by an `approval` node listing exactly those
effects.

Node ids are `n0`, `n1`, … in creation order. Because a region's body is lowered
before its container node is pushed, body nodes can have lower ids than their
container — ids record creation order, not execution order.

## Diagnostics

Diagnostics are data, not strings: a stable code, a severity, a primary label,
optional secondary labels, notes, and help. The terminal renderer and the future
language server consume the same structure.

Codes are grouped by area (`ING1xxx` parsing through `ING6xxx` lowering) and are
never reused for a different meaning. `ingot explain <CODE>` prints the long
form for the codes where the rule is not self-evident.

Unknown names get a suggestion when one is within a length-aware edit-distance
threshold — three characters for a long name, one for a short one, so short
names do not produce nonsense suggestions.

## Testing

| Layer | Where | What it protects |
|-------|-------|------------------|
| Unit | in each crate | one rule each, named after the behaviour |
| Behaviour | `ingot-semantic/src/tests.rs` | every diagnostic code has a test |
| Structural | `ingot-compiler/src/tests.rs` | lowering shape: hoisting, regions, approvals |
| Golden | `tests/golden-ir/` | the IR of the reference examples, byte for byte |
| End to end | `ingot-cli/tests/cli.rs` | commands, exit codes, stdout/stderr split |
| Consistency | `golden_ir.rs` | the published schema matches the Rust model |

The golden and consistency layers are the ones that catch drift. Everything else
catches mistakes.

## What is not built yet

A second backend and the portability report, OCI packaging, the lockfile, and
the language server. The [roadmap](../../README.md#roadmap) has the sequence;
[ADR-0002](../adr/0002-compiler-not-runtime.md) explains why execution is
delegated rather than implemented, and why a reference interpreter exists
anyway.

Within what is built, the known limitations are catalogued with stable
identifiers in the [gap register](../gaps.md) — including the two that are
*unenforced* rather than merely absent, and are therefore the ones to read
first.
