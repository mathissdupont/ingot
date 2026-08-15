# The toolchain, command by command

[Getting started](getting-started.md) walks the shortest path from nothing to a
packaged agent. This page is the longer version: what each command is for, what
it refuses, and why.

Every heading below is one part of the loop. Read the ones you need.

## Check readiness before a run

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
[`docs/doctor-json-v1.md`](../doctor-json-v1.md).

## Develop without command juggling

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

## One surface over all of it

`ingot studio` serves a page on the loopback interface showing what the other
commands print, in one place: your projects, and for each one its diagnostics,
its readiness, the boundary each tool server would get, the agents it declares
and the runs it has had.

```bash
ingot studio
# http://127.0.0.1:7317/?token=…
```

The token in that URL belongs to the process and is stored nowhere. The studio
refuses to bind anywhere but loopback, refuses a request whose `Host` is not its
own address and port — which is what stops a name that merely resolves to
`127.0.0.1` from being treated as local — and refuses a cross-site `Origin`.

It computes nothing. [`ingot-studio`](../../crates/ingot-studio/) has no dependencies
and no compiler; every fact reaches it through one trait the CLI implements by
calling the same functions `ingot doctor`, `ingot sandbox`, `ingot check` and
the editor call. The tests are equalities against `ingot doctor --json` and
`ingot check` rather than assertions about the page.

Run history is the one thing it needed that did not exist. `ingot run` now
writes `<out-dir>/runs/<id>.jsonl` — the JSON event stream verbatim, wrapped in
two lines carrying the wall clock and the outcome, which is where a clock is
allowed to live because an event may not carry one. `--no-history` writes
nothing.

A run can also be started from the page. The form offers the agents the artifact
declares and a field per input it takes, and then spawns the same command you
would type — the studio interprets nothing itself.

**It still cannot pass `--yes`,** and a gate it reaches is shown rather than
assumed. The run stops in front of the effect and the page says so: what the
effect is, the reason the compiler attached, and two buttons. Answering one gate
lets the run continue to the next, which asks again. A run waiting this way is
still `running` and will wait indefinitely — a person is not a service and no
clock decides for them — so the page shows it as waiting and the Stop button is
there for the run nobody is going to answer.

That distinction is the whole of it: `--yes` answers every gate in a run before
any of them has been seen, and this answers the one in front of you. See
[RFC-0020](../../rfcs/0020-a-person-in-the-loop.md).

Connecting a model service is still something you do by hand: the page shows the
`[[model.provider]]` block to write and the variable to export, and there is no
field to type a credential into. See
[RFC-0015](../../rfcs/0015-ingot-studio.md).

## Authoring with a model

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

## Packaging the checked artifact

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
[SECURITY.md](../../SECURITY.md)'s commitment hold is that there is no path from the
environment into an artifact at all.

