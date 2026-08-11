# RFC-0016: The canvas, a two-way view of a flow

- Status: **Proposed**
- Created: 2026-08-11
- Affects: `ingot-studio`, `ingot-language-service`, CLI
- Builds on: [RFC-0015](0015-ingot-studio.md)

## Problem

The studio shows a project. It cannot change one. Someone who does not write
code can watch an agent, read its diagnostics and start a run, and then has to
open an editor and learn a language to change the prompt.

The obvious answer is a node editor, and the obvious node editor is the thing
[RFC-0007](0007-the-ingot-product-loop.md) refused by name: *"a hosted no-code
workflow editor as the source of truth."* That refusal is right and it has not
expired. A canvas that owns the graph and writes the file makes the file an
export — and then the comments in it, the constructs the canvas cannot draw, and
anything a person hand-wrote survive only as long as nobody drags anything.

So the question this RFC answers is not "how do we draw a flow". It is: **what
must an editing surface be, so that it can never be the source of truth?**

## The rule, and how it is kept

> The canvas never produces a file. It produces a **byte range and a
> replacement**, and everything outside that range is untouched by
> construction.

That is the whole design. An edit is a [`TextEdit`][edit] — the same thing an
LSP code action produces — applied to source that is then compiled. Three
consequences follow, and they are the reason this is not the surface RFC-0007
refused:

* **Nothing outside the edited span can be lost**, because nothing outside it is
  written. Not a comment, not a construct the canvas cannot draw, not a blank
  line somebody left deliberately.
* **The canvas is never authoritative about correctness.** It proposes an edit;
  `ingot check` decides. A canvas bug is a compile error rather than a corrupted
  program, and the person sees the same diagnostic they would have seen typing.
* **The canvas may be partial.** It draws what it understands and shows the rest
  as read-only source, in place. A surface that owns the model cannot be partial
  — it has to represent everything or lose it. A surface that owns nothing can.

[edit]: ../crates/ingot-language-service/src/lib.rs

## What a block is

One statement. `Stmt` in [`ingot-syntax`][syntax] is already exactly this, and
every variant carries a `Span` — byte offsets into the file:

| Statement | Drawn as |
|---|---|
| `queries = ask<string[]>("…")` | a model call, with its prompt |
| `hits = call web.search(query)` | a tool call, with its arguments |
| `state.notes = queries` | a write to working memory |
| `verify CitationCheck(draft, …)` | a check |
| `checkpoint "sources-collected"` | a marker |
| `emit report = draft` | an output |
| `if` / `loop` | a container, holding blocks |
| a binding whose value is `parallel map` | a fan-out container |

`Stmt::Error { span }` is drawn too. The parser keeps a statement it could not
understand so that later phases still see the shape of the flow, which means a
file that does not compile still has a canvas — with one block marked unreadable
rather than an empty page.

Note that `parallel map` is an *expression*, so a fan-out is a binding whose
value happens to contain a statement list. The canvas draws the binding as the
container. This is the first place the view and the grammar differ, and the view
loses: what is drawn is a rendering of the statement, never a second model of it.

[syntax]: ../crates/ingot-syntax/src/lib.rs

## What an edge is, and why you cannot drag one

**An edge is derived, never drawn.** There is an edge from block A to block B
when B mentions a name that A binds. That is all. The canvas computes them from
the same scope analysis the compiler runs, and renders them.

This is the decision that makes the canvas a view rather than a model. In an
ordinary node editor the edges *are* the program: you connect two ports and the
tool records a connection. Here the program is a sequence of named bindings, and
an edge is a fact *about* the text. There is no edge to store, so there is
nothing that can disagree with the file.

It follows that **you cannot drag an edge**, and the canvas must not pretend you
can. To make a step read a different value you change which name it reads —
which the canvas offers as a dropdown of the names in scope at that point, and
which lands as a replacement of one `Ident` span. The arrow moves because the
text changed, not the other way round.

## Three granularities, and nothing else

Every canvas gesture is one of these. Anything that is not one of these is not a
canvas gesture.

**1. Replace a leaf.** A prompt, a tool name, an output type, a budget number, a
policy value, a name a step reads. Every one of these is a node with a span —
`StringLit`, `Ident`, `DottedName`, a literal — so the edit is a replacement of
that span and touches nothing else. This is where most of the work is for
somebody who does not write code: a textarea for a prompt is a textarea over
`StringLit.span`.

**2. Move or delete a statement.** Cut the statement's *move unit* (below) and
insert it at a boundary between two statements in the same block, or drop it.

**3. Insert at a boundary.** A new statement from a template, at a position
between two others. The template is canonical source; the person then edits its
leaves with (1).

