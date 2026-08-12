# MCP Binding, version 0.2

Status: **Draft, implemented**. Normative for `ingot-mcp`; optional for a
backend.

Version 0.2 is [version 0.1](mcp-v0.1.md) plus one transport and the rule that
makes it safe to have:

1. **Streamable HTTP** (§2), so a server may be reached rather than started;
2. a **policy check** on the server's host (§4), so an agent's `network` grant
   still means what it says.

The handshake, the routing, the calling convention, the result conversion and
the error mapping are 0.1 unchanged. So is everything in 0.1 §9 about a local
server; §4 here adds to it rather than replacing it.

Design reasoning is in
[RFC-0019](../../rfcs/0019-a-tool-server-that-is-not-a-child-process.md), which
also records what
[ADR-0005](../../docs/adr/0005-mcp-over-stdio-only.md) decided and why the
condition it set for revisiting was met.

## 1. Scope

Added: the **Streamable HTTP** transport, as defined in MCP revisions
`2025-03-26` and `2025-06-18`.

Still not implemented: prompts, sampling, resources, completions, logging
subscriptions, and the **deprecated HTTP+SSE** transport — two endpoints and a
long-lived `GET`, for servers the protocol itself is moving off. A server
offering only that is refused naming what it offered.

## 2. Streamable HTTP

One endpoint, one `POST` per outgoing JSON-RPC message.

| Reply | Means |
|---|---|
| `content-type: application/json` | one message |
| `content-type: text/event-stream` | a sequence of messages, one per event's `data` |
| `202 Accepted`, no body | a notification was accepted; nothing to read |

Within an event stream, repeated `data:` lines in one event join with newlines
and are still **one** message; a line beginning `:` is a keep-alive and is not a
message; `event`, `id` and `retry` are framing.

A client:

* sends `accept: application/json, text/event-stream`;
* keeps `Mcp-Session-Id` from the `initialize` reply, when the server sends one,
  and echoes it on every later request;
* ends the session with a `DELETE` carrying that id. A server answering `405`
  has nothing to end, which is not a failure;
* delivers each message to the same reader the stdio transport feeds. Nothing
  above the transport may need to know which one it is talking through.

### 2.1 A tool call is not retried

An HTTP request *can* be retried where a write to a child's pipe cannot, and
this transport does not.

`tools/call` is not idempotent. A server that sent mail and then failed to
answer must not be asked twice, and the transport cannot tell that case from a
lost connection. A failure is a failure. This is the rule the streaming layer
already follows for the same reason: it stops retrying the moment anything has
been delivered.

## 3. Configuration

```toml
[[mcp.server]]
name = "hosted"
url = "https://mcp.example.com/mcp"
auth-env = "SEARCH_MCP_TOKEN"   # a name; the value becomes a bearer token
```

`command` and `url` are mutually exclusive, and exactly one is **required**.

`args`, `cwd`, `pass-env` and `image` each configure a program this machine
starts. Each is **refused** beside a `url` rather than ignored: accepting
`pass-env` silently would let an operator believe a credential had reached a
server that never saw it. `auth-env` is refused beside a `command` for the
mirror-image reason — a child process is given variables with `pass-env`.

`auth-env` names an environment variable whose value becomes
`authorization: Bearer …`. A name and never a value, for the reason the rest of
this binding gives: a manifest is committed, and a secret in one is a secret
published. The value is read at connect time and never appears in an error
message, a run record, or `ingot tools` output.

## 4. A remote server is checked against the agent's `network` grant

Serving a tool over HTTP puts the agent's arguments on the network. That is
network access whoever chose the destination, so **before connecting**, the
server's host is looked up in the calling agent's own `network` policy rule.

| The agent's policy | Which remote servers it may use |
|---|---|
| no `network` rule | none |
| `network deny` | none |
| `network allow` | any |
| `network allow ["mcp.example.com"]` | that host and its subdomains |

Subdomain matching is the rule a tool's declared reach already uses, so an
operator does not learn two.

The check is per agent, because servers are already started per agent and two
agents in one program legitimately differ: one being allowed to reach a hosted
server does not admit the other.

A refusal names the agent, the server, the URL, what the policy does grant, and
both ways out — widen the grant, or configure the server with a `command`.

### 4.1 What this costs, stated

The same artifact needs a **wider policy** to be served remotely than locally.
An agent whose only tool reads files needs `network allow ["mcp.example.com"]`
in its source to use a hosted server.

That is the design telling the truth rather than a wart. Serving that tool
remotely does put its arguments on the network, and an author who does not want
that written in the source does not want it happening. The grant is inert for
tool dispatch — no tool of that agent declares `network`, so no call is widened
by it — and exists solely to authorise the hop to the server.

### 4.2 No artifact, no check

`ingot tools` connects to every configured server with no agent in hand.
Showing what is out there is its whole job, so it connects, and it reports the
result as unchecked rather than implying a policy was consulted.

## 5. A remote server is not confined

`--sandbox` bounds a process this machine starts, and there is none.
`--contained` puts the interpreter in a box whose network is denied, and the
supervisor channel carries a model call and an approval gate — not a tool call.

Both **refuse** a remote server, naming it. Connecting anyway and reporting a
boundary that covers nothing would be worse than not offering the flag.

## 6. Transport security is the operator's

`https` is expected. Plain `http` is permitted, because a server on the loopback
interface or inside a cluster is an ordinary deployment; anything else over
plain `http` draws a warning naming the server on **every** run.

Certificates are verified against the platform trust store, which is the
position the model providers already take. There is no pinning.