A contained-run image may be pinned by digest — `ingot/run@sha256:…` in
`[run] image` or `--image` — and a run refuses when the image present is not the
one named. Acquisition stays manual: a pull becomes automatic only once there is
a signature and a trust root to check it against
([GAP-029](../gaps.md#gap-029)).

The normative rules are [Ingot Package 0.1](../../specs/image/v0.1.md); the reasoning
is [RFC-0012](../../rfcs/0012-the-ingot-package.md).

## Charging what a run costs

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

## Choosing a model

Which model an agent uses is part of the agent, not part of the command line:

```ingot
language 0.1

agent Brief(topic: string) -> brief<markdown> {
  model exact "openai/gpt-5.1"      // or "anthropic/claude-opus-5"

  budget { steps <= 4 }
  policy { network deny }

  flow {
    emit brief = ask<markdown>("A brief about ${topic}.")
  }
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
| `google/…` | Gemini. `GEMINI_API_KEY`, or `GOOGLE_API_KEY` |

Those three need no configuring. **Anything else you name yourself:**

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

An agent then reaches the first of those with `model exact "local/llama-3.3-70b"`.

`kind` names a **protocol**, not a company. Ingot implements three, and Chat
Completions alone is spoken by Ollama, vLLM, llama.cpp, LM Studio, Azure OpenAI,
Groq, Together, OpenRouter, Fireworks, DeepSeek and most hosted gateways — so
"how many providers does Ingot support" is the wrong question. A declaration may
also take over a built-in name, pointing `openai/…` at a company gateway without
editing a single agent.

| `kind` | Reaches | `base-url` is |
|---|---|---|
| `openai` | anything speaking Chat Completions | the full endpoint |
| `anthropic` | Anthropic, and gateways fronting it | the full endpoint |
| `google` | Gemini | the API base — this protocol puts the model and the method in the path |

### Asking for a capability instead of a model

An agent can state what it needs rather than which model provides it:

```ingot
language 0.1

agent Brief(topic: string) -> brief<markdown> {
  model requires {
    structured_output
    context >= 128k
  }

  budget { steps <= 2 }
  policy { network deny }

  flow {
    emit brief = ask<markdown>("A brief about ${topic}.")
  }
}
```

That has to be matched against something, and what it is matched against is the
**catalogue** — the same manifest, beside the prices:

```toml
[[model.catalogue]]
model = "openai/gpt-5.1"          # `vendor/model`, as `model exact` spells it
context = 400000                  # tokens; absent means unknown
capabilities = ["tool_calling", "structured_output", "streaming", "vision"]
```

The vendor half is the name the provider goes by here: one of the three built-in
protocols, or whatever you called a `[[model.provider]]`. It is the same name
`model exact "<vendor>/<model>"` uses, because one provider answering to two
names in one manifest is a way of having half your configuration silently not
apply.

The first entry of that vendor which satisfies the requirement answers, so
declaration order is preference order and it is yours. Ingot carries a small
built-in catalogue so `model requires` works out of the box; a declared entry
for the same model **replaces** it rather than merging, because a
half-overridden model is a set of facts from two places that matches neither.

A model's context window and capabilities change on the vendor's schedule, not
on this project's. Keeping them here is what stops a model growing a larger
window from being a code change and a release — and it is why an artifact
asking for `context >= 2m` is a line in your manifest away from working rather
than a version of Ingot away.

An **unknown** window does not satisfy a requirement. Guessing would turn a
refusal you can fix into a provider error at the first long prompt, a long way
from its cause. When nothing matches, the refusal names every candidate and
what each one was short of.

A third protocol exists for one reason: Gemini is the vendor that cannot be
reached by pretending to be something else. Anything already speaking one of the
first two needs no code here, only a `base-url`.

An agent may instead state what it needs — `model requires { structured_output,
context >= 128k }` — and let the provider pick. Anthropic resolves that against
its default model; OpenAI and Google refuse, because a guessed model name
produces a `404` that reads like a bug here.

## Running an agent

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
[RFC-0013](../../rfcs/0013-streaming.md) and [Runtime 0.3](../../specs/runtime/v0.3.md).

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

### Answering a gate from another program

An artifact whose policy says `require approval` stops in front of the gated
effect and asks. At a terminal it prompts. Anywhere else — cron, CI, a parent
process — there is no terminal, and the gate is **denied** rather than assumed:
the artifact asked for a person, and not having one is an answer.

`--approvals stdin` gives that person a way in without a terminal:

```bash
ingot run ./agent --events json --approvals stdin
```

The exchange uses two streams that already exist. The gate leaves on the event
stream, and the answer comes back as one JSON line:

```text
stderr  {"event":"approvalRequested","node":"n7","effects":["filesystem_write"],"reason":"…"}
stdin   {"node":"n7","allowed":true}
```

`--events json` is required, and a run that asks for the channel without it is
refused before it starts — a parent that cannot see a gate cannot answer one, and
both sides would wait for the other forever. `--yes` cannot be combined with it
either: that answers every gate in the run before any of them is reached, which
is a different thing from answering the one in front of you.

Standard output is untouched, because it carries the run's artifacts so the
command still composes with a pipe.

Every way of failing to answer is a refusal. A closed pipe, an unreadable line,
an answer naming a different gate — none of them opens the gate, because the one
thing an approval exists to prevent is an effect happening without a person. See
[RFC-0020](../../rfcs/0020-a-person-in-the-loop.md).

## Tools

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
[`docs/tools-json-v1.md`](../tools-json-v1.md).

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

[`ingot-mcp-fs`](../../crates/ingot-mcp/) is a small sandboxed filesystem server that
ships with the repository, so a fresh checkout can run a tool-using agent
without installing anything else:

```bash
cargo install ingot-mcp          # or `--path crates/ingot-mcp` in a checkout
ingot tools examples/repo-digest
```

Details: [MCP binding 0.1](../../specs/tools/mcp-v0.1.md), and
[0.2](../../specs/tools/mcp-v0.2.md) for a server reached over a network.

## The policy block as a boundary

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

See [RFC-0004](../../rfcs/0004-ingot-containers.md) and
[ADR-0006](../adr/0006-a-policy-enforcing-runner.md). What the boundary
actually grants is asserted against a real runtime in
[`crates/ingot-sandbox/tests/container.rs`](../../crates/ingot-sandbox/tests/container.rs):
a read mount refuses a write, an unnamed path does not exist inside, and
`network deny` leaves no interface at all.

## Putting the agent in the box too

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
([GAP-029](../gaps.md#gap-029)).

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
  This is [Runtime 0.1 §11](../../specs/runtime/v0.1.md) satisfied by topology.

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
([GAP-023](../gaps.md#gap-023)).

See [RFC-0005](../../rfcs/0005-the-contained-run.md) and
[ADR-0007](../adr/0007-containing-the-run-is-not-blocked-on-a-second-backend.md).

