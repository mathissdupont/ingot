# RFC-0003: MCP tool host

- Status: **Accepted**
- Created: 2026-08-07
- Affects: runtime spec, CLI, manifest format, `file` value representation

## Problem

After M4 an agent can think but not act. The language has had `tool` since 0.1:
a declaration with typed parameters, a typed result, and the effects that
calling it has. The compiler checks every call against that declaration, checks
the effects against the agent's policy, and lowers a `tool.call` node into the
artifact. The runtime re-checks all of it and then asks a `ToolHost` — and the
only host that exists refuses everything.

The consequence is visible in the examples. Of the three shipped before this
RFC, one runs; the other two compile, type-check, produce valid IR, and stop at
their first `call` with "no host provides the tool". Everything about tools is
implemented except the part that does anything.

The wider problem is that the effect system is unfalsified. `!network` on a tool
declaration, `filesystem_write allow ["target/review"]` in a policy — these are
claims about what an agent can reach, and no run has ever tested one against a
tool that could actually reach something.

## Goals and non-goals

**Goals**

1. An Ingot agent calls tools on any MCP server, over stdio, at run time.
2. Where a tool comes from is **operator** configuration, not part of the
   artifact: the same artifact runs against different servers unrecompiled.
3. A tool result is converted into a value of the declared Ingot type, or the
   run stops. No coercion, no best-effort parse.
4. A tool server starts with a minimal environment. No credential reaches one
   because the operator happened to have it exported.
5. A reference server ships, so a fresh checkout can run a tool-using agent
   without trusting anything from anywhere else.
6. `ingot tools` reports which server serves each declared tool, and fails when
   one is unserved.

**Non-goals for this RFC**

* **HTTP and SSE transports.** Reaching a remote MCP server is itself a
  `network` effect, and Ingot has no way yet to say "this agent may talk to
  *this* server but not the wider internet". Designing that properly is its own
  RFC; shipping it badly would put a hole in the effect system.
* **Prompts, sampling and resources.** An Ingot agent's prompts are compiled
  into its artifact. A server asking to sample from the agent's model would be
  an effect nothing declared. Resources have no type in the language yet.
* **Recording tool results in cassettes.** See "What this leaves undone".
* **Tool discovery driving the compiler.** The artifact's declaration stays the
  source of truth. A server's `inputSchema` is not consulted to type-check a
  call, because the artifact must be checkable without any server running.

## Where MCP fits

The research this project came from concluded that a tool protocol is not worth
reinventing, and MCP is the one with adoption. Nothing here changes that: Ingot
defines no tool protocol. What it defines is the *declaration* — name,
parameter types, result type, effects — and the check that a call matches it.

This RFC is the binding between the two. It is deliberately thin, and it lives
in its own crate (`ingot-mcp`) so that the runtime keeps no dependency on it: a
backend that hosts tools some other way replaces this crate and nothing else.

## Design

### 1. Configuration lives in the manifest, not the artifact

```toml
[mcp]
timeout-seconds = 30

[[mcp.server]]
name = "workspace"
command = "ingot-mcp-fs"
args = ["--root", "workspace", "--allow-write"]
pass-env = ["BRAVE_API_KEY"]

[mcp.server.tools]
"repo.read_file" = "fs.read_file"
```

The artifact says *what* it needs and what that is allowed to do. The manifest
says *where it comes from on this machine*. Keeping them apart is what makes an
artifact portable: the same `CodeReviewTeam.ir.json` runs against a local
filesystem server on a laptop and a hosted one in CI.

`[mcp.server.tools]` maps the artifact's name onto the server's. It is needed
only when they differ, so most entries need no map at all.

Unknown keys are **rejected**, not ignored. In particular there is no `env` key
that takes literal values — see §3.

### 2. Routing is decided once, and ambiguity is an error

At connect time each server is asked for its tool list. Explicit aliases are
applied first, then remaining names match identically. If two servers would
serve the same tool, the connection fails naming both. Which server answers must
not depend on which one started first.

A server whose alias map covers none of the tools this program declares is never
started. With no map, its tool list is the only way to know what it offers, so
it is started.

### 3. A tool server starts with a minimal environment

`env_clear()`, then a fixed list of platform-essential variables (`PATH`,
`SystemRoot`, `HOME`, and so on — a subprocess with no environment at all is
broken on every platform), then whatever `pass-env` names.

`pass-env` takes **names**. There is deliberately no way to write a value into
the manifest, because a manifest is committed, and a secret in a committed file
is a published secret. This is the same rule the runtime already applies to
model credentials, extended to tools.

### 4. Result conversion is type-directed

MCP returns content blocks for a model to read, plus an optional
`structuredContent` object for a program to read. The artifact declared a type,
so:

| Declared type | Source |
|---------------|--------|
| `text`, `markdown`, `string`, `bytes` | the text blocks, joined, verbatim |
| a record, `json`, `file` | `structuredContent` as it stands, else the text parsed as JSON |
| a scalar or a list | `structuredContent.value`, else the text parsed as JSON |

MCP requires `structuredContent` to be an object, so a tool whose Ingot result
type is *not* object-shaped returns it under `value` — the same convention the
model side already uses for `ask<string[]>`.

