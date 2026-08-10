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
real source shows the need. `ingot new` can hand authoring to a model and have
the compiler verify what comes back, on a new project or as a diff against an
existing one. `ingot package` writes the checked artifact as a reproducible OCI
package with a lockfile, and a build-time scan refuses a credential before it can
leave the machine. See [the roadmap](#roadmap).

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
| `ingot new [--out-dir dir] "workflow words..."` | create a compiler-verified project from a workflow description |
| `ingot new --provider auto "workflow words..."` | the same, with a model writing the source and the compiler verifying it |
| `ingot new --project dir --provider auto "what to change" [--apply]` | propose a source diff for an existing project; writes nothing without `--apply` |
| `ingot new --previous old.ing --candidate proposed.ing [--repair-candidate fixed.ing]` | review model-proposed source, run bounded compiler repair and separate policy requests |
| `ingot check` | parse, type-check, validate policy and budgets |
| `ingot fmt [--check]` | canonical formatting |
| `ingot build [--target ir\|python] [--out-dir]` | compile to Agent IR or self-contained Python 3 |
| `ingot package [--report python] [--out-dir]` | write the checked artifact as an OCI package with a lockfile and a reproducible digest |
| `ingot package --verify` | report every source, agent and field that moved since the package was written |
| `ingot ir [--agent]` | print the IR to stdout |
| `ingot run [--input k=v]` | execute the agent |
| `ingot run --sandbox` | execute it with each tool server inside a boundary |
| `ingot run --contained` | execute the agent itself inside a boundary |
| `ingot image build [SOURCE]` | build the version-matched local image used by contained runs |
| `ingot test` | replay recorded cassettes, tool results included |
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

### Authoring with a model

`ingot new` turns a workflow description into a project. Without `--provider` it
writes a maintained offline template and makes no model call at all. With one, a
model writes the source and the compiler decides whether it is any good:

```bash
ingot new --out-dir audience-brief --provider auto \
  "summarise a document for a named audience"
```

What comes out is an ordinary project — `main.ing`, `ingot.toml`, a README,
example inputs — and the authoring model has no further part in it. `check`,
`build`, `test` and `run --provider replay` never call one.

On a project that already exists, the model proposes rather than edits. The
change is printed as a diff and nothing is written until you say so:

```bash
ingot new --project . --provider auto "cap the summary at five bullet points"
ingot new --project . --provider auto "cap the summary at five bullet points" --apply
```

Four rules hold on both paths, and they are what makes the result reviewable:

* **The compiler verifies, not the model.** A proposal that does not compile
  becomes a repair prompt carrying the actual diagnostics. The loop is bounded
  by `--max-repairs`, and reaching the ceiling stops with the last source and its
  diagnostics on screen rather than trying again.
* **A model cannot approve its own permissions.** New `policy` grants are listed
  separately from ordinary source repair and stop the command. `--accept-policy`
  is a second, explicit run, and the grants it accepted are printed with the
  result. Restrictions — `deny`, `require approval`, a removed host — are not
  proposals: the language is default-deny, so narrowing asks for nothing.
* **Tools are routed, not invented.** A `tool` declaration is checked against
  what discovery actually reports from the project's MCP servers — so proposing
  into a project starts the servers the manifest already names, exactly as
  `ingot tools` does. A tool nothing can serve is a diagnostic, not a file that
  compiles and fails at run time.
* **No credential goes anywhere.** A credential-shaped value in the proposed
  source ends the loop without writing a file and without sending the source
  back to the model; the report names the shape and the line, never the value.
  The same scan runs on the workflow description before it reaches a prompt.

`--provider replay --cassette FILE` replays a recorded authoring session, and
`--record FILE` writes one — including for a session that ended in a refusal,
which is the one most worth reading again.

An authored project gets no cassette. A recorded answer that no model produced
would be a test proving nothing, so the generated README carries the one
`ingot run --record` command that creates a real one.

### Packaging the checked artifact

`ingot package` writes what was compiled and tested as a standard
[OCI image layout][oci-layout], so an existing client moves it and Ingot invents
no transport:

```bash
ingot package --report python
oras cp --from-oci-layout target/ingot/package:latest ghcr.io/you/research-agent:0.1.0
```

[oci-layout]: https://github.com/opencontainers/image-spec/blob/main/image-layout.md

What comes out is one artifact manifest whose layers are the Agent IR documents
themselves — **the bytes `ingot build` wrote**, not a re-encoding — plus
`ingot.lock` and, when asked, a portability report per target. The printed digest
is `sha256` of the manifest, and it is reproducible: the same source, manifest
and compiler give the same digest on Linux, macOS and Windows, because there is
no timestamp, no build-machine path and no compression anywhere in it.

The lockfile records **identity, not content** — source and agent digests, the
compiler version, and the declared tool servers and model services by name. It is
written into the project as well as into the package, so it can be committed and
reviewed:

```json
{
  "agents": [{ "agent": "packaged.Brief", "digest": "sha256:…" }],
  "ingot": "0.3.0",
  "sources": [{ "digest": "sha256:…", "path": "main.ing" }],
  "toolServers": [{ "command": "ingot-mcp-fs", "name": "workspace", "passEnv": ["GITHUB_TOKEN"] }]
}
```

`passEnv` and `apiKeyEnv` hold variable **names**. There is no field anywhere in
a lockfile that can hold a value, which is the same rule `[[mcp.server]]` already
follows and for the same reason: a lockfile is committed.

Source text does not travel; its digest does. `ingot package --verify` recompiles
the project and names every source, agent and metadata field that moved since the
package was written, repairing nothing:

```bash
ingot package --verify
ingot package --verify --json | jq -e '.matches'
```

`ingot build` and `ingot package` also scan source, the compiled IR and every
cassette for credential-shaped **values**, and refuse rather than warn. The
message names the file, the line and the shape, and never the value. It is a
check on the author rather than a security boundary: what makes
[SECURITY.md](SECURITY.md)'s commitment hold is that there is no path from the
environment into an artifact at all.

A contained-run image may be pinned by digest — `ingot/run@sha256:…` in
`[run] image` or `--image` — and a run refuses when the image present is not the
one named. Acquisition stays manual: a pull becomes automatic only once there is
a signature and a trust root to check it against
([GAP-029](docs/gaps.md#gap-029)).

The normative rules are [Ingot Package 0.1](specs/image/v0.1.md); the reasoning
is [RFC-0012](rfcs/0012-the-ingot-package.md).

### Charging what a run costs

`budget { cost <= 5 usd }` is enforced against prices the project supplies, per
model, in the manifest:

```toml
[[model.price]]
model = "claude-opus-5"     # exactly as the provider reports it
input = "3"                 # per million input tokens
output = "15"
cache-read = "0.3"          # optional; absent means the input rate
currency = "usd"
```

Prices live here rather than in the artifact because they are provider- and
time-dependent: an artifact carrying a price list would be stale the moment it
was published.

With prices configured, exceeding the budget ends the run the way `steps` and
`tokens` do, and `ingot run` and `ingot test` both print what a run cost:

```text
ok   brief  (1 step(s), 690 token(s), 0.00459 USD)
```

Without them the budget is **not enforced**, and everything says so rather than
letting a limit that looks enforced go unchecked — `ingot check` warns
(`ING5007`) and a run names each model it could not price. A budget is only ever
charged against a total that missed nothing.

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

Against a provider that streams, the answer itself is printed as it is written,
indented under the node that asked for it. That text is *not* an event: it is
how the answer happened to arrive over one connection, so it is not recorded in
a cassette and a replay shows none of it. In `--events json` an event is a line
with an `event` key, and the live text arrives as lines without one. If the
answer is then discarded — cut off, or the wrong type — the run says so rather
than leaving a half-finished answer on screen looking like a result. See
[RFC-0013](rfcs/0013-streaming.md) and [Runtime 0.3](specs/runtime/v0.3.md).

```text
trace[0002] node.started demo.Brief:n0  llm.call
             prompt "Write about <redacted input.topic:string>."
             source main.ing:7:19..7:60
trace[0003] model.call   demo.Brief:n0  cassette/model -> markdown  (120 in, 60 out)
             observed steps 1/4; tokens 180/20000
```

Agent IR 0.2 carries optional portable source spans, so a trace renderer with
the local source can map `agent:node` back to the originating `.ing` byte range.
Those spans use project-relative slash-normalized source ids; absolute build
machine paths are not embedded as a shortcut.

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
never falls back to a host run. An image may be pinned by digest —
`ingot/run@sha256:…` — and a run refuses when the image present is not the one
named; automatic acquisition waits for a signature and a trust root
([GAP-029](docs/gaps.md#gap-029)).

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

A run that stops responding is ended rather than waited on. The guest narrates as
it works, so every line it sends resets the clock and the deadline only has to
cover the gap between two steps — which is one tool call inside the box, and that
is already bounded by `[mcp] timeout-seconds`. The ceiling is derived from it,
and stated when you want to state it:

```toml
[run]
timeout-seconds = 600   # or --timeout 600; --timeout 0 waits indefinitely
```

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
* [Agent IR 0.2](specs/ir/v0.2.md) — the backend contract with portable node source spans
* [Agent IR 0.1](specs/ir/v0.1.md) — the original backend contract
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
| M6 | OCI artifact, lockfile, reproducible digest | done |
| M7 | language server and editor support | done |
| M8 | conformance suite and backend author guide | planned |
| M9 | Ingot Containers — the policy block as an enforced boundary | done |
| M10 | `ingot new` — authoring with a model, verified by the compiler | done |
| M11 | integrated `.ing` product loop: templates, `dev`, trace, readiness and safe-run UX | done |

A number is an identity, not a position in a queue; things get referenced by it,
so they keep it. The remaining milestone is **M8**.
[RFC-0007](rfcs/0007-the-ingot-product-loop.md) explains why the
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
exist. [Issue #11](https://github.com/mathissdupont/ingot/issues/11) is now
implemented by Agent IR 0.2: human traces can resolve runtime nodes to portable
`.ing` source ranges when local source is available. M10 followed M11 rather
than preceding it, which was the point: model assistance accelerates a product
loop that already worked, and produces the same ordinary source that loop was
built around. What it deliberately does not do is grow: policy acceptance, tool
routing and credential handling stayed where they were, and the authoring model
is refused at each of them.

## What is missing

Every known limitation has an identifier and an entry in the
**[gap register](docs/gaps.md)** — what is missing, how it shows up, why it is
not done, and what closing it would take. Read it before relying on anything
here.

The five worth knowing before you write an agent:

| | |
|---|---|
| [GAP-001](docs/gaps.md#gap-001) | `network allow ["arxiv.org"]` does not constrain which hosts a tool reaches. The decision is enforced; the list is not. |
| [GAP-030](docs/gaps.md#gap-030) | `verify` cannot be executed by anything. The run reports it as `notPerformed` rather than claiming a pass, so nothing misleads — but nothing checks either. |
| [GAP-010](docs/gaps.md#gap-010) | `parallel map` runs its iterations one after another. The result is identical; only the wall clock differs. |
| [GAP-017](docs/gaps.md#gap-017) | The differential tests are not yet a packaged conformance suite or backend author guide. |
| [GAP-025](docs/gaps.md#gap-025) | Authoring, checking, tracing, testing and safe execution are separate manual workflows. |

The first is **unenforced**: it looks like a guarantee and is not. Everything
else in the register either says what it did not do, fails loudly, or cannot be
expressed at all.

## Contributing

Read [CONTRIBUTING.md](CONTRIBUTING.md). Changes to the language, the IR or the
artifact format go through the [RFC process](rfcs/); everything else can start
as an issue.

## Licence

Apache-2.0. See [LICENSE](LICENSE).

Ingot is an open-source toolchain, not a brand. The project **claims no rights in
the name** and is not seeking a trademark; it is used descriptively. `ingot` on
crates.io is an unrelated packet-parsing library — it publishes no binary, and
this CLI is distributed as `ingot-cli` and as prebuilt archives, so nothing
collides. The reasoning, and what would reopen the question, is
[GAP-019](docs/gaps.md#gap-019).
