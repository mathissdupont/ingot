# The language, in one example that grows

A tour rather than a specification. When you need the exact rule, read
[Language 0.1](../../specs/language/v0.1.md) and
[0.2](../../specs/language/v0.2.md); this is the shape of the thing.

Every `ingot` block below is compiled by the test suite, so none of them can
quietly stop being true.

## An agent is a function with a budget and a policy

```ingot
language 0.1

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
    emit brief = ask<markdown>("Write a brief about ${topic}.")
  }
}
```

Five parts, and four of them are declarations the compiler enforces:

* the **signature** — typed inputs, and named outputs with a content type;
* `model requires` — capabilities, not a vendor. `structured_output`,
  `tool_calling`, `context >= 128k`. Which model satisfies them is a deployment
  question, answered by `ingot.toml` or `--model`;
* `budget` — a ceiling on steps, tokens and cost. `steps` is checked
  statically: a flow that cannot fit is a compile error, not a surprise;
* `policy` — what the agent may do. Default-deny;
* `flow` — the program.

## Types are ordinary, and content types are separate

The scalar types are `string`, `int`, `float`, `bool`, and `text`. Records are
declared at the top of a file, alongside the agent that uses them — every file
declares at least one agent, so these snippets are all whole programs you can
paste into a `.ing` file and check:

```ingot
language 0.1

type search_result {
  title: string
  url: string
  snippet: string
}

agent Pick(topic: string) -> choice<json> {
  model requires { structured_output }
  budget { steps <= 4 }
  policy { network deny }

  flow {
    hits = ask<search_result[]>("Invent three plausible results for ${topic}.")
    emit choice = ask<json>("Pick the best one.", context: hits)
  }
}
```

`ask<search_result[]>` is typed against that record: a response whose fields do
not fit fails the node rather than flowing onward as something else.

`markdown` in `choice<json>` — or `brief<markdown>` above — is a **content
type**, not a data type. It says what the artifact *is*, so a backend can write
it out correctly and a verifier knows what it is looking at. `json`, `text` and
`markdown` are the ones 0.1 has.

Language 0.2 adds optionals and unions:

```ingot
language 0.2

type finding {
  summary: string
  url: string?
  score: int | float
}

agent Score(topic: string) -> result<json> {
  model requires { structured_output }
  budget { steps <= 4 }
  policy { network deny }

  flow {
    scored = ask<finding>("Score ${topic}.")
    emit result = ask<json>("Format that as a report.", context: scored)
  }
}
```

Note the two-step emit. `finding` is a data type and `json` is a content type,
and an artifact is declared with the latter — so a `finding` cannot be emitted
directly as `result<json>`, and the compiler says so rather than coercing it.

## `ask` is a model call, and its type is checked

```ingot
language 0.1

agent Plan(topic: string) -> plan<json> {
  model requires { structured_output }
  budget { steps <= 4 }
  policy { network deny }

  flow {
    queries = ask<string[]>("Three research queries for ${topic}.")
    emit plan = ask<json>("Turn these into a plan.", context: queries)
  }
}
```

`ask<string[]>` says the model must return an array of strings, and the runtime
holds it to that: a response that does not fit the type fails the node rather
than flowing onward as something else. `context:` passes earlier values in.
`temperature:` is available too.

A placeholder that does not resolve is a compile error, so `${topci}` cannot
silently render as nothing.

## Tools are declared, typed, and carry effects

```ingot
language 0.1

type search_result {
  title: string
  url: string
  snippet: string
}

/// Full-text web search over an MCP server.
tool web.search(query: string) -> search_result[] !network

agent Research(topic: string) -> report<markdown> {
  model requires { tool_calling structured_output }
  budget { steps <= 20 }

  tools {
    mcp web.search
  }

  policy {
    network allow ["arxiv.org"]
  }

  flow {
    hits = call web.search(topic)
    emit report = ask<markdown>("Summarise.", context: hits)
  }
}
```

Three things are separate here on purpose. The `tool` declaration says what the
tool *is* — its argument types, its result type, and the `!network` **effect**:
a statement that calling it reaches the network. The `tools` block says this
agent holds it. The `policy` block says what that agent may do. Effects in 0.1
are `network`, `filesystem_read`, `filesystem_write`, `external_write` and
`secrets`.

