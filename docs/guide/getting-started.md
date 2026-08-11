# Getting started

Fifteen minutes, from nothing to an agent you have run, contained and packaged.
Nothing here needs an API key until the section that says it does.

## Install

```bash
cargo install --git https://github.com/mathissdupont/ingot ingot-cli
```

Or download an archive for your platform from
[Releases](https://github.com/mathissdupont/ingot/releases) — each one carries
`ingot`, the reference tool server `ingot-mcp-fs`, and the language server
`ingot-lsp`.

```bash
ingot --version
```

## A project

```bash
ingot init hello
cd hello
```

Five files, and none of them hide anything:

```text
main.ing                      the agent — the source of truth
ingot.toml                    the project: its name, entry, and where output goes
tests/cassettes/example.json  a recorded model exchange, so tests need no key
README.md                     the commands below, in the project
.gitignore                    /target
```

`main.ing` is the whole program:

```ingot
language 0.1
package hello

/// Summarises a topic into a short markdown brief.
agent Brief(topic: string) -> brief<markdown> {
  model requires {
    structured_output
  }

  budget {
    steps <= 4
    tokens <= 20000
  }

  policy {
    network deny
  }

  flow {
    emit brief = ask<markdown>(
      "Write a short, factual brief about ${topic}. Use headings and bullet points."
    )
  }
}
```

## Check it

```bash
ingot check
```

This is the part that is different from a framework. Before anything runs, the
compiler has resolved the types, checked that every prompt placeholder exists,
worked out which effects the flow needs, compared them against the `policy`
block, and proved the flow fits inside `steps <= 4`.

Try breaking it. Change `${topic}` to `${topci}` and check again:

```text
error[ING2003]: `topci` is not in scope
```

Change `network deny` to nothing at all and add a tool call, and you get a
different refusal — Ingot is **default-deny**, so an effect no rule mentions is
denied rather than allowed.

## Run it, offline

```bash
ingot run --provider replay --input topic="compiler design"
```

That prints a real artifact. It contacts nothing: `--provider replay` uses the
cassette in `tests/cassettes/`, which is a recording of a model exchange. The
project shipped with one, which is why this works on a fresh clone.

A cassette is matched **strictly**. Change the prompt in `main.ing` and replay
again, and the run fails rather than handing back the old answer:

```text
error: interaction 0 was recorded for a different request at node `n0`.
       The prompt or its context changed since recording — re-record the
       cassette and review the diff.
```

`ingot test` replays every cassette in the directory and reports pass or fail.
That is the suite: no key, no network, same answers every time.

## Run it against a real model

```bash
export ANTHROPIC_API_KEY=…        # or OPENAI_API_KEY, or GEMINI_API_KEY
ingot run --input topic="compiler design"
```

The key is read from the environment at the moment of the request. Nothing
writes it to a manifest, a lockfile, a package or a log — and a build-time scan
refuses a credential that ends up in source before it can leave the machine.

Not sure whether the machine is ready?

```bash
ingot doctor
```

It reports the source, the providers, the tool servers and the container
runtime without starting any of them. It names environment variables and never
prints a value.

To keep a run for later, record it:

```bash
ingot run --record tests/cassettes/live.json --input topic="compiler design"
```

Now `ingot test` replays that too, and CI needs no key.

## See all of it at once

```bash
ingot studio
```

This serves one page on the loopback interface: your projects, and for each one
its diagnostics, its readiness, the boundary each tool server would run inside,
the agents it declares, and every run it has had — with a button to start
another. It computes nothing the command line cannot; it shows the same reports
in one place. The URL it prints carries a token that belongs to that process.

## Give it a tool

Tools come from [MCP](https://modelcontextprotocol.io) servers. The source
declares what the agent needs, with the effect that calling it has:

```ingot
language 0.1
package hello

/// Read a file from the workspace, over an MCP server.
tool fs.read_file(path: string) -> text !filesystem_read

agent Digest(path: string) -> digest<markdown> {
  model requires {
    tool_calling
    structured_output
  }

  tools {
    mcp fs.read_file
  }

  budget {
    steps <= 8
    tokens <= 40000
  }

  policy {
    filesystem_read allow ["docs"]
    network deny
  }

  flow {
    contents = call fs.read_file(path)
    emit digest = ask<markdown>("Summarise this file.", context: contents)
  }
}
```

The `!filesystem_read` is an **effect**. The compiler reads it at the call site
and checks it against the `policy` block; remove the `filesystem_read allow`
line and the program stops compiling. `["docs"]` is a bound, not a comment —
under `--sandbox` it becomes a read-only mount of that directory and nothing
else.

Where the tool actually comes from is deployment, so it lives in `ingot.toml`
rather than in the source:

```toml
[[mcp.server]]
name = "files"
command = "ingot-mcp-fs"
args = ["--root", "."]

[mcp.server.tools]
"fs.read_file" = "fs.read_file"
```

Then check what is actually wired up:

```bash
ingot tools
```

That starts each server, asks it what it publishes, and compares the schema
against what the source declared — argument names, types, and all.

## Put it in a box

The `policy` block is not a comment. With a container runtime installed:

```bash
ingot sandbox          # what boundary each tool server would get
ingot run --sandbox    # start them inside it
```

`filesystem_read allow ["docs"]` becomes a read-only mount of `docs` and
nothing else. `network allow ["arxiv.org"]` becomes a filtering proxy on a
network with no other route out — so a server that ignores the proxy reaches
nothing rather than reaching everything.

If the boundary cannot honour a rule the program states, the run is **refused**
rather than started with the rule quietly downgraded.

```bash
ingot run --contained  # the agent process itself goes in the box too
```

## Ship it

```bash
ingot build                   # -> target/ingot/Brief.ir.json
ingot build --target python   # -> target/ingot/Brief.py, self-contained
ingot package                 # an OCI artifact with a lockfile and a digest
```

Agent IR is the canonical artifact: target-neutral JSON carrying the types, the
effects, the policy and the budgets. The Python backend is a second,
independent consumer of it, and a differential test suite asserts the two
produce the same event stream.

## What to read next

* [The language](the-language.md) — a tour of the constructs, in one example
  that grows.
* [Language 0.1](../../specs/language/v0.1.md) and
  [0.2](../../specs/language/v0.2.md) — the specification, when you need the
  exact rule.
* [Runtime 0.1](../../specs/runtime/v0.1.md) — what a compliant runtime must do,
  including why a replayed run reproduces its event stream byte for byte.
* [The gap register](../gaps.md) — every known limitation, with what closing it
  would take. Read it before relying on anything.
* [`examples/`](../../examples/) — three complete projects, each checked in CI.
