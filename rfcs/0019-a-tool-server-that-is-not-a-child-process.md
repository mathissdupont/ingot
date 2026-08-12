# RFC-0019: A tool server that is not a child process

- Status: **Accepted**
- Author(s): Heptapus Group
- Created: 2026-08-12
- Affects: CLI, MCP binding

## Problem

[GAP-007](../docs/gaps.md#gap-007). Every MCP server Ingot can use is a program
on the same machine:

```toml
[[mcp.server]]
name = "search"
command = "some-search-server"
```

An organisation that already runs a hosted MCP server cannot point Ingot at it.
The workaround is a local proxy process that forwards stdio to HTTP, which
someone has to write, deploy and keep alive next to every runner.

This was a decision, not an omission.
[ADR-0005](../docs/adr/0005-mcp-over-stdio-only.md) took it and wrote down the
condition for revisiting it:

> When the language grows a way to say which endpoints an agent may reach — a
> policy subject naming tool servers, or `network allow` scoped to a tool — the
> HTTP transport becomes implementable without weakening anything. That is an
> RFC, not a patch.

[GAP-013](../docs/gaps.md#gap-013) closed on 2026-08-10 and gave the language a
reach. This is the RFC.

## The question that has to be answered first

ADR-0005 rejected the easy version for a specific reason, and it is worth
quoting because this RFC has to not do it:

> If a tool call could open an HTTPS connection to a configured MCP server,
> [`network deny`] would quietly become "no network, except through tools,
> except we do not model that".

So the question is not "how do we speak HTTP". The client already exists; the
transport is one trait away, exactly as the ADR said. The question is **where
the endpoint is declared and what checks it** — the one the gap register also
names.

### There are two hops, and only one of them is modelled today

```
  Ingot  ──①──▶  MCP server  ──②──▶  the world
```

Hop ② is what a tool's reach already describes:
`tool web.search(...) !network("arxiv.org")`. The compiler checks it against the
agent's policy, and the server enforces whatever bound it was started with.

Hop ① does not exist for a child process. Over HTTP it does, and nothing in the
artifact mentions it.

### The tempting answer, and why it is wrong

The tempting answer is that hop ① belongs to the operator. The endpoint goes in
`[[mcp.server]]`, which is deployment configuration; the artifact keeps
describing hop ② and stays deployment-independent, which is the property
`config.rs` opens by stating:

> The artifact says *which tools it needs and what they are allowed to do*; this
> says *where those tools come from on this machine*.

It is wrong, and ADR-0005 already explained why in advance. Bytes leaving the
machine on the agent's behalf are network access whoever picked the destination.
An artifact carrying `network deny` that nonetheless ships every tool argument
to `mcp.vendor.example` over TLS is an artifact whose policy is a lie, and the
fact that an operator chose the vendor does not make the agent's declaration
true.

## Proposal

**The endpoint is declared by the operator, in `[[mcp.server]]`, and it is
checked against the agent's own `network` policy grant before the server is
connected.**

```toml
[[mcp.server]]
name = "search"
url = "https://mcp.example.com/mcp"
# Names only, as everywhere else. Read from the operator's environment.
auth-env = "SEARCH_MCP_TOKEN"
```

```ingot
policy {
  network allow ["mcp.example.com", "arxiv.org"]
}
```

At connect time, for each agent that holds a tool served by a remote server, the
host takes the server's URL, extracts its host, and looks it up in that agent's
`network` grant. Not permitted — the run does not start:

```
error: agent `research.Report` may not reach the server that serves `web.search`
  |
  = the server `search` is at https://mcp.example.com/mcp
  = this agent's policy grants network to: arxiv.org
  |
  = help: add "mcp.example.com" to `network allow`, or configure `search`
          with a `command` so it runs locally
```

This satisfies every constraint at once:

* **`network deny` stays absolute.** An agent that denies network cannot be
  served by a remote server at all. There is no "except through tools".
* **The artifact still names no server.** It names *hosts it permits*, which it
  already did. Any deployment whose servers live at permitted hosts works
  without a recompile.
* **No new effect and no new policy subject.** `PolicySubject` maps one-to-one
  onto `Effect`, which is what makes the checker a single lookup; a
  `tool_endpoint` subject would have no effect to pair with, because no tool
  would ever declare `!tool_endpoint(...)`. Reusing `network` keeps the
  invariant and is also simply true.
* **It is checked before anything connects**, like every other configuration
  error in this binding.

### What this costs, stated plainly

The same artifact needs a **wider policy** to run against a remote server than
against a local one. An agent whose only tool reads files needs
`network allow ["mcp.example.com"]` in its source to be served remotely, even
though nothing it does touches the network in the ordinary sense.

That is not a wart to be sanded off later. It is the design telling the truth:
serving that tool remotely *does* put the agent's arguments on the network, and
an author who does not want that written in the source does not want it
happening. The grant is inert for tool dispatch — no tool of that agent declares
`network`, so no call is widened by it — and exists solely to authorise hop ①.

The cost falls where it should: on the author who wants remote serving, at
authoring time, in one line.

### Which agent's policy

Servers are already started **once per agent that holds one of their tools**, so
the check has an agent to check against without any new plumbing. Two agents in
one program legitimately differ, and one of them being allowed to reach a
hosted server does not admit the other.

`ingot tools`, which connects to everything with no artifact in hand, has no
policy to check. It connects anyway — showing what is out there is its whole job
— and labels every remote server as unchecked, the same way `DirectLauncher`
says the policy is checked and not enforced.

## Transport

Streamable HTTP, the transport in MCP revisions `2025-03-26` and `2025-06-18`.
One endpoint, `POST` per outgoing JSON-RPC message.

* `accept: application/json, text/event-stream`.
* A reply with `content-type: application/json` is one message.
* A reply with `content-type: text/event-stream` is a sequence of messages, one
  per event's `data`.
* `202 Accepted` with no body is how a notification is acknowledged, and yields
  no message. `notifications/initialized` is the one Ingot sends.
* `Mcp-Session-Id` from the `initialize` response, when present, is echoed on
  every later request.

The deprecated HTTP+SSE transport (two endpoints, a long-lived `GET`) is not
implemented. A server that only offers it is refused naming what it offered.

Nothing above the transport changes. `McpClient` still writes a line and reads a
line; the JSON-RPC layer, the handshake, the routing and the result conversion
are the code that already exists. This is the same discipline the streaming work
settled on in [RFC-0013](0013-streaming.md) — one parser, two transports — and
it is why `Transport` was a trait from the first commit.

## Configuration

`command` and `url` are mutually exclusive, and exactly one is required.

Every other field means something for a child process and nothing for a URL:
`args`, `cwd`, `pass-env` and `image` are **refused** in combination with `url`
rather than ignored. Silently accepting `pass-env` beside a `url` would let an
operator believe a credential had been supplied to a server that never saw it.

`auth-env` names an environment variable whose value becomes an
`authorization: Bearer …` header. A name, never a value, for the reason the rest
of this binding gives: a manifest is committed, and a secret in one is a secret
published. The value never appears in an error message, a run record, or the
`ingot tools` output.

## Security and policy impact

The policy check above is the substantive one. Three further consequences, each
stated because each is easy to assume the other way.

**A remote server is not confined.** `--sandbox` starts a server inside a
boundary derived from the agent's policy, and there is no process to put in one.
`--sandbox` with a remote server is **refused**, naming the server. Connecting
anyway and reporting a boundary that covers nothing would be worse than not
offering the flag.

**A contained run cannot use one either.** Under `--contained` the interpreter
is inside a box whose network is denied, and the model call crosses out through
the supervisor channel. There is no channel for a tool call, so a remote server
is unreachable from inside. Refused, naming the server, rather than failing at
the first call with a connection error. This is the same family as
[GAP-023](../docs/gaps.md#gap-023) and it gets its own register entry rather
than being folded in, because the reason differs: GAP-023 is about a boundary
that could exist and does not, this is about a hop that has no channel.

**Transport security is the operator's.** `https` is expected and `http` is
permitted, because a server on `127.0.0.1` or inside a cluster is an ordinary
deployment. A plain-`http` URL to anything that is not a loopback address draws
a warning on every run naming the server. No certificate pinning; the platform
trust store is what verifies a certificate, which is the same position the model
providers take.

## Static bounds

Unaffected. A tool call charges one step whatever transport carried it, and the
per-request deadline is the existing `timeout-seconds`, applied to the HTTP
request rather than to a channel read.

One difference worth stating: an HTTP request can be retried where a child-pipe
write cannot. It is **not** retried here. `tools/call` is not idempotent — a
server that sent mail and then failed to answer must not be asked twice — so a
failure is a failure. This matches the streaming rule in `http.rs`, which stops
retrying the moment anything has been delivered, and for the same reason.

## Compatibility

No language change, no IR change, no runtime-specification change. An artifact
compiled before this RFC runs unchanged, and a manifest that configures only
`command` servers is byte-identical.

The MCP binding moves to **0.2** for the transport and the configuration.
ADR-0005 is superseded in part and gets an amendment recording what changed and
what did not: the crate boundary, and the reason for it, still stand.

`ingot-mcp` grows an `http` feature, off by default, mirroring `ingot-runtime`'s
`http` feature. A build that hosts only local servers carries no TLS stack.

## Alternatives

**Keep refusing.** The workaround — a local proxy — genuinely works, and the
ADR was right that shipping the coarse version would have been worse than
shipping nothing. What changed is that the coarse version is no longer the only
option: the policy check in §Proposal is the precise one the ADR was waiting
for. Continuing to refuse would now be refusing for a reason that has been
answered.

**A `tool_endpoint` policy subject.** The other option ADR-0005 named. More
precise on its face — a reader could tell which hosts serve tools from which
hosts tools reach — and rejected because a subject with no matching effect
breaks the one-to-one invariant the checker is built on, and because the
precision it buys is available from the run log, which says which servers are
remote, without touching the policy vocabulary.

**Put the endpoint in the tool's reach.** `tool web.search(...) !network("mcp.example.com")`
makes hop ① look exactly like hop ②. Rejected: it puts a deployment address in
the artifact, so the same artifact cannot be pointed at a second deployment; and
the compiler could not check it anyway, since it does not know which server
serves a tool — that is decided at connect time from a manifest the compiler
never reads.

**Support the deprecated HTTP+SSE transport too.** Two endpoints and a
long-lived `GET` that has to be reconnected. Rejected as a second transport to
maintain for servers that the protocol itself is moving off.

## Conformance tests

- [ ] `a-remote-server-the-policy-does-not-permit-is-refused` — before connecting.
- [ ] `network-deny-refuses-every-remote-server` — the ADR's guarantee, directly.
- [ ] `a-json-reply-and-an-event-stream-reply-are-the-same-message` — one parser,
      two transports.
- [ ] `a-session-id-is-echoed-on-every-later-request`.
- [ ] `url-with-args-or-pass-env-is-refused` — not ignored.
- [ ] `a-credential-never-appears-in-output` — error messages, run records and
      `ingot tools`.
- [ ] `sandbox-and-contained-refuse-a-remote-server`.
