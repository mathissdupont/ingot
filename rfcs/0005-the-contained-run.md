# RFC-0005: The contained run

- Status: **Accepted**
- Created: 2026-08-08
- Affects: runtime spec, CLI, manifest
- Implements: [ADR-0006](../docs/adr/0006-a-policy-enforcing-runner.md) stage 2,
  as amended by [ADR-0007](../docs/adr/0007-containing-the-run-is-not-blocked-on-a-second-backend.md)
- Closes: the rest of the filesystem half of [GAP-001](../docs/gaps.md#gap-001)
- Opens: [GAP-023](../docs/gaps.md#gap-023) (sub-agents across a boundary),
  [GAP-024](../docs/gaps.md#gap-024) (a wedged run is not timed out; since closed)

## Problem

[RFC-0004](0004-ingot-containers.md) put each tool server inside a boundary
derived from the artifact's policy. It left the interpreter on the host, and
said so:

> The boundary constrains a **tool server**. It does not constrain the
> interpreter, which is ours and which follows a checked artifact.

"Ours and following a checked artifact" is true and is not the same as
"constrained". Today, for an agent whose policy is

```ingot
policy {
  filesystem_read allow ["src"]
  network deny
}
```

the boundary gives the tool server `/workspace/src` and no network, and the
process that reads the model's answers, renders prompts, writes artifacts and
holds the API key runs as the operator with the operator's whole machine. The
narrow half is contained. The wide half is not.

That is not a hypothetical. `ingot run --sandbox --out-dir target/x` writes
`target/x` from the host process, outside every mount the policy named, and
nothing in the policy authorised that path.

## Goals and non-goals

**Goals**

1. The interpreter runs inside the boundary its own policy describes.
2. `network deny` and a working model call are compatible, because the model call
   leaves through the supervisor rather than through the network stack.
3. The model credential is never inside the boundary, as a matter of topology.
4. An `approval` gate still reaches a human.
5. The protocol is testable without a container runtime.

**Non-goals**

* **Sub-agent calls across boundaries.** Two agents with different policies need
  two boxes. This RFC refuses that program rather than running it in one box with
  the union of its grants. See *Whose policy*, below.
* **An egress proxy.** `network allow ["arxiv.org"]` is still unenforceable, and
  still reported as such.
* **Publishing an image.** The image is built from a Dockerfile in this
  repository; nothing is pushed anywhere.
* **Streaming.** The channel could carry tokens later. It does not now.

## Shape

```
host                                        inside the boundary
────                                        ───────────────────
ingot run --contained
  │  holds ANTHROPIC_API_KEY                  ingot exec
  │  holds the terminal                         │  the interpreter
  │  holds the compiled IR                      │  the MCP tool servers
  │                                             │
  ├── config ────────────────────────────────►  │  IR, inputs, step ceiling
  │◄─ model/complete ───────────────────────────┤  network deny still holds
  ├── the answer ─────────────────────────────►  │
  │◄─ approval ────────────────────────────────┤  asked, never decided, inside
  │◄─ event ───────────────────────────────────┤  printed on the host's stderr
  │◄─ finished { outputs } ─────────────────────┤
  └─ writes --out-dir
```

The boundary is exactly the plan [RFC-0004](0004-ingot-containers.md) already
derives, applied to the entry agent, with nothing added: `--read-only`,
`--cap-drop ALL`, `--security-opt no-new-privileges`, `--tmpfs /tmp`, the policy's
mounts under `/workspace`, and `--network none` unless the policy says otherwise.

Two things do **not** cross the boundary and this is the point of the design:

* **The credential.** `--env` names nothing but what `pass-env` listed. The
  provider is on the host; the guest never sees a key, a base URL or a vendor
  name it did not get from the artifact.
* **The filesystem outside the mounts.** `--out-dir` is written by the host after
  the run, from the outputs the guest returned. The guest cannot write there and
  does not know where it is.

### Why the artifact is sent rather than mounted

An earlier draft mounted the IR read-only at `/artifact`. Sending it over the
channel is better: one fewer mount, and the boundary contains only paths the
policy named. Nothing in the box is readable that the policy did not grant.

## The protocol

Newline-delimited JSON on the guest's standard streams. One object per line, no
embedded newlines, UTF-8.

Initiative flows one way. **The guest calls, the host answers.** The host never
initiates, which removes the interleaving problem `ingot-mcp` had to solve, and
means a guest is a simple loop rather than a state machine.

Guest to host, on stdout — a call, awaiting a reply:

```json
{"seq":1,"call":"config","params":{"protocol":1}}
{"seq":2,"call":"model","params":{"node":"n3","model":{"kind":"exact","reference":"openai/gpt-5.1"},"prompt":"…","context":[],"responseType":"markdown","shape":{"kind":"prose"},"maxTokens":4096}}
{"seq":3,"call":"approval","params":{"node":"n7","effects":["external_write"],"reason":"post the review"}}
```

Guest to host — a notification, expecting no reply:

```json
{"notify":"event","params":{"event":"nodeStarted","node":"n3","kind":"ask"}}
{"notify":"finished","params":{"agent":"review.Digest","steps":4,"usage":{"inputTokens":900,"outputTokens":120},"outputs":{"digest":{"name":"digest","contentType":"markdown","value":"# …"}}}}
{"notify":"failed","params":{"reason":"node `n3` needs the `network` effect, which the artifact's policy denies","operatorError":false}}
```

Host to guest, on stdin — a reply, always carrying the `seq` it answers:

```json
{"seq":1,"ok":{"agent":"review.Digest","inputs":{"topic":"compilers"},"maxSteps":1000,"agents":[{…IR…}],"mcp":{…}}}
{"seq":2,"err":{"kind":"rateLimited","retryAfterSeconds":30}}
```

### Rules

* **`config` is first and happens once.** A guest that calls anything else first
  is a protocol error and the run is refused. The reply carries everything the
  run needs: the entry agent's name, every agent IR document in the program, the
  inputs, the step ceiling, and the MCP configuration.
* **`protocol` is checked, not negotiated.** One version exists. A mismatch is
  refused by the host with both numbers named, because a guest from a different
  image than the host is exactly the situation where guessing is worst.
* **An error is reproduced, not flattened.** `err.kind` maps onto the runtime's
  own `ProviderError` variants, so a rate limit inside the box is the same
  condition as a rate limit outside it and the interpreter behaves identically.
  A single opaque `"error"` string would turn `Truncated { limit }` into prose
  and lose the retry semantics.
* **The guest writes nothing else to stdout.** Diagnostics go to stderr, which
  the host relays with a prefix so that a crash inside the boundary is visible
  rather than being a closed pipe.
* **A closed channel is a failure.** A guest that exits without `finished` or
  `failed` produces an error naming its exit status, never a run that appears to
  have succeeded with no outputs.

### Whose policy

One agent's. The boundary is planned from the **entry** agent, and the run is
**refused** when any other agent in the program would get a different boundary:

```
error: this program's agents do not share one boundary, so containing the run
       would widen a policy
  review.Digest      /workspace/src ro
  review.Coordinator /workspace/src ro, /workspace/out rw

  the coordinator may write and the sub-agent may not. One box cannot hold both
  without giving the sub-agent a grant its own policy denies.

  run without --contained, or use --sandbox, which gives each agent's tool
  servers their own boundary
```

This is [Runtime 0.1 §2](../specs/runtime/v0.1.md) again: refusing is the only
honest option, because the alternative is a box built to the widest policy in the
file and a sub-agent silently holding grants it does not declare. The fix is a
box per agent with sub-agent calls crossing the supervisor — the same channel,
one more method — and it is [GAP-023](../docs/gaps.md#gap-023), not this RFC.

Single-agent programs, which is most of them, are unaffected.

### Where the tool servers run

Inside, as children of the contained interpreter, spawned by the same
`DirectLauncher` a host run uses. They inherit the boundary rather than getting
their own, which is the same guarantee by a simpler route: they are already
inside a box built from the policy they would have been given.

Consequences:

* The image must contain the tool server binaries. `tools/ingot.Dockerfile` ships
  `ingot` and `ingot-mcp-fs`; a manifest naming a third-party server needs an
  image with that server in it, and the run fails on `command not found`
  otherwise. That failure is legible and is left to be legible.
* `image` and `cwd` on an `[[mcp.server]]` are meaningless inside and are ignored
  with a warning, rather than silently.
* `pass-env` still applies, and still crosses by name only.

### Why `--supervised` exists

The protocol is worth testing where there is no container runtime, which is most
CI matrices and most laptops. `ingot run --supervised` runs `ingot exec` as an
ordinary child process over the same channel — same config, same model proxying,
same approval routing, **no boundary at all**.

It is hidden from `--help` and prints a line saying it enforces nothing, because
a flag that looks like containment and is not is worse than no flag. Its purpose
is to make the interesting half — the protocol — assertable on every platform,
and to leave one variable when a contained run misbehaves.

## Refusals

Before anything starts:

| Situation | Why refuse |
|---|---|
| The agents do not share a boundary | Above. Widening a policy is the failure this whole feature exists to prevent. |
| A plan has unenforceable entries | Unchanged from RFC-0004. `--sandbox-allow-unenforced` still proceeds. |
| No image is configured | We cannot guess. The error prints the `docker build` command that makes one. |
| No container runtime, or the wrong kind | Unchanged from RFC-0004, including the Linux-containers check. |
| `--record` with `--contained` | The recorder wraps the host's provider, and it would work — but the cassette would claim to be a recording of a contained run while the tool results, which happened inside, are absent from it. Refused until tool recording exists. |

## Compatibility

Additive. No language change, no IR change, no new node kind, no change to any
existing command's behaviour.

* `ingot exec` is new and hidden. It is not a way to run an agent; it is the guest
  half of a supervised run and refuses to do anything without a channel.
* `ingot run --contained` and `--supervised` are new flags. Without them nothing
  differs.
* `[run] image` is a new optional manifest key.

The protocol is versioned from 1 and is **not** a stability promise: the host and
the guest are the same binary in normal use, and a mismatch is refused rather
than tolerated. It becomes a compatibility surface the day an image is published
separately from the host, and that day needs its own RFC.
