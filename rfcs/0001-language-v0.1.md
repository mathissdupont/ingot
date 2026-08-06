# RFC-0001: Language 0.1 kernel

- Status: **Accepted**
- Created: 2026-08-06
- Affects: language, IR

## Problem

An agent has to be written in something. The two available options both give
something up.

**YAML** works until the agent needs control flow, and then it stops. There is
no way to say that a tool takes a `string` and returns a list of records, no way
to catch `${topci}` before it renders as empty text at runtime, and no way to
state that this agent may reach `arxiv.org` and nothing else. The definition is
data, so nothing checks it until something runs it.

**Python frameworks** solve expressiveness and lose portability. The agent
becomes a program against one framework's API, running in one language's
runtime. Moving it elsewhere is a rewrite, and answering "what can this agent
reach?" means reading all of it.

Neither can answer, before execution:

- does this tool call type-check?
- can this agent reach the network, and where?
- can this flow finish inside its step budget?
- does every path produce the declared output?

That is the gap: a definition language small enough to be checkable and
expressive enough to be worth using.

## Goals and non-goals

**Goals**

1. A grammar small enough that the whole language fits in one document, and that
   a model can generate correctly from that document alone.
2. Static verification of types, tool signatures, prompt placeholders,
   capabilities, loop bounds and step budgets.
3. Capabilities and effects as first-class syntax, not comments or convention.
4. One canonical lowering to a target-neutral IR.
5. Enough expressiveness for three real agents: a summariser, a research
   workflow with fan-out, and a multi-agent review team.

**Non-goals for 0.1**

Module imports, optional and union types, generics, user-defined functions,
persistent memory, streaming, dynamic agent creation, transports beyond MCP, and
organisation-level policy composition. Each is a later RFC. Keeping them out is
what makes the semantics of 0.1 statable in one page.

## Proposed syntax

The kernel, exercised in full:

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

