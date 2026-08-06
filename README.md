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
ingot run --input topic=…         # execute it
```

Status: **milestone M3**. The front end is complete — lexer, parser, type and
effect checker, lowering to Agent IR — and a reference interpreter executes that
IR against a real model provider, re-enforcing every capability, budget and
approval the artifact declares. Runs can be recorded to a cassette and replayed
offline, so agent tests work in CI with no API key. Additional runtime backends,
OCI packaging and the language server are planned; see [the roadmap](#roadmap).

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
| Execution | Existing runtimes, through backends. A native runtime is deferred. |

The differentiators are compile-time portability, typed effects and
capabilities, a machine-readable target compatibility report, reproducible
artifacts, and a conformance suite backends can be tested against.

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

Three complete examples live in [`examples/`](examples/).

## Install

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
| `ingot init <name>` | create a project |
| `ingot check` | parse, type-check, validate policy and budgets |
| `ingot fmt [--check]` | canonical formatting |
| `ingot build [--out-dir]` | compile to Agent IR |
| `ingot ir [--agent]` | print the IR to stdout |
| `ingot run [--input k=v]` | execute the agent |
| `ingot test` | replay recorded cassettes |
| `ingot explain <CODE>` | explain a diagnostic in full |

Exit codes: `0` success, `1` the program has blocking diagnostics, `2` the
command itself failed. Diagnostics, progress events and status lines go to
stderr; `ingot ir` and `ingot run` write only their payload to stdout, so both
are safe to pipe.

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
  interpreter     other backends               ingot-runtime; more planned (M5)
     │                │
     ▼                ▼
  execution      portability report            planned: M5
```

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
| `ingot-cli` | the `ingot` binary |

## Specifications

* [Language 0.1](specs/language/v0.1.md) — syntax and static semantics
* [Agent IR 0.1](specs/ir/v0.1.md) — the backend contract
* [Runtime 0.1](specs/runtime/v0.1.md) — what executing an artifact means
* [`agent-ir.schema.json`](specs/ir/agent-ir.schema.json) — machine-readable schema
* [Architecture](docs/architecture/overview.md) — how the phases fit together
* [Decision records](docs/adr/) — why the load-bearing choices were made

## Roadmap

| Milestone | Scope | State |
|-----------|-------|-------|
| M0 | scope, prior art, non-goals | done |
| M1 | grammar, parser, diagnostics, formatter | done |
| M2 | types, effects, policy, budgets, Agent IR | done |
| M3 | reference interpreter, `ingot run`, end-to-end execution | done |
| M4 | cassette record and replay, `ingot test` | done |
| M5 | a second backend and the portability report | next |
| M6 | OCI artifact, lockfile, reproducible digest | planned |
| M7 | language server and editor support | planned |
| M8 | conformance suite and backend author guide | planned |

M3 and M4 landed together: the interpreter needed cassettes to be testable, and
cassettes needed the interpreter to be worth recording. M5 is where the central
claim is actually proven — the same source, two independent runtimes, with every
unsupported feature named in a report rather than discovered in production.

Two things are deliberately missing from the runtime today, and both fail loudly
rather than silently: **tool calls** have no host until MCP lands (an agent that
grants a tool stops with a message saying so), and `parallel` **executes
sequentially** — valid because the compiler guarantees map iterations cannot
observe each other, but not yet fast.

## Contributing

Read [CONTRIBUTING.md](CONTRIBUTING.md). Changes to the language, the IR or the
artifact format go through the [RFC process](rfcs/); everything else can start
as an issue.

## Licence

Apache-2.0. See [LICENSE](LICENSE).

`Ingot` is a working name. Trademark, domain and package-registry clearance has
not been carried out, and must be completed with legal review before any public
release under this name.
