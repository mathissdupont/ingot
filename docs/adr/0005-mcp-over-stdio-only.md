# ADR-0005: MCP over stdio only, in a separate crate

- Status: Accepted; **decision 1 superseded 2026-08-12** by
  [RFC-0019](../../rfcs/0019-a-tool-server-that-is-not-a-child-process.md)
- Date: 2026-08-07

## Context

Ingot needs to call tools. MCP is the protocol with adoption, and it defines
several transports: a child process on stdio, and HTTP variants for reaching a
server over a network.

Supporting both looks like a free win — the client code is nearly identical, and
the transport is one trait away. It is not free, for a reason that has nothing
to do with the code.

## Decision

1. Only the **stdio** transport is implemented in 0.2.
2. Tool support lives in its own crate, `ingot-mcp`, which depends on
   `ingot-runtime` rather than the other way round.

## Rationale

**Reaching a remote server is an effect the language cannot yet express.**
An agent's policy can say `network deny`, and today that is a real guarantee:
the runtime refuses any node whose effects include `network`. If a tool call
could open an HTTPS connection to a configured MCP server, that guarantee would
quietly become "no network, except through tools, except we do not model that".

The honest options were to (a) treat every tool call on an HTTP transport as
requiring `network`, which is true but so coarse that it makes the effect
useless — an agent that only wants to read files would need `network allow`; or
(b) add a policy subject for tool endpoints, which is a language change and
deserves its own RFC. Shipping (a) as if it were a design would be worse than
shipping neither.

A local subprocess needs none of that. Its reach is bounded by how it was
started, and starting it is the operator's explicit act.

**The runtime should not know what MCP is.** `ingot-runtime` defines
`ToolHost`, three methods wide, and knows nothing else about tools. Putting the
protocol in a separate crate means a backend that hosts tools differently — a
sandboxed WASM host, an in-process registry, a vendor's own bridge — replaces
one crate and keeps the rest. It also keeps process spawning and a JSON-RPC
implementation out of the dependency graph of anyone who only wants to execute
IR.

The direction of the dependency matters: `ingot-mcp` depends on
`ingot-runtime`, so `ToolHost` stays the interface and MCP stays an
implementation of it. Reversing that would make MCP the runtime's problem.

## Consequences

**Good.** The `network` effect keeps meaning what it says. Backend authors
inherit a three-method trait rather than a protocol. The security story for a
tool server is short enough to state in a paragraph: it is a child process, it
starts with the environment you named, and it can reach what you started it
with.

**Bad.** Remote MCP servers — including hosted ones an organisation may already
run — cannot be used. The workaround is a local proxy process, which is a real
cost and not a pretence otherwise.

**Bad.** Two crates rather than one, and `cargo install ingot-cli` does not
bring `ingot-mcp-fs` with it.

## Revisiting

When the language grows a way to say which endpoints an agent may reach — a
policy subject naming tool servers, or `network allow` scoped to a tool — the
HTTP transport becomes implementable without weakening anything. That is an RFC,
not a patch.

## Amendment, 2026-08-12

That condition was met and the RFC was written.
[RFC-0019](../../rfcs/0019-a-tool-server-that-is-not-a-child-process.md)
implements Streamable HTTP and **supersedes decision 1**.

What changed: a `[[mcp.server]]` may carry a `url`, and the server's host is
checked against the calling agent's own `network` grant before anything
connects.

What did not change, and is the part worth re-reading:

* **The rationale above still holds.** This RFC did not decide that reaching a
  remote server is unmodelled network access; it decided the opposite, and put
  the check where the ADR said it had to go. `network deny` still means what it
  says, with no "except through tools".
* **Neither of the two options this ADR rejected was taken.** It is not option
  (a) — a blanket `network` requirement on every tool call — because a local
  server still needs no grant at all. It is not option (b) either: no policy
  subject was added, because a subject with no matching effect would break the
  one-to-one invariant the checker is built on. The third answer is that the
  *existing* `network` grant is exactly the right thing to check, and it took
  [GAP-013](../gaps.md#gap-013) closing to make that grant precise enough to be
  worth checking.
* **Decision 2 stands.** Tool support is still its own crate, `ingot-mcp` still
  depends on `ingot-runtime` and not the reverse, and `ToolHost` is still three
  methods wide. The HTTP transport is one more implementation of the `Transport`
  trait, which is why the client above it did not change.
* **The "bad" consequence about remote servers is resolved.** The one about two
  crates is not, and stands.
