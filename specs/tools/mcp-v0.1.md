# MCP Binding, version 0.1

Status: **Superseded in part by [0.2](mcp-v0.2.md)**, which adds the Streamable
HTTP transport and the policy check that governs it. Everything here that 0.2
does not replace is still normative for `ingot-mcp`; optional for a backend.

How an Ingot `tool` declaration is served by a [Model Context Protocol] server.
This is one implementation of the tool host described in
[Runtime 0.1 §5.2](../runtime/v0.1.md), not a requirement of it: a backend may
host tools any way it likes and still conform.

Design reasoning is in [RFC-0003](../../rfcs/0003-mcp-tool-host.md) and
[ADR-0005](../../docs/adr/0005-mcp-over-stdio-only.md).

[Model Context Protocol]: https://modelcontextprotocol.io

## 1. Scope

Implemented: the **tools** half of MCP — `initialize`, `tools/list`,
`tools/call`, `ping` — over the **stdio** transport.

Not implemented: prompts, sampling, resources, completions, logging
subscriptions, and every non-stdio transport. A server that requires one of
these is refused explicitly; nothing is silently ignored.

[0.2 §1](mcp-v0.2.md) adds one transport to that list and leaves the rest.

## 2. Transport

A child process. One JSON-RPC 2.0 message per line on stdin and stdout, UTF-8,
no embedded newlines. Standard error is not part of the protocol and is captured
for diagnostics.

A client:

* must give every request a deadline, and report a timeout naming the method;
* must tolerate notifications arriving while a request is outstanding;
* must ignore a reply whose id it is not waiting for, rather than treating it as
  the answer;
* must refuse a request from the server, since an Ingot client advertises no
  capabilities;
* must close the server's stdin to stop it, and kill it only if it does not
  exit.

## 3. Handshake

The client sends `initialize` with the newest revision it implements. The server
answers with a revision; if that revision is not one the client implements, the
connection fails naming both sides. The client then sends
`notifications/initialized` before any other request.

Revisions implemented by `ingot-mcp` 0.2: `2025-06-18` (preferred),
`2025-03-26`, `2024-11-05`.

A server that does not declare the `tools` capability is refused: it has nothing
this binding can use.

## 4. Configuration

```toml
[mcp]
timeout-seconds = 30            # per request; default 30, minimum 1

[[mcp.server]]
name = "workspace"              # unique within the manifest
command = "ingot-mcp-fs"        # resolved through PATH
args = ["--root", "workspace", "--allow-write"]
cwd = "."                       # relative to the project, default the project
pass-env = ["BRAVE_API_KEY"]    # names only

[mcp.server.tools]
"repo.read_file" = "fs.read_file"   # Ingot name -> the server's name
```

Unknown keys are an error. There is no key that takes a literal environment
value: a manifest is committed, and a secret in a committed file is a published
secret.

`ingot tools --propose` may render an explicit tool alias for an unresolved
declaration only when one live server tool has the same final name component and
a matching checked input shape. The proposal is advisory text: the command does
not edit this configuration, start an installer, or treat discovery as consent.

A server starts with `env_clear()`, then a fixed set of platform-essential
variables, then whatever `pass-env` names and the operator's environment
actually has. Naming a variable the operator does not have is not an error.

## 5. Routing

At connect time each server is asked for its tool list and a routing table is
built, once:

1. explicit `[mcp.server.tools]` entries are applied first;
2. remaining published names route to the identical Ingot name;
3. a tool two servers would both serve is an **error** naming both. Which server
   answers must not depend on which one started first;
4. an alias pointing at a tool the server does not publish is an error listing
   what it does publish.

A server whose alias map covers none of the tools the program declares is not
started. A server with no alias map is started, because its tool list is the only
way to know what it has.

## 6. Calling

`tools/call` with `name` set to the server's name for the tool and `arguments`
an object of the call's arguments, keyed by the parameter names in the Ingot
declaration.

The artifact's declaration is authoritative. A server's `inputSchema` is not a
compile-time dependency: `ingot check` must work with nothing running, so
compilation never depends on what happened to be installed. `ingot tools`
performs a separate live preflight that compares the checked declaration with
the discovered schema and reports drift before execution.

## 7. Results

| Declared Ingot type | Source of the value |
|---------------------|---------------------|
| `text`, `markdown`, `string`, `bytes` | the `text` content blocks, joined, verbatim |
| a record, `json`, `file` | `structuredContent`, else the joined text parsed as JSON |
| a scalar, or any `T[]` | `structuredContent.value`, else the joined text parsed as JSON |

MCP requires `structuredContent` to be a JSON object, so a result whose Ingot
type is not object-shaped travels under `value` — the same convention the model
side uses for `ask<string[]>`.

Further rules:

* `isError: true` is a tool that **ran and failed**: the run stops with the
  server's message. A JSON-RPC error is a call that was **malformed**: the run
  stops saying so. The two are never conflated.
* Content Ingot has no type for — images, audio, embedded resources — is
  ignored when a text block or `structuredContent` is present, and is an error
  naming the kinds when it is all the server returned.
* An empty result is the empty string for a prose type, and an error for any
  other.
* The value is then validated against the declared type by the runtime. This
  section chooses a source; it never widens a type.

## 8. Errors

Every failure names the server. A transport failure also carries the last lines
the server wrote to standard error, because when a server dies that is the only
useful thing in the message.

| Situation | Outcome |
|-----------|---------|
| the command cannot be started | connection fails, naming the command |
| the server exits during the handshake | connection fails, with its stderr |
| no revision in common | connection fails, listing both sides' revisions |
| the server declares no `tools` capability | connection fails |
| a request outlives `timeout-seconds` | the run stops, naming the method |
| `tools/call` returns a JSON-RPC error | the run stops, naming the tool and code |
| `tools/call` returns `isError` | the run stops, quoting the server |
| the result does not match the declared type | the run stops, naming the type |

## 9. Security

Three independent gates stand between an agent and a file, and none of them is
redundant:

1. the **compiler** refuses to build an artifact whose policy does not grant the
   effects of the tools it holds;
2. the **runtime** re-checks that grant against the artifact's own policy before
   each call, because the person running an artifact is often not the person who
   built it;
3. the **server** enforces whatever bound it was started with, whatever the
   artifact says.

The first two are claims the artifact makes about itself. The third is a limit
the operator imposes on it.

A server is a program the operator chose to run, with the arguments and
environment the operator gave it. This binding makes no attempt to sandbox an
arbitrary server beyond that — it cannot — which is why the environment is
minimal by default and why `pass-env` is explicit.

## 10. The reference server

`ingot-mcp-fs` exposes one directory:

| Tool | Signature | Notes |
|------|-----------|-------|
| `fs.read_file` | `(path: string) -> text` | UTF-8 only; capped by `--max-bytes` |
| `fs.list_dir` | `(path: string) -> string[]` | sorted, so a run is reproducible |
| `fs.write_file` | `(path: string, content: text) -> file` | only with `--allow-write` |

`--root` is required; the server refuses to guess what to expose. A path is
refused if it is absolute, if it contains `..`, or if it resolves — through
symlinks — outside the canonicalised root.

It exists so a fresh checkout can run a tool-using agent without trusting
anything from anywhere else, and so the integration tests talk to a real
subprocess rather than a mock.