Where the tool comes from is a fourth thing, and it is not in the source at all:
`ingot.toml` maps `web.search` onto an MCP server on this machine. The same
artifact runs against a different server without being recompiled.

Delete the `network allow` line and it stops compiling:

```text
error[ING4007]: `web.search` requires the `network` effect, which no policy rule grants
   = note: Ingot is default-deny: an effect with no rule is denied
```

## A policy value is a bound, and it is enforced

`network allow ["arxiv.org"]` names hosts. `filesystem_read allow ["docs"]`
names paths. These are not documentation: with `ingot run --sandbox`, the paths
become mounts and the hosts become a filtering proxy on a network with no other
route out.

Language 0.2 lets a *tool* narrow further than its agent:

```ingot
language 0.2

tool web.search(query: string) -> text !network("arxiv.org")

agent Narrow(topic: string) -> out<markdown> {
  model requires { tool_calling }
  budget { steps <= 8 }
  tools { mcp web.search }
  policy { network allow ["arxiv.org", "github.com"] }

  flow {
    hits = call web.search(topic)
    emit out = ask<markdown>("Summarise.", context: hits)
  }
}
```

The agent may reach two hosts; this tool may reach one of them. A reach wider
than the policy that contains it is a compile error, so the narrowing cannot be
a lie.

## Flow has the control you would expect, and the bounds you would not

```ingot
language 0.1

tool web.search(query: string) -> text !network

agent Fanout(topic: string) -> report<markdown> {
  model requires { tool_calling structured_output }

  tools { mcp web.search }

  memory {
    working ephemeral {
      queries: string[]
    }
  }

  budget { steps <= 60 }
  policy { network allow ["arxiv.org"] }

  flow {
    queries = ask<string[]>("Diverse queries for ${topic}.")
    state.queries = queries
    sources = parallel map queries as query {
      call web.search(query)
    }
    checkpoint "sources-collected"
    emit report = ask<markdown>("Write it up.", context: sources)
  }
}
```

