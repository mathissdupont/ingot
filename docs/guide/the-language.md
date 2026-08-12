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
  compile time. (It currently *executes* sequentially; the result is identical
  and [GAP-010](../gaps.md#gap-010) records the difference.)
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
