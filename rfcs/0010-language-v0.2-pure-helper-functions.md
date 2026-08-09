# RFC-0010: Language 0.2 pure helper functions

- Status: Draft
- Author(s): Heptapus Group
- Created: 2026-08-09
- Affects: language, type checker, lowering, editor tooling

## Problem

Ingot flows can bind intermediate values, but they cannot name a small reusable
transformation. As agents grow, authors repeat pure expressions such as numeric
normalisation, small field selections, or prompt-context shaping. Copying those
expressions makes the source noisier and makes edits drift.

The language should let authors extract a tiny pure transformation without
turning Ingot into a general-purpose programming language.

## Goals and non-goals

Goals:

- Add named expression-only helper functions.
- Keep helpers pure: no `ask`, no tool calls, no agent calls, no state access
  requirement, and no emitted artifacts.
- Keep Agent IR compatible by inlining helper calls during lowering.
- Let helpers participate in editor completion, hover and definition.

Non-goals:

- General statement-bodied functions.
- Recursion.
- Helper-to-helper composition in the first implementation slice.
- Side effects, host-language escape hatches, I/O, async, exceptions or dynamic
  dispatch.
- Generic helper functions.

## Proposed syntax

```ingot
language 0.2

fn bump(n: int) -> int = n + 1

agent A(count: int) -> report<markdown> {
  flow {
    next = bump(count)
    emit report = ask<markdown>("done", context: next)
  }
}
```

Grammar:

```ebnf
function-decl = "fn" , IDENT , params , "->" , type , "=" , expression ;
primary       = IDENT , args ;    // when IDENT names a declared helper
```

Function declarations are top-level declarations. They require `language 0.2`
or newer.

## Static semantics

A helper has a fixed parameter list and declared return type. Arguments may be
positional or named, using the same argument matching discipline as tools,
verifiers and agents.

The body is a single expression. It may use:

- parameters;
- literals;
- lists;
- field/path reads from parameter values;
- builtin pure functions such as `len`;
- unary and binary operators.

The body may not use expressions that lower to agent work. `ask`, `call`,
`parallel map`, and helper-to-helper calls are rejected in the first slice.

The body's inferred type must be assignable to the declared return type.

## IR semantics

Helper functions do not appear in Agent IR. A helper call is lowered by
substituting the lowered argument values into the helper body and lowering the
body as a pure `Value`.

This requires no helper-specific Agent IR schema change. No new node kind is
introduced, and helper calls consume no step budget.

## Target lowering

Reference interpreter:

- receives no helper declarations at runtime;
- evaluates the inlined pure IR value it already understands.

Python backend:

- receives the same inlined pure IR value;
- has no new portability limitation for helpers themselves.

If a future helper feature stops being erasable before IR, that feature needs a
separate IR and backend RFC.

## Diagnostics and formatter

Using `fn` under `language 0.1` is rejected with `ING1020`.

Effectful helper bodies are rejected with `ING2019`.

The formatter prints helpers after verifiers and before agents:

```ingot
fn bump(n: int) -> int = n + 1
```

## Compatibility and migration

Existing Language 0.1 source is unchanged.

Migration is mechanical:

1. identify a repeated pure expression;
2. extract it as `fn name(args) -> type = expression`;
3. replace copies with `name(args)`;
4. run `ingot check` and verify the emitted IR remains structurally equivalent
   apart from pure value inlining.

## Alternatives

**Do nothing.** Keeps the language smaller but leaves repeated pure expression
friction unsolved.

**Statement-bodied functions.** More familiar, but too close to a general
language. It also raises control-flow, budget and effect questions this slice
does not need.

**Lower helpers into IR function calls.** More direct, but every backend would
need a new evaluator construct. Inlining keeps helpers a source-language
convenience.

**Allow recursion.** Recursion breaks static bounds unless a separate bounded
recursion model exists. Use `loop max N` in agent flow instead.

## Conformance tests

- [ ] `pure_helper_declaration_parses_and_formats`
- [ ] `language_0_1_rejects_function_declaration`
- [ ] `helper_call_type_checks_positional_arguments`
- [ ] `helper_call_type_checks_named_arguments`
- [ ] `helper_return_type_must_match_body`
- [ ] `helper_body_rejects_ask`
- [ ] `helper_body_rejects_tool_call`
- [ ] `helper_call_lowers_by_inlining_value`
- [ ] `helper_call_consumes_no_step_budget`
- [ ] `editor_completion_includes_helper_function`