* `parallel map` fans out over a collection. It is the only loop form, and it
  is bounded by the collection — which is what lets `steps` be checked at
  compile time. Its iterations *overlap* where they can, and where they cannot
  they run one at a time — the result and the record are the same either way, and
  [GAP-043](../gaps.md#gap-043) records what still does not overlap.)
* **There is no way to say what happens when a step fails.** A tool that errors, a
  model that cannot be reached, a `verify` that does not hold — each ends the run,
  named at the node it happened on. That is the right answer for an agent whose job
  is to produce something correct or nothing at all, and a wall for one that wants
  a second attempt or a second source. Recorded as
  [GAP-044](../gaps.md#gap-044); the workaround today is to let the run fail and
  decide outside it.
* `memory { working ephemeral { … } }` declares typed state. `state.queries`
  reads and writes it.
* `checkpoint "name"` marks a resumable point in the event stream.
* `emit` is how an output is produced. An output that is never emitted is a
  compile error.

## Verifiers are checks that run, or say they did not

A verifier carries the check, as one boolean expression over its parameters:

```ingot
language 0.2

type source {
  url: string
  title: string
}

type draft {
  body: markdown
  sources: source[]
}

verifier MinSources(d: draft, min: int) = len(d.sources) >= min

agent Careful(topic: string) -> report<markdown> {
  model requires { structured_output }
  budget { steps <= 8 }
  policy { network deny }

  flow {
    found = ask<draft>("Write about ${topic}. Cite your sources.")
    verify MinSources(found, min: 3)
    emit report = found.body
  }
}
```

If the check does not hold, the run **stops there** and the record says which
verifier failed. Note the ordering: the value is bound, checked, and only then
emitted. Write it the other way round and the compiler warns (`ING6007`),
because a check that runs after publication cannot prevent it.

The body is pure on purpose — parameters, field reads, `len`, operators, and
calls to `fn` helpers. It cannot `ask` or `call`. That is what makes a check's
outcome reproducible from the run record alone: replay the run and the same
`verified` event comes back, with no cassette involved. A property that really
needs to reach outside the run — resolving a URL, reading prose — is a `tool`
call, whose result is a value you can then verify.

Which is the limit worth knowing: a check inspects the *shape* of a value.
"Cites eight distinct sources" is only expressible if the sources are a field,
not a claim buried in markdown. [GAP-034](../gaps.md#gap-034) is that limit.

A verifier may still be declared with no body, as Language 0.1 required:

```ingot
language 0.1

verifier CitationCheck(draft: markdown, min_sources: int)

agent Hopeful(topic: string) -> report<markdown> {
  model requires { structured_output }
  budget { steps <= 8 }
  policy { network deny }

  flow {
    draft = ask<markdown>("Write about ${topic}.")
    verify CitationCheck(draft, min_sources: 8)
    emit report = draft
  }
}
```

That still compiles, with a warning (`ING6006`), and the run reports the node as
`notPerformed` — not `passed`. This is the project's general habit: say what did
not happen rather than let silence read as success.

## Projects, packages and imports

`package hello` at the top of a file namespaces its agents, so the IR carries
`hello.Brief` rather than a bare name:

```ingot
language 0.1
package hello.app

agent Named(topic: string) -> out<markdown> {
  model requires { structured_output }
  budget { steps <= 4 }
  policy { network deny }

  flow {
    emit out = ask<markdown>("About ${topic}.")
  }
}
```

Language 0.2 adds project-local imports — `import "./types.ing"` — so records
and tools declared once can be shared across a project's files. See
[Language 0.2 §1](../../specs/language/v0.2.md).

## What the compiler catches before anything runs

* a tool called with the wrong argument type, or with one missing
* a capability the policy denies — or never mentions, which is also a denial
* a prompt placeholder that does not resolve
* an output that is never emitted, or emitted with the wrong content type
* a loop with no static bound, or a flow that cannot fit its `steps` budget
* recursion between agents, which would make budgets uncheckable
* a tool whose declared reach is wider than the policy containing it
* a credential written into source, refused at build before it can leave

Every diagnostic has a stable code, and `ingot explain ING4007` prints the full
explanation of any of them.

## Asking a person

An `ask` goes to a model. A `consult` goes to a person, and its answer is a value
the flow reads:

```ingot
language 0.3

/// Asks a person how to frame a report, then writes it that way.
agent Framing(topic: string) -> report<markdown> {
  budget {
    steps <= 6
    tokens <= 20000
  }

  policy {
    human allow
    network deny
  }

  flow {
    framing = consult(
      "Which framing should the report take?",
      choices: ["technical", "executive", "narrative"]
    )
    emit report = ask<markdown>("Write about ${topic} as ${framing}.")
  }
}
```

Without `choices:` the person types free text. With it they pick, and the runtime
guarantees the value is one of the listed strings — the program said what may
come back, so nothing else can.

The effect is `human`, and like every effect it has to be granted. That grant is
worth more than it looks: **whether an artifact can run unattended becomes a
question you answer by reading its policy**, instead of by starting it and
finding out. CI denies `human`, and an artifact needing a person fails at the
gate naming the question rather than waiting forever on a pipe.

A question is recorded like a model call and replays like one, so an agent with a
person in it still has an offline test:

```bash
ingot run . --record tests/cassettes/framing.json --approvals stdin --events json
ingot run . --provider replay --cassette tests/cassettes/framing.json
```

The replay asks nobody. What it will not do is reuse an answer to a question that
changed — a different question, or different choices, is a digest mismatch and a
loud refusal, the same as an edited prompt. Re-recording that one means asking
somebody again, which is why `consult` is worth being sparing with.

`--yes` cannot answer a question. It approves gates, and there is no safe default
for *which framing should the report take*.

`consult` is refused inside `parallel map` and inside a verifier body. Both rules
already existed, for `emit` and for `ask`, and the reasons transfer unchanged.
See [Language 0.3](../../specs/language/v0.3.md) and
[RFC-0020](../../rfcs/0020-a-person-in-the-loop.md).

## When something fails

Every failure ends the run, and that is usually right: *summarise this document*
should fail loudly rather than invent a summary.

The case where it is wrong is partial data, and it is the most ordinary case there
is. A fan-out over three incidents where one write-up was never filed used to end
the run on the missing file — **after paying for the other two summaries**, because
iterations are drained rather than cancelled so that a failing run's cost does not
depend on the schedule. The run spent the whole fan-out and returned nothing.

`else` gives one attempt a value to use when it fails:

```ingot
language 0.4

tool fs.read_file(path: string) -> string !filesystem_read

/// Digests incident write-ups, including the ones nobody filed.
agent Digest(incidents: string[]) -> digest<markdown> {
  tools {
    mcp fs.read_file
  }

  budget {
    steps <= 20
    tokens <= 40000
  }

  policy {
    filesystem_read allow
    network deny
  }

  flow {
    notes = parallel map incidents as incident {
      writeup = call fs.read_file(incident) else "no write-up was filed"
      ask<string>("Summarise this incident in one line.", context: writeup)
    }
    emit digest = ask<markdown>("Collect these into a digest.", context: notes)
  }
}
```

The iteration no longer fails, so the fan-out no longer fails, and the collected
list keeps **one entry per element** — a missing file becomes a line saying so
rather than a hole you cannot see.

### The value after `else` reaches nothing

It is a literal, a list, something already bound, or arithmetic over those. No
`ask`, no `call`, no `consult`. That restriction is not a limitation waiting to be
lifted; three things depend on it:

* the attempt is still exactly one step, so the `steps` budget the compiler proved
  does not move;
* the fallback reaches nothing, so the policy you read still describes one sequence
  of calls rather than a union over two paths;
* a recording still has one row for the attempt.

If the second path genuinely needs to reach something — *search the web, and if
that fails read the cache* — that is a larger feature and it is not in the language
yet. Run the flow, let it fail, and decide outside the artifact.

### What `else` will not swallow

This is the part worth knowing before you rely on it. `else` absorbs a failure of
the *attempt*: a tool that failed, a provider that could not be reached, an answer
that did not match its type, a sub-agent that failed for one of those reasons.

It will not absorb a capability your policy denies, a budget that ran out, an
approval somebody refused, a `verify` that did not hold, a stale cassette, a
missing API key, or a tool no host provides. Each of those still ends the run.

The first one is the reason the rest of the list exists: if `else` could route
around a denial, `deny` would be advice.

### The record says a default was used

A run that quietly succeeded on a default is a run whose record does not say what
happened, so every fallback emits an event:

```json
{"event":"fallbackTaken","node":"n1","because":"tool"}
```

Counting those tells you how much of a digest was made of defaults, which is the
first thing to ask of one. A digest built entirely from *no write-up was filed* is
a successful run and a useless answer, and the event stream is where you can see
the difference.

### What you cannot put after `else` yet

A fallback has to be a value the language can write, which means a literal type —
`string`, `int`, `float`, `bool`, lists of those — or `text`, which a string widens
into. That covers a tool returning a file's contents and an `ask` for a number,
which is most of what the feature is for.

What it does not cover is prose and records:

```
summary = ask<markdown>("Summarise this.") else "nothing to say"
#         error[ING3001]: the fallback is `string` and the attempt is `markdown`
```

`markdown` is the more specific type, and a bare string does not get to claim it —
if it did, a `text` value would have as good a claim, and then `markdown` and `text`
would be one type with two names. There is no record literal either, so
`ask<rating>(…) else rating { … }` does not parse.

The shape that works is to ask for `string` and let the step that assembles the
document produce the markdown, which is what the example above does. The rest is
[GAP-045](../gaps.md#gap-045).

### One widening came with this

`string` now widens to `text`, joining `int` → `float` and `markdown` → `text`. A
string is text in the same sense markdown is.

It is here because without it the example above did not compile: the filesystem
tool reads a file as `text`, and the alternative was to declare the tool
`-> string` so that a flow could have a fallback — which makes a tool's signature
depend on how somebody uses it.

See [Language 0.4](../../specs/language/v0.4.md),
[Runtime 0.7](../../specs/runtime/v0.7.md) and
[RFC-0022](../../rfcs/0022-a-failure-an-iteration-can-absorb.md).
