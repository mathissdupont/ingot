# ingot

**An agent's permissions, its budget and its behaviour — checked before it runs,
and enforced while it does.**

This crate is the `ingot` command-line tool. It compiles a small declarative
language to **Agent IR**: a portable artifact that states what an agent may
reach, what it will cost at most, and what it produces.

```bash
cargo install ingot-cli
```

The binary is called `ingot`. The reference tool server (`ingot-mcp-fs`) and the
language server (`ingot-lsp`) are separate crates in the same workspace.

## What it refuses

```text
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

That is not a lint. `network allow ["arxiv.org"]` is checked against every call
at compile time — and then enforced at run time, by a filtering proxy on a
container network with no other route out.

## Thirty seconds, and no API key

```bash
ingot init hello && cd hello
ingot check          # types, effects, policy, budgets
ingot test           # runs against the recorded cassette, offline
ingot run            # replays it, byte for byte
```

A run with no key replays a cassette. The same artifact, the same events, the
same outputs — which is what makes `ingot test` a test rather than a demo.

## Conforming a backend

The conformance suite ships inside this binary. A backend under test is a
command; the suite hands it one request file per case and compares what comes
back.

```bash
ingot conform --backend "python my-adapter.py"
ingot conform --export ./suite     # read the cases a case failed on
```

No checkout required, and the suite is the one this binary's specifications
describe.

## Commands

| Command | Purpose |
|---|---|
| `check` | Types, effects, policy and budgets, before anything runs |
| `build` | Source to Agent IR |
| `run` | Execute an artifact, live or from a cassette |
| `test` | Replay recorded runs and compare, offline |
| `conform` | Hold a backend to the specification |
| `doctor` | Whether this machine can run this agent |
| `sandbox` | The boundary the artifact's own policy implies |
| `studio` | One local page showing what the commands each show a piece of |
| `fmt` / `lsp` | Formatting, and the editor's diagnostics |

Run `ingot --help` for the full list.

## Documentation

- [Repository, guides and specifications](https://github.com/mathissdupont/ingot)
- [Writing a backend](https://github.com/mathissdupont/ingot/blob/main/docs/guide/writing-a-backend.md)
- [The gap register](https://github.com/mathissdupont/ingot/blob/main/docs/gaps.md) —
  what this does not do yet, and why

Pre-1.0. The language, the Agent IR and the artifact format may change between
releases.

Licensed under Apache-2.0.