## The move unit, and the comment problem

A statement's span is exact, and it is **not** what a person means by "this
block". Consider:

```ingot
// The queries decide everything downstream, so they are worth a slow model.
queries = ask<string[]>("Diverse queries for ${topic}.")
```

The comment is trivia. The lexer skips it and keeps no span for it; only `///`
doc comments survive, and only as text attached to the next token. So the AST
cannot tell the canvas that comment exists, let alone that it belongs to the
statement below it.

Moving `queries` by its AST span alone would leave the comment behind, now
describing whatever moved into its place. That is exactly the kind of quiet loss
this design exists to prevent, and it would be caused by the mechanism meant to
prevent it.

**So the move unit is computed from the source text, not from the tree.** From
the statement's span start, walk backwards over whole lines while they are blank
or contain only a comment, and stop at the first line that is neither — or at
the opening brace of the enclosing block. The unit is that range through the end
of the statement's line.

This is the one place the canvas reads raw source rather than the tree, and it
is worth stating plainly: the tree cannot answer this question, because the
information was discarded before the tree existed.

*Alternative considered and rejected:* teach the lexer to keep comment spans as
trivia. It is the better long-term answer and it is a change to the front end of
a compiler whose front end is complete and stable. Reading four lines of text
backwards is not worth that, and if a future change wants trivia for other
reasons, this rule becomes a one-line lookup instead.

## Legality: the canvas proposes, the compiler disposes

A move is illegal when it would put a statement above the one binding a name it
reads, or below one reading a name it binds. The canvas knows enough to refuse
the drop before making it, from the same analysis that drew the edges — so the
common case is a drop that does not land rather than an error afterwards.

But that check is an ergonomic convenience and **nothing depends on it being
right**. The edit produces source; the source is compiled; a mistake is
`ING2003: 'queries' is not in scope` with a span, shown the way every other
diagnostic is shown. This is the property that makes the canvas safe to be wrong
about things.

## Concurrency: the file moved under the view

A canvas holds a rendering of bytes it read some time ago. The file may have
changed since — an editor saved, `ingot fmt` ran, a branch switched. Applying a
byte range computed against the old text to the new text can corrupt it, and
this is the one way this design can lose a person's work.

**So an edit carries the range and the text it expects to find there.** The
studio compares before replacing, and refuses with "this file changed since the
canvas read it; reload" when they differ. Cheap, and it turns the one dangerous
case into a message.

## What the canvas does not do

* **Format.** `ingot fmt` rewrites a whole file, which is precisely the
  operation this design refuses. Inserted templates are canonical already; the
  rest of the file is left exactly as the person wrote it, formatted or not.
* **Keep an undo history.** The file is the state. Editors and version control
  are better at undo than a web page would be, and a second history would be a
  second source of truth about what the program was.
* **Draw everything.** A construct the canvas cannot render is shown as source
  and marked read-only. A construct it cannot render is never one it will
  rewrite — the two are the same rule.
* **Create an agent.** `ingot init` and `ingot new` do that, and they already
  work.

## Open questions

These are genuinely open, and the reason this RFC is Proposed rather than
Accepted.

1. **Does the canvas edit the policy block?** It is not part of the flow, but it
   is the part a non-coder most needs to change and the part where a mistake
   matters most. Editing `network allow ["…"]` through checkboxes is the same
   span-replacement machinery. The argument against is that a policy is the one
   thing that should be slow and deliberate to widen.
2. **How is a leaf edit committed?** Per keystroke is unusable — every keystroke
   recompiles and reflows the canvas. On blur is predictable. On an explicit
   save is safest and most tedious. Leaning towards blur, with the diagnostic
   panel updating behind it.
3. **Is `parallel map` drawn as one container or as a fan-out of N?** One
   container is honest — the collection's size is a runtime fact. N would look
   better and would be a picture of something the program does not say.

## Conformance tests

The property to test is not "the canvas draws correctly". It is that **the file
survives**. So:

* Round-trip: for every reference example, render every statement, apply an
  identity edit to each, and assert the file is byte-identical.
* Move a statement with a comment above it and assert the comment moved with it.
* Move a statement below one that uses its binding, apply the edit anyway
  (bypassing the canvas's own check), and assert `ingot check` reports
  `ING2003` — the compiler is the backstop, and the test proves it.
* Edit a leaf in a file containing a construct the canvas cannot draw, and
  assert that construct is byte-identical afterwards.
* Apply an edit whose expected text no longer matches, and assert refusal.
* A file that does not parse still yields a canvas, with the unreadable
  statement marked.