agent ResearchAgent(topic: string) -> report<markdown> {
  model requires {
    tool_calling
    structured_output
    context >= 128k
  }

  tools {
    mcp web.search
  }

  memory {
    working ephemeral {
      queries: string[]
    }
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
    state.queries = queries
    sources = parallel map queries as query {
      call web.search(query)
    }
    checkpoint "sources-collected"
    draft = ask<markdown>("Produce a source-grounded report.", context: sources)
    verify CitationCheck(draft, min_sources: 8)
    emit report = draft
  }
}
```

### The ten concepts

`type`, `tool`, `verifier`, `agent`, `model`, `tools`, `memory`, `budget`,
`policy`, `flow`. Nothing else is load-bearing, and every one of them earns its
place by enabling a check.

### Decisions worth arguing about

**Tools are declared separately from being granted.** A `tool` declaration is a
signature; the `tools` block is an authorisation. Splitting them means a call to
an ungranted tool is an error (`ING2005`) and an unused grant is a warning
(`ING2012`), so an agent's reach is visible in its own source and stays minimal.

**Verifiers are declared.** The research document sketched `verify
CitationCheck(...)` with no declaration. A `verifier` declaration costs one line
and makes the call type-checkable, which is the whole point of the language.

**No shadowing, no rebinding.** A name is bound once per agent. This makes the
flow a dataflow graph, which is what lets lowering resolve a reference to an
input or a binding with no scope information in the IR. Values that change use
`state`.

**Default-deny.** An effect with no policy rule is denied, with a diagnostic
distinct from an explicit `deny` (`ING4007` versus `ING4001`). The alternative —
permitting what is not mentioned — makes the artifact's reach depend on what
someone forgot to write.

**`model_access` is implicit.** Every `ask` performs it, and it is always
granted. Requiring `network allow` for the model itself would make every policy
block meaningless, because the allowlist would have to include the provider.

**Loops carry `max`.** Without a static bound, no step budget can be checked
before running. `while` alone is not enough.

**Comparisons do not chain.** `a < b < c` is a syntax error, not a silently
different meaning.

**Layout is not significant.** Newlines separate declarations by convention.
This costs a one-token lookahead in two places — a budget unit versus the next
budget key, and a policy qualifier versus the next subject — and buys a grammar
with no indentation rules.

## IR semantics

Every construct lowers to the flat node graph specified in
[Agent IR 0.1](../specs/ir/v0.1.md):

| Source | IR |
|--------|-----|
| `ask<T>(...)` | `llm.call` with `responseType` |
| `call tool(...)` | `tool.call` with `effects` |
| `call Agent(...)` | `agent.call` |
| `parallel map` | `parallel` with `mode: "map"` |
| `if` / `else` | `branch` with `then` and `else` node ids |
| `loop max N` | `loop` with `maxIterations` |
| `verify` | `verify` |
| `emit` | `artifact.emit` |
| `checkpoint` | `checkpoint` |
| `state.x = v` | `state.write` |
| `state.x` read | `state.read` bound to `$state.x` |
| policy `require approval` | `approval`, inserted by the compiler |

Three lowering decisions:

**Nested calls are hoisted.** `ask("...", context: call t(x))` becomes a
`tool.call` bound to `$tmp0`, then an `llm.call` referring to it. A node's
arguments are therefore always pure values, and a backend never has to evaluate
an expression that performs work.

**Pure bindings are inlined.** `x = topic` produces no node; uses of `x` resolve
to the input directly. The IR contains only nodes that do something.

**State reads are nodes.** `${state.notes}` emits a `state.read` rather than an
inline reference, so state access is auditable in the artifact and a backend
without state support fails at a precise point instead of silently.

## Target lowering

No backend exists yet; this RFC defines what one will receive. The kernel was
chosen so that a first backend has a plausible mapping for every construct:
declarative agent config, MCP tool references, sub-agent hierarchies and
sequential flow are all common ground. `approval`, typed `state` and
`checkpoint` are the constructs most likely to be unsupported, which is why they
are separate node kinds — a capability profile can name them, and a portability
report (M5) can report their loss precisely.

## Security and policy impact

This RFC introduces the entire effect and policy model. The invariants:

- an effect is available only if a policy rule grants it
- a tool's effects are declared with the tool, not inferred
- a sub-agent's effects are bounded by the tools it grants
- `require approval` inserts a node the backend must honour
- policy values must be literals; a policy depending on a runtime value could
  not be checked statically

## Static bounds

Loops require `max`. Recursion between agents is rejected (`ING2014`). The
minimum number of steps on a flow's shortest path is compared against the
`steps` budget (`ING5006`); a bounded loop contributes zero, since it may run
zero times, and an `if` without `else` contributes zero.

## Compatibility

There is no prior version. Every source file declares `language 0.1`, so 0.2 can
change semantics without changing the meaning of anything already written. A
compiler that does not implement a declared version rejects the file rather than
guessing (`ING1020`).

## Alternatives

**Extend YAML with a schema.** Gets validation, never gets control flow, typed
data flow, or diagnostics with source spans. The gap this RFC targets is exactly
the one a schema cannot close.

**Embed a general-purpose language.** Maximum expressiveness, minimum
checkability. Once arbitrary code can appear in the flow, effects cannot be
enumerated and portability cannot be reported.

**Adopt an existing agent workflow DSL.** Several exist and are strong prior
art. None combines static effects, capability-based policy and multi-target
lowering, which is where the value here is. Adapters to them are a better use of
effort than reimplementation, and are planned as backends.

**Start with the IR only, no surface language.** The IR alone is unpleasant to
write. The language exists so a human or a model can produce something that
type-checks.

## Conformance tests

- [x] `accepts_the_reference_agent`
- [x] `rejects_a_wrong_tool_argument_type`
- [x] `rejects_a_denied_network_effect`
- [x] `rejects_a_missing_output`
- [x] `an_absent_policy_rule_denies_the_effect`
- [x] `require_approval_is_reported_as_an_inserted_checkpoint`
- [x] `model_access_never_needs_a_policy_rule`
- [x] `a_tool_must_be_granted_before_it_is_called`
- [x] `a_typo_in_a_prompt_placeholder_is_an_error`
- [x] `an_unbounded_loop_is_rejected`
- [x] `a_flow_that_cannot_fit_its_step_budget_is_rejected`
- [x] `recursion_is_rejected`
- [x] `state_writes_are_rejected_inside_parallel_map`
- [x] `rebinding_a_name_in_the_same_block_is_rejected`
- [x] `emitting_in_both_branches_satisfies_the_check`
- [x] `formatting_is_idempotent`
- [x] `ir_matches_the_golden_files`
- [x] `compiling_twice_produces_identical_bytes`