`isError: true` is a tool that ran and failed: the run stops with the server's
message. A JSON-RPC error is a call that was malformed: the run stops saying so.
The two are not conflated, because one is the agent's problem and the other is
the operator's.

The converted value is then validated against the declared type by the
interpreter, exactly as a model response is. Conversion chooses a source; it
never widens a type.

### 5. `file` gets a runtime representation

`file` has been a type since 0.1 with no defined JSON form, which no artifact
noticed until a tool returned one. It is a **handle**:

```json
{ "path": "out/summary.md" }
```

`path` is required and must be a string; a producer may add anything else, such
as a media type or a size. The content is deliberately absent: a file exists so
that bytes can move between tools *without* passing through the agent, the event
stream, or a cassette.

`bytes` is a base64 string, for the same reason — the IR, events and cassettes
are all JSON.

### 6. Reads have a deadline and stderr is kept

A server that accepts a request and never answers must not hang a run. Child
pipes have no read timeout, so reading happens on its own thread and the client
waits on a channel with a deadline.

When a server dies, its last words on standard error are the only useful thing
in the failure message, so the last twenty lines are kept and included.

### 7. A reference server ships

`ingot-mcp-fs` exposes one directory over stdio: `fs.read_file`, `fs.list_dir`,
and — only with `--allow-write` — `fs.write_file`. `--root` is required; it
refuses to guess what to expose.

It exists for two reasons. A fresh checkout needs something to point
`[[mcp.server]]` at before installing anything from anywhere else, and the
integration tests need a real subprocess, so that the path under test is the
path that ships.

Its sandbox is enforced twice, because either check alone is insufficient: a
component scan refuses `..`, and the resolved path is compared against the
canonicalised root, which refuses the same escape smuggled through a symlink.

## Two sandboxes, and why both

A read from `ingot-mcp-fs` passes three independent gates:

1. the **compiler**, which refused to build the artifact unless its policy
   granted `filesystem_read`;
2. the **runtime**, which re-checks that grant against the artifact's own policy
   before the call, because the person running an artifact is often not the
   person who built it;
3. the **server**, which refuses anything outside `--root` whatever the artifact
   says.

They are not redundant. The first two are claims the artifact makes about
itself; the third is a limit the operator imposes on it. An agent whose policy
says `filesystem_read allow ["."]` still cannot read `../secret.txt`, and there
is a test that asserts exactly that.

## Alternatives considered

**Build tool support into `ingot-runtime`.** Rejected. The runtime would gain
process spawning and a protocol implementation, and every backend author would
inherit MCP whether or not they use it. A separate crate keeps `ToolHost` the
only thing the runtime knows about tools.

**Let the server's `inputSchema` type-check calls.** Rejected. `ingot check`
must work with nothing running. Compile-time checking against a run-time
discovery would make "it compiles" depend on what happened to be installed.

**Match tools by fuzzy name.** Rejected. `repo.read_file` and `fs.read_file` are
mapped explicitly or not at all. Guessing at deployment time is how an agent
ends up calling something nobody intended.

**Inherit the parent environment.** Rejected, and it is the default everywhere
else, which is why it is worth stating: an agent framework that hands every
exported credential to every tool server is a credential-exfiltration primitive
with a plugin system.

## What this leaves undone

Each of these has an entry in the [gap register](../docs/gaps.md), which is
where their current state is tracked; this section is why they were left.

* **`ingot test` hosted no tools** ([GAP-006], since closed). A cassette
  recorded model exchanges and nothing else, so a tool call during replay would
  have had to reach a real server — and a test that touches the filesystem is not
  the offline, repeatable thing `ingot test` promises. Cassette 0.2 records the
  invocations and their results instead
  ([Runtime 0.2 §2](../specs/runtime/v0.2.md)), which was indeed the obvious next
  piece of work and indeed a format change.
* **Remote servers** ([GAP-007]). Only stdio, per the non-goals.
* **Per-call effect narrowing** ([GAP-013]). A tool's effects are declared once
  for the tool. There is no way to say "this call reads only `docs/`" beyond
  what the server's root already gives — and note that the policy allowlist does
  *not* give it either ([GAP-001]).

[GAP-001]: ../docs/gaps.md#gap-001
[GAP-006]: ../docs/gaps.md#gap-006
[GAP-007]: ../docs/gaps.md#gap-007
[GAP-013]: ../docs/gaps.md#gap-013

`ingot run --provider replay --cassette …` *does* work with live tools, which is
how the end-to-end tests get determinism on the model side and reality on the
tool side.

## Compatibility

Additive. Language 0.1 is unchanged: no new syntax, no new keywords, no IR
change. An artifact built before this RFC runs identically. A manifest without
an `[mcp]` section behaves exactly as before — nothing is hosted, and an agent
that needs a tool stops at the call.

The one behaviour change is that `validate` now accepts `file` and `bytes`
values instead of reporting "unknown type", which could only ever have been a
failure before.

## Migration

None required. To make a tool-using agent run, add a server to `ingot.toml` and
check the wiring with `ingot tools`.
