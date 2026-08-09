# Ingot

**Write an agent once, verify it statically, compile it to a portable
artifact, and run it on any compliant runtime.**

Ingot is a statically typed language and compiler for AI agents. It is not
another agent framework and not a runtime: it is the toolchain layer above them.
A single `.ing` source compiles to a target-neutral **Agent IR**, which backends
lower into the configuration a real runtime consumes.

```
ingot init research-agent
ingot check                       # types, effects, policy, budgets
ingot build                       # -> target/ingot/ResearchAgent.ir.json
ingot build --target python       # -> target/ingot/ResearchAgent.py
ingot run --input topic=…         # execute it
```

Status: **pre-1.0, active product-loop development**. The compiler front end is
complete, a reference interpreter executes Agent IR against real model
providers, MCP tools can be discovered and preflighted, and runs can be recorded
to cassettes and replayed offline. A second, independent backend emits
self-contained Python 3 and reports unsupported constructs before build. The
policy-derived boundary now covers tool servers and, with `--contained`, the
agent process itself. The editor-facing authoring stack is usable through the
shared language service, `ingot-lsp`, and a reference VS Code extension.
Language 0.2 adds project-local imports, optional/union type expressions and
expression-only pure helper functions; generics are deliberately deferred until
real source shows the need. OCI packaging and model-assisted authoring remain
planned. See [the roadmap](#roadmap).

---

## Why a language

Agent definitions written in YAML stop scaling once they need control flow,
types, and a story for permissions. Agent definitions written in Python bind the
agent to one framework and one runtime.

A small, statically typed language sits between those. It can answer questions
before the agent ever runs:

```
$ ingot check
error[ING4007]: `web.search` requires the `network` effect, which no policy rule grants
  --> main.ing:31:12
   |
31 |     hits = call web.search(topic)
   |            ^^^^^^^^^^^^^^^^^^^^^^ needs `network`
   |
   = note: Ingot is default-deny: an effect with no rule is denied
   = help: add `network allow [...]` to the agent's `policy` block
```

Things the compiler catches before runtime:

* a tool called with the wrong argument type, or with an argument missing
* a capability the policy denies — or never mentions, which is also a denial
* a prompt placeholder that does not resolve, so `${topci}` cannot render empty
* an output that is never emitted, or emitted with the wrong content type
* a loop with no static bound, or a flow that cannot fit its `steps` budget
* recursion between agents, which would make budgets uncheckable

## What it is not

Ingot deliberately does **not** reimplement the layers that already exist:

| Concern | Ingot's position |
|---------|------------------|
| Tool protocol | MCP. No new tool protocol. |
| Agent-to-agent messaging | A2A. No new messaging protocol. |
| Distribution | OCI registries. No new registry. |
| General execution | Existing runtimes, through backends. |
| Sandboxing | **Ours** — the policy block, enforced. See [ADR-0006](docs/adr/0006-a-policy-enforcing-runner.md). |

The differentiators are compile-time portability, typed effects and
capabilities, a machine-readable target compatibility report, reproducible
artifacts, and a conformance suite backends can be tested against.

The language remains the product surface. Templates, editor support and future
model assistance all create or edit ordinary `.ing`; the compiler, cassette
tests, policy-derived containers and backends form one product loop around that
source. See [RFC-0007](rfcs/0007-the-ingot-product-loop.md).

The one place Ingot does build its own execution machinery is the last row, and
the scope there is narrow on purpose: a **policy-enforcing runner**, not a
general execution engine. [`docs/vision.md`](docs/vision.md) is the full picture
of what the project is for.

## An example

```ingot
language 0.1
package heptapus.examples.research

type search_result {
  title: string
  url: string
  snippet: string
}

tool web.search(query: string) -> search_result[] !network

verifier CitationCheck(draft: markdown, min_sources: int)

/// A realistic research workflow.
agent ResearchAgent(topic: string) -> report<markdown> {
  model requires {
    tool_calling
    structured_output
    context >= 128k
  }

  tools {
    mcp web.search
  }

  budget {
    steps <= 60
    tokens <= 120000
    cost <= 5 usd
  }

  policy {
    network allow ["arxiv.org", "github.com"]
    filesystem_write deny
    external_write deny
    secrets deny export
  }

  flow {
    queries = ask<string[]>("Create diverse research queries for: ${topic}")
    sources = parallel map queries as query {
      call web.search(query)
    }
    draft = ask<markdown>("Produce a source-grounded report.", context: sources)
    verify CitationCheck(draft, min_sources: 8)
    emit report = draft
  }
}
```

Four complete examples live in [`examples/`](examples/).

## Install

Each release carries `ingot`, `ingot-mcp-fs` and `ingot-lsp` for Linux, macOS
(Intel and Apple silicon) and Windows. Download the archive for your platform from
[Releases](https://github.com/mathissdupont/ingot/releases), verify it, and put
the binaries on your `PATH`:

```bash
sha256sum -c SHA256SUMS --ignore-missing
tar -xzf ingot-*-x86_64-unknown-linux-gnu.tar.gz
```

Pre-1.0: the language, the Agent IR and the artifact format may change between
releases.

### From source

Requires a stable Rust toolchain (MSRV 1.85).

```bash
git clone https://github.com/mathissdupont/ingot
cd ingot
cargo build --release
./target/release/ingot --help
```

On Windows, if the repository sits inside a synced folder such as OneDrive, put
the build directory elsewhere — synchronisation and `target/` interact badly:

```bash
export CARGO_TARGET_DIR=/c/build/ingot
```

## Commands

| Command | Purpose |
|---------|---------|
| `ingot init <name> [--template brief\|document-workflow]` | create a tested starter project |
| `ingot check` | parse, type-check, validate policy and budgets |
| `ingot fmt [--check]` | canonical formatting |
| `ingot build [--target ir\|python] [--out-dir]` | compile to Agent IR or self-contained Python 3 |
| `ingot ir [--agent]` | print the IR to stdout |
| `ingot run [--input k=v]` | execute the agent |
| `ingot run --sandbox` | execute it with each tool server inside a boundary |
| `ingot run --contained` | execute the agent itself inside a boundary |
| `ingot image build [SOURCE]` | build the version-matched local image used by contained runs |
| `ingot test` | replay recorded cassettes |
| `ingot doctor [--json]` | report source, provider, MCP and container readiness without starting them |
| `ingot dev [--run]` | watch source, check and build good revisions, optionally run them |
| `ingot tools [--json] [--propose]` | discover, preflight and propose MCP tool declarations/routes |
| `ingot sandbox` | show the boundary each tool server would run inside |
| `ingot explain <CODE>` | explain a diagnostic in full |

Exit codes: `0` success, `1` the program has blocking diagnostics, `2` the
command itself failed. Diagnostics, progress events and status lines go to
stderr; `ingot ir` and `ingot run` write only their payload to stdout, so both
are safe to pipe.

### Check readiness before a run

`ingot doctor` gathers the setup failures that would otherwise appear at
different stages of a live or contained run. It compiles the source, checks
provider selection and credential presence, checks static MCP routes and server
commands, then detects Docker or Podman and the version-matched reference image.
It starts no provider, MCP server or container, and prints environment-variable
names but never their values.

```bash
ingot doctor
ingot doctor --json | jq -e '.ready'
```

Exit code `1` means at least one check failed. Warnings identify facts that a
read-only check cannot prove, such as an MCP server's dynamically published tool
inventory; `ingot tools` performs that live verification. The versioned JSON
contract is documented in
[`docs/doctor-json-v1.md`](docs/doctor-json-v1.md).

### Develop without command juggling

`ingot dev` immediately checks and builds the project, then watches the entry
source and manifest through the operating system's filesystem events. A bad
revision prints the compiler's normal diagnostics, is never built or run, and
leaves the last successful IR in place.

```bash
ingot dev
```

Model execution is deliberately opt-in. Example inputs use the same syntax as
`ingot run`; runs are serialized, so a slow completion can never overlap the
next one:

```bash
ingot dev --run --input topic="compiler design"
ingot dev --run --provider replay --cassette tests/cassettes/example.json \
  --input topic="compiler design"
```

The compact status identifies each source revision and says when an older good
artifact was kept. Run `ingot doctor` first when a live provider or configured
tool is not ready.

### Choosing a model

Which model an agent uses is part of the agent, not part of the command line:

```ingot
agent Brief(topic: string) -> brief<markdown> {
  model exact "openai/gpt-5.1"      // or "anthropic/claude-opus-5"
  ...
}
```

`ingot run` reads the vendor from the artifact and sends the call there, using
whichever keys are exported. An artifact that names a vendor you have no key for
**stops** — it is never answered by a different vendor, because a plausible
answer from the wrong model is the worst outcome available.

| | |
|---|---|
| `anthropic/…` | Messages API. `ANTHROPIC_API_KEY` |
| `openai/…` | Chat Completions. `OPENAI_API_KEY` |

Those two need no configuring. **Anything else you name yourself:**

```toml
# ingot.toml
[[model.provider]]
name = "local"          # the vendor half of `model exact "local/…"`
kind = "openai"         # the protocol it speaks, not the company
base-url = "http://localhost:11434/v1/chat/completions"
# no api-key-env: a server on your own machine usually wants no auth

[[model.provider]]
name = "azure"
kind = "openai"
base-url = "https://….openai.azure.com/openai/deployments/x/chat/completions?api-version=2024-10-21"
api-key-env = "AZURE_OPENAI_KEY"     # a name; a manifest never holds a key
```

```ingot
model exact "local/llama-3.3-70b"
```

`kind` names a **protocol**. Ingot implements two, and Chat Completions is
spoken by Ollama, vLLM, llama.cpp, LM Studio, Azure OpenAI and most hosted
gateways — so "how many providers does Ingot support" is the wrong question.
A declaration may also take over a built-in name, pointing `openai/…` at a
company gateway without editing a single agent.

An agent may instead state what it needs — `model requires { structured_output,
context >= 128k }` — and let the provider pick. Anthropic resolves that against
its default model; OpenAI refuses, because a guessed model name produces a `404`
that reads like a bug here.

### Running an agent

```bash
export ANTHROPIC_API_KEY=...
ingot run examples/document-summarizer   --input document=@report.txt   --input "audience=engineering leads"
```

The declared response type is enforced at run time, not just at compile time:
`ask<string[]>` is sent as a JSON Schema the model is constrained to, and an
answer that does not match the declared type is an error rather than a
best-effort parse. Capabilities, budgets, loop bounds and approval gates are all
re-checked against the artifact's own policy — the person running an artifact is
usually not the person who built it.

Record a run, then replay it with no key and no network:

```bash
ingot run examples/document-summarizer --record tests/cassettes/brief.json --input ...
ingot test examples/document-summarizer
```

A cassette stores the inputs alongside the exchanges, so it is self-contained,
and replay verifies a digest of each request — an edited prompt fails loudly
instead of quietly reusing a stale answer.

A cassette records model exchanges and nothing else, so `ingot test` hosts no
tools: a tool-using agent fails there rather than passing by luck. To get
determinism on the model side *and* real tools, replay a cassette from
`ingot run`, which does host them.

The default event output is a deterministic human trace. Every block retains
the portable event order and adds agent/node provenance, model and tool
boundaries, artifact origins, and observed/final budget progress. Static prompt
text is shown, while dynamic substitutions and named context values are always
marked `<redacted>`; the trace has no secret classification with which to expose
them safely. Use `--events json` for the original unchanged JSON Lines stream or
`--events quiet` for no event output.

```text
trace[0002] node.started demo.Brief:n0  llm.call
             prompt "Write about <redacted input.topic:string>."
             source span unavailable in Agent IR 0.1 (GAP-027)
trace[0003] model.call   demo.Brief:n0  cassette/model -> markdown  (120 in, 60 out)
             observed steps 1/4; tokens 180/20000
```

Portable source spans require the minimal IR addition tracked in
[GAP-027](docs/gaps.md#gap-027) and
[issue #11](https://github.com/mathissdupont/ingot/issues/11); absolute build
machine paths will not be embedded as a shortcut.

### Tools

An `.ing` file declares what a tool is — its parameter types, its result type,
and the effects calling it has. Where that tool comes from is not part of the
program; it is configuration, so the same artifact runs in more than one place:

```toml
# ingot.toml
[[mcp.server]]
name = "workspace"
command = "ingot-mcp-fs"
args = ["--root", "workspace", "--allow-write"]

# Only needed when the artifact and the server use different names.
[mcp.server.tools]
"repo.read_file" = "fs.read_file"
```

```bash
ingot tools           # live routes plus source/schema compatibility
ingot tools --json    # stable discovery data for editors and CI
ingot tools --propose # editable source/manifest proposals; writes nothing
ingot run --input …   # servers start, the agent calls them, they stop
```

The preflight compares each checked `.ing` parameter with the live MCP
`inputSchema`. Missing required parameters, rejected source parameters and
incompatible primitive/list types make the command exit `1`; schema constructs
Language 0.1 cannot prove are reported as `unverified`, never silently treated
as compatible. The JSON report also carries each server's input/output schemas
and required environment-variable **names**, never their values. Its versioned
contract is documented in
[`docs/tools-json-v1.md`](docs/tools-json-v1.md).

For a published tool not yet declared in the program, `--propose` renders a
typed declaration from the required input fields and output schema. MCP cannot
prove which Ingot effects a tool can cause, so every source proposal contains
`!TODO_EFFECT` and cannot pass `ingot check` until the operator replaces it.
Optional parameters and schema features Language 0.1 cannot express are named
beside the snippet. When a declared but unresolved name has the same suffix and
input shape as exactly one published tool, the command also proposes the
specific `[mcp.server.tools]` mapping. It never edits either file.

Tools are served over [MCP](https://modelcontextprotocol.io) by a child process
on stdio. Three independent gates stand between an agent and a file: the
compiler refuses an artifact whose policy does not grant the effect, the runtime
re-checks that policy before every call, and the server enforces whatever bound
it was started with — so an agent whose policy says `filesystem_read allow ["."]`
still cannot read `../secret.txt`.

A tool server starts with a **minimal environment**. Nothing you exported
reaches it unless `pass-env` names it, and `pass-env` takes names, never values:
a manifest is committed, and a secret in a committed file is a published secret.

[`ingot-mcp-fs`](crates/ingot-mcp/) is a small sandboxed filesystem server that
ships with the repository, so a fresh checkout can run a tool-using agent
without installing anything else:

```bash
cargo install --path crates/ingot-mcp
ingot tools examples/repo-digest
```

Details: [MCP binding 0.1](specs/tools/mcp-v0.1.md).

### The policy block as a boundary

A `policy` block is checked by the compiler and re-checked by the runtime, and
both checks answer the same question: *may this call have this effect?* Neither
can answer *where did it go?* — a tool server is a separate process with the
operator's filesystem and the operator's network.

`ingot sandbox` derives, from the same policy, the boundary that server should
run inside:

```text
$ ingot sandbox examples/code-review-team
workspace  /home/me/ingot

server `repo`  (for agent heptapus.examples.review.SecurityReviewer)
  mount    /workspace/crates  ro   filesystem_read allow ["crates"]
  network  none
  workdir  /workspace

server `repo`  (for agent heptapus.examples.review.CodeReviewTeam)
  mount    /workspace/crates         ro   filesystem_read allow ["crates"]
  mount    /workspace/target/review  rw   filesystem_write allow ["target/review"]
  network  none
  workdir  /workspace

  cannot enforce:
    external_write require approval
      a boundary cannot tell an intended external write from any other;
      the effect check, and any approval gate, still apply
```

One plan per **agent**, not per server: the sub-agent may read and the
coordinator may write, and a box wide enough for both would hand the sub-agent a
grant its own policy denies.

A policy path is relative to the **workspace** — a root the operator binds with
`--workspace`, defaulting to the project. The artifact says `crates`; the
operator says where `crates` lives, so the same artifact means the same thing on
two machines.

What a boundary **cannot** enforce is named rather than glossed over, and
`ingot run --sandbox` refuses to start when anything is unenforced.

`--sandbox` turns the plan into a real boundary: each tool server runs in a
container with those mounts, that network, no capabilities, a read-only root
filesystem, and only the environment variables `pass-env` named.

```bash
docker build -f tools/mcp-fs.Dockerfile -t ingot/mcp-fs:0.2 .
ingot run examples/repo-digest --sandbox --input directory=. --input out=out/digest.md
```

Every run says which it is:

```text
tool servers run contained, by docker 28.3.2; the policy is enforced
tool servers run as child processes; the policy is checked, not enforced
```

The image is the operator's choice — `image` on each `[[mcp.server]]` — because
the server is the operator's program. Without one, `--sandbox` says so instead
of running the server loose.

See [RFC-0004](rfcs/0004-ingot-containers.md) and
[ADR-0006](docs/adr/0006-a-policy-enforcing-runner.md). What the boundary
actually grants is asserted against a real runtime in
[`crates/ingot-sandbox/tests/container.rs`](crates/ingot-sandbox/tests/container.rs):
a read mount refuses a write, an unnamed path does not exist inside, and
`network deny` leaves no interface at all.

### Putting the agent in the box too

`--sandbox` contains an agent's *tools*. The agent itself — the process that holds
the API key, renders the prompts and writes the artifacts — still runs on the
host with the host's whole machine. `--contained` closes that:

```bash
ingot image build
ingot run examples/repo-digest --contained \
  --input directory=. --input out=out/digest.md
```

`ingot image build` finds the nearest Ingot source checkout, verifies that its
workspace version matches the running binary, and builds `ingot/run:<version>`
with the available Docker or Podman daemon. A contained run selects that exact
local tag when neither `[run] image` nor `--image` deliberately names a custom
deployment image. It never pulls an image automatically and a missing boundary
never falls back to a host run. Verified remote acquisition remains part of M6.

Everything is inside: the interpreter, its tool servers, and the mounts the policy
named. Nothing else. The model call and the approval gate leave through a
supervisor on the standard streams:

```text
host                                  inside the boundary
────                                  ───────────────────
ingot run --contained                   the interpreter
  holds the credential                  the MCP tool servers
  holds the terminal                    network deny, and it holds
  writes --out-dir

  ├── the IR, the inputs ───────────►
  │◄─ a completion ──────────────────┤   fetched from outside
  │◄─ an approval question ──────────┤   asked inside, decided outside
  │◄─ progress, then the outputs ────┤
```

Two things follow from the shape rather than from care:

* **`network deny` now applies to the agent**, not only to its tools. It gets
  `--network none` and still completes a model call, because that call does not
  use a socket.
* **The credential is never inside.** The provider lives out here; the box has no
  environment for a key to be read from, and no route to the process that has one.
  This is [Runtime 0.1 §11](specs/runtime/v0.1.md) satisfied by topology.

`--out-dir` is written by the host afterwards, from the outputs that came back, so
the agent cannot write outside its mounts even to deliver its own result.

One limit worth knowing before you rely on it: a program whose agents want
*different* boundaries is refused rather than run in the widest of them. The
two-agent example is exactly that case — the coordinator may write and the
reviewer may not — so it needs `--sandbox` for now
([GAP-023](docs/gaps.md#gap-023)).

See [RFC-0005](rfcs/0005-the-contained-run.md) and
[ADR-0007](docs/adr/0007-containing-the-run-is-not-blocked-on-a-second-backend.md).

## How it fits together

```
        main.ing
           │
           ▼
   lexer → parser → AST                        ingot-lexer, ingot-parser, ingot-syntax
           │
           ▼
   name resolution → types → effects/policy    ingot-semantic, ingot-types
           │
           ▼
       lowering                                ingot-compiler
           │
           ▼
      Agent IR (canonical JSON)                ingot-ir
           │
     ┌─────┴──────────┐
     ▼                ▼
  interpreter     Python backend               ingot-runtime, ingot-backend-python
     │                │
     ▼                ▼
  execution      portability report            implemented: M5
     │
     ▼
  MCP servers (stdio)                          ingot-mcp
```

With `--sandbox` the MCP servers move inside a policy-derived boundary
(`ingot-sandbox`). With `--contained` the interpreter moves in with them, and
`ingot-supervisor` is the channel it reaches the model and the operator through.

| Crate | Responsibility |
|-------|----------------|
| `ingot-source` | files, spans, line/column resolution |
| `ingot-diagnostics` | diagnostic model, stable codes, terminal renderer |
| `ingot-lexer` | tokens; never fails, always resynchronises |
| `ingot-syntax` | AST and the canonical printer behind `ingot fmt` |
| `ingot-parser` | recursive descent with error recovery |
| `ingot-types` | types, effects, policy subjects and decisions |
| `ingot-semantic` | resolution, type checking, effect and policy analysis |
| `ingot-ir` | the Agent IR model and its canonical encoding |
| `ingot-compiler` | the driver and lowering |
| `ingot-runtime` | the reference interpreter, providers and cassettes |
| `ingot-backend-python` | self-contained Python 3 emission and portability reports |
| `ingot-language-service` | editor-neutral diagnostics, formatting, completion, hover and definition over the compiler |
| `ingot-lsp` | stdio language server adapter for editor diagnostics, formatting and navigation |
| `ingot-mcp` | the MCP tool host, and the `ingot-mcp-fs` reference server |
| `ingot-sandbox` | a `policy` block turned into a container boundary |
| `ingot-supervisor` | the channel between a contained run and the host serving it |
| `ingot-cli` | the `ingot` binary |

## Specifications

* [Language 0.1](specs/language/v0.1.md) — syntax and static semantics
* [Agent IR 0.1](specs/ir/v0.1.md) — the backend contract
* [Runtime 0.1](specs/runtime/v0.1.md) — what executing an artifact means
* [MCP binding 0.1](specs/tools/mcp-v0.1.md) — how a declared tool is served
* [`agent-ir.schema.json`](specs/ir/agent-ir.schema.json) — machine-readable schema
* [Vision](docs/vision.md) — what the project is for, and where it is going
* [Architecture](docs/architecture/overview.md) — how the phases fit together
* [Language service](docs/language-service.md) — editor-facing diagnostics, formatting, completion and navigation
* [Decision records](docs/adr/) — why the load-bearing choices were made
* [Gap register](docs/gaps.md) — every known limitation, with an identifier

## Roadmap

| Milestone | Scope | State |
|-----------|-------|-------|
| M0 | scope, prior art, non-goals | done |
| M1 | grammar, parser, diagnostics, formatter | done |
| M2 | types, effects, policy, budgets, Agent IR | done |
| M3 | reference interpreter, `ingot run`, end-to-end execution | done |
| M4 | cassette record and replay, `ingot test`, MCP tool host | done |
| M5 | a second backend and the portability report | done |
| M6 | OCI artifact, lockfile, reproducible digest | planned |
| M7 | language server and editor support | done |
| M8 | conformance suite and backend author guide | planned |
| M9 | Ingot Containers — the policy block as an enforced boundary | done |
| M10 | `ingot new` — authoring with a model, verified by the compiler | planned |
| M11 | integrated `.ing` product loop: templates, `dev`, trace, readiness and safe-run UX | done |

A number is an identity, not a position in a queue; things get referenced by it,
so they keep it. The remaining proposed order is **IR 0.2 source spans → M10 →
M6 → M8**. [RFC-0007](rfcs/0007-the-ingot-product-loop.md) explains why the
usable language loop comes before generation and packaging.

M3 and M4 landed together: the interpreter needed cassettes to be testable, and
cassettes needed the interpreter to be worth recording. M9 landed in two stages —
tool servers contained ([RFC-0004](rfcs/0004-ingot-containers.md)), then the run
itself ([RFC-0005](rfcs/0005-the-contained-run.md)) — with one piece still open,
[GAP-023](docs/gaps.md#gap-023). M5 closed [GAP-018](docs/gaps.md#gap-018): the
same artifact and cassette now run through two independent backends, and every
unsupported construct is named in a portability report before deployment.
M11 and the Language 0.2 reuse slice are complete: first-use templates,
`doctor`, `dev`, human traces, contained-run readiness, editor/LSP support,
typed tool onboarding, imports, optional/union types and pure helpers all now
exist. [Issue #11](https://github.com/mathissdupont/ingot/issues/11) is the next
trace-quality follow-up: carrying portable source spans in Agent IR.

## What is missing

Every known limitation has an identifier and an entry in the
**[gap register](docs/gaps.md)** — what is missing, how it shows up, why it is
not done, and what closing it would take. Read it before relying on anything
here.

The five worth knowing before you write an agent:

| | |
|---|---|
| [GAP-001](docs/gaps.md#gap-001) | `network allow ["arxiv.org"]` does not constrain which hosts a tool reaches. The decision is enforced; the list is not. |
| [GAP-002](docs/gaps.md#gap-002) | `verify` evaluates its arguments and reports `passed: true` without a verifier existing to have checked anything. |
| [GAP-006](docs/gaps.md#gap-006) | Cassettes record model exchanges only, so `ingot test` cannot test a tool-using agent. |
| [GAP-017](docs/gaps.md#gap-017) | The differential tests are not yet a packaged conformance suite or backend author guide. |
| [GAP-025](docs/gaps.md#gap-025) | Authoring, checking, tracing, testing and safe execution are separate manual workflows. |

The first two of those are **unenforced**: they look like guarantees and are
not. Everything else in the register either fails loudly or cannot be expressed
at all.

## Contributing

Read [CONTRIBUTING.md](CONTRIBUTING.md). Changes to the language, the IR or the
artifact format go through the [RFC process](rfcs/); everything else can start
as an issue.

## Licence

Apache-2.0. See [LICENSE](LICENSE).

`Ingot` is a working name. Trademark, domain and package-registry clearance has
not been carried out, and must be completed with legal review before any public
release under this name.
