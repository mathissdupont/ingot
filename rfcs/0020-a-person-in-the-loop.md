# RFC-0020: A person in the loop

- Status: **Draft**
- Created: 2026-08-15
- Affects: language, IR, artifact, runtime, CLI, `ingot-studio`, `ingot-supervisor`
- Closes: [GAP-041](../docs/gaps.md#gap-041), [GAP-042](../docs/gaps.md#gap-042)
- Builds on: [RFC-0005](0005-the-contained-run.md), [RFC-0015](0015-ingot-studio.md),
  [RFC-0018](0018-state-that-outlives-a-run.md)

## Problem

Two entries in the register, and they are the same missing thing.

An artifact with `filesystem_write require approval` in its policy runs at a
terminal and nowhere else. `ingot run` selects `ApprovalMode::Deny` when standard
input is not a terminal, so a run started by the studio, by cron or by CI is
refused at the gate rather than asked. That is GAP-041, and it has the shape of a
joke: *an agent that needs a person cannot be run from the one surface built for
people.*

Underneath it is the larger one. The single interaction point between a run and a
human is not one the program writes — the compiler inserts it, in front of an
effect the policy gates. It carries an effect and a tool, it answers yes or no,
and its answer is not a value the flow can read. An agent that wants to ask
*which of these three framings should the report take* has nowhere to put the
question. That is GAP-042.

So there is a channel with no vocabulary above it, and a vocabulary with no
channel under it. The register says they share a design and building them
separately would give two. This RFC is that design.

## The move

> A person is a third source of answers, recorded and replayed exactly like the
> other two.

That is the whole proposal, and everything below follows from it.

A cassette already holds two independent ordered lists. `interactions` are what
the model answered; `toolCalls` are what the tools returned. Each is matched by
position, then checked against a digest of everything that determined the answer,
so an edited prompt fails loudly instead of quietly reusing the previous row. The
two are kept separate rather than interleaved precisely because they are matched
independently.

A question put to a person has that shape. It sends a prompt and gets back a
typed value; what determined the answer is the text of the question; asking again
after the question changed would be reusing the wrong row. So it is a third list,
`consultations`, matched the same way — and the machinery that matches it exists,
is tested, and does not need to learn anything new.

**The objection, stated plainly.** A model call is cheap to redo and a person is
not. Re-recording a cassette full of `ask`s costs tokens; re-recording one that
contains a consultation costs somebody's afternoon. That is a real difference and
it is a difference in *cost*, not in *structure* — and structure is what the
cassette encodes. The cost has consequences, and they are dealt with under
["What re-recording costs"](#what-re-recording-costs) rather than by pretending a
person is a service.

## What the program writes

`consult` is an expression, beside `ask` and `call`:

```ingot
framing = consult<string>("Which framing should the report take?")
```

The common case is not that one. It is this one:

```ingot
framing = consult(
  "Which framing should the report take?",
  choices: ["technical", "executive", "narrative"],
)
```

And a question that needs the run's own work in front of it says so, with the
same named argument `ask` uses:

```ingot
approach = consult(
  "The draft argues two things and they do not sit together. Which do we keep?",
  context: [draft],
  choices: ["the cost argument", "the risk argument"],
)
```

`consult` takes one positional argument — the question — plus optional named
arguments `context` and `choices`. Its effect is `human`.

### Why `context` is not optional decoration

A person's answer can depend on something they saw earlier — the draft that went
past, the search results, the state of the world at the moment they were asked.
If that dependency is real and the digest does not cover it, then two runs with
identical question text can legitimately deserve different answers, and the
cassette would replay the first into the second without noticing.

So the rule is the one `ask` already keeps: **whatever determined the answer is
an argument, and every argument is in the digest.** A question whose answer
depends on the draft passes the draft. This is not a new discipline; it is the
existing one, and the reason it transfers is that it was never really about
models.

### Typing an answer

`consult<string>` yields text a person typed. `choices:` yields one of the listed
strings, and the runtime guarantees it — the person picked from a list rather
than typing something that has to be validated afterwards.

There is deliberately no `consult<Report>`. A model can be schema-constrained
into a struct; a person confronted with one is being asked to type JSON, which is
exactly the failure [RFC-0016](0016-the-canvas.md) named when it decided the
canvas may widen a policy: a surface that leaves somebody stuck sends them to
paste something they do not understand, which is worse than the thing it was
protecting them from. So the answer type is `string`, or one of a fixed list of
strings, and a program that wants structure asks a model to build it from what
the person said.

*The better answer, deferred.* With literal types in the union machinery from
[RFC-0009](0009-language-v0.2-optionals-and-unions.md), the choice form would
yield `"technical" | "executive" | "narrative"` and a `match` over it would be
exhaustively checked. That is worth having and it is a change to the type system,
not to this feature. The choice form is designed so that adopting it later
narrows a type rather than changing a syntax.

## `human` is an effect

It joins the table in [Language 0.1 §7](../specs/language/v0.1.md):

| Effect | Meaning |
|--------|---------|
| `human` | puts a question to a person and waits for the answer |

Default-deny applies unchanged: an artifact that calls `consult` without `human`
granted in its policy is `ING4007`, before it runs.

This is the part that earns the most. **"Can this artifact run unattended?"
becomes a question you answer by reading its policy**, rather than by running it
and finding out. CI denies `human`; an artifact that needs one fails at the gate
with a sentence naming the question, instead of hanging on a pipe until the job
times out. A scheduler can refuse to enqueue what it cannot answer.

**`human` takes `allow` and `deny` and not `require approval` (`ING4011`).** An
approval gate in front of a question is a question in front of a question, and
the person answering the first is the person who would answer the second.

### A sub-agent may ask, and its caller must have said so

A sub-agent call's effects are bounded by the union of the effects of the tools
that agent grants ([Language 0.1 §7](../specs/language/v0.1.md)). That is a rule
about tools, and `human` is not a tool, so it has to be said rather than
inherited: **`human` propagates like every other effect, and a caller whose
policy does not grant it cannot call an agent that consults.** The channel is
passed down; the policy is checked at the call site, as always.

The alternative — a sub-agent that inherits its caller's channel while its
caller's policy never mentions `human` — would let an agent acquire a person's
attention by delegation. That is the shape of every capability escape this
project refuses, and it does not become acceptable because the thing reached is a
person rather than a socket. If anything it is worse: a socket does not get
tired.

## What `--yes` can answer, and what it cannot

`ApprovalMode::AssumeYes` exists because an operator may say in advance that
every gate in this run is approved. It has no counterpart here. There is no
default answer to *which framing should the report take*, and inventing one —
first choice, empty string — would put a value into the flow that no person
chose and no recording holds.

So `--yes` answers approvals and **fails at a consultation**, naming the node and
the question. The asymmetry is not an inconsistency; it is the difference between
a decision with a known safe side and a decision without one.

Which leaves the honest question: how does an artifact containing a `consult`
run in CI at all? The same way one containing an `ask` does. **It replays.** A
consultation in CI is served from the cassette, never asked, and that is not a
workaround — it is the identical bargain the project already strikes for the
model and for tools.

## Recording and replay

A recording adds one list, in the shape of the two beside it:

```json
{
  "cassetteVersion": "0.3",
  "consultations": [
    {
      "index": 0,
      "node": "n7",
      "questionDigest": "…",
      "question": "Which framing should the report take?",
      "choices": ["technical", "executive", "narrative"],
      "answer": "executive"
    }
  ]
}
```

`question` and `choices` are recorded beside the digest even though the digest
would be enough to match. A cassette is checked in and reviewed, and a reviewer
looking at `"answer": "executive"` needs to see what was asked without running
anything. This is the same reason `Interaction` carries `model`.

Cassette version moves to 0.3. A 0.2 recording is a valid 0.3 one with no
consultations in it, so every existing cassette keeps replaying — the rule 0.2
already established for `toolCalls`, applied again, including its converse: a
recording that states 0.2 and carries consultations is refused rather than
replayed.

Replay matches by position, then by digest, and a mismatch is the message the
other two lists give: *the question changed since recording — re-record the
cassette and review the diff*.

### What re-recording costs

For a model, "re-record" means spend tokens. For a person it means **ask
somebody again**, and no flag makes that cheaper. Three consequences, and they
are design constraints rather than warnings:

- **`consult` should be rare, and the language should not make it comfortable.**
  It is an expression with an effect that must be granted, not a convenience.
- **The choice form is more stable than the prose form.** A fixed question with a
  fixed option list survives edits to surrounding prompts that a free-text
  question does not. That is an argument for `choices:` being the common case,
  which is why it is shown first above.
- **`--record` re-records everything.** A cassette whose model prompts changed
  but whose questions did not still costs a person their answers again. This is
  the strongest argument in this RFC for a future partial re-record, and it is
  deliberately not solved here: the right shape for it is unclear, and guessing
  would put a second matching rule beside the one that works.

## Pause, not resumption

[GAP-042](../docs/gaps.md#gap-042) asks whether a run that stopped for a person
is a resumption rather than a pause. It is a pause, and the reason is already
settled.

[RFC-0018 §4](0018-state-that-outlives-a-run.md) made only a top-level
`checkpoint` resumable, because a snapshot is a JSON document a person can read
and that requirement is what rules out serialising a continuation. A `consult`
can appear anywhere an expression can — inside a branch, inside a loop, halfway
down a nested block. Making it a resumption point would mean snapshotting at
arbitrary depth, which is the thing RFC-0018 refused, and it would be refused
again for the same reason.

So **the run holds.** The node blocks on the channel exactly as a model call
blocks on a socket, and the process stays alive.

**And a pause that has to survive the operator going home is a `checkpoint`
before the question.** The language already has the tool: the run stops at the
checkpoint, the snapshot is written, somebody resumes it tomorrow and the
question is asked at the top of the second half. Nothing new is needed for the
long case, and the vocabulary that handles it is the vocabulary that was already
there for every other reason a run outlives a sitting.

*The consequence, recorded rather than hidden:* a run blocked on a person holds a
process indefinitely, and nothing times it out. That is correct — **a person is
not a service and a deadline on one is a deadline on the wrong thing** — but a
forgotten question is then an invisible held process. So the obligation lands on
the surfaces: the studio lists a run blocked on a question *as* blocked on a
question, with the question in the list, and `ingot runs` says the same. A
forgotten question must be visible; it must not be cancelled by a clock.

## The channel

`ApprovalHandler` becomes `Interlocutor`, with the method it had and one more:

```rust
pub trait Interlocutor {
    fn approve(&mut self, request: &ApprovalRequest) -> bool;
    fn consult(&mut self, request: &ConsultRequest) -> Result<Value, ConsultError>;
}
```

`ApprovalMode` is renamed `HumanChannel` and keeps its three shapes: `Ask` around
an `Interlocutor`, `AssumeYes` (which approves and refuses to consult, as above),
and `Deny` (which refuses both, naming which it refused).

**The transport already exists.** [RFC-0005](0005-the-contained-run.md) carries
an approval out of a contained run and a decision back in, over line-delimited
JSON-RPC, because there is nobody inside the box to ask. `CALL_CONSULT` joins
`CALL_APPROVAL` beside it. What is new is not the protocol but *where it is
spoken*: `ingot run --channel-stdio` makes an ordinary run speak it, so a parent
process — the studio's launcher, or anything else — can answer without a
container in the picture.

That inverts nothing about the existing design. A run with no channel and no
terminal behaves as it does today: it refuses at the gate, with a message.

### What the stream says

Two events join `RunEvent`, beside the two an approval already emits:

- `ConsultationAsked { node, id, question, choices }`
- `ConsultationAnswered { node, id, answer }`

These are not decoration, and leaving them out would break the half of this RFC
that motivates it. **RFC-0015's whole claim is that the studio can watch a run**,
and a run stopped at a question with nothing in the stream is a run that looks
hung — indistinguishable from the failure mode this design exists to remove. The
pending question reaches the page the way every other fact about a run does, over
the stream that was already there. The channel carries the answer back; the
stream is what says one is wanted.

`id` is the question id an answer must name, so the two halves of an exchange are
joinable in a recorded stream as well as a live one — which is what lets
`ingot trace` show a question and its answer together after the fact.

## The studio surface

This is where RFC-0015's refusal has to be re-examined, because it is the reason
the gap exists. RFC-0015 kept `--yes` out of the argv the launcher builds, on the
grounds that *a button in the same flow as building gets clicked the way a
notification prompt gets clicked*. That reasoning holds and this RFC does not
weaken it. `--yes` stays out of the argv, and no field puts it there.

What changes is that refusing `--yes` no longer means refusing to answer. The two
were conflated because there was only one mechanism; with a channel they come
apart:

- **`--yes` is a blanket answer given before the run.** It approves gates nobody
  has seen yet. That is what RFC-0015 refused and it stays refused.
- **An answer over the channel is one gate, seen, at the moment it is reached.**
  The page renders the effect, the reason and the node, and the person answers
  that one. The next gate asks again.

This is the same distinction [RFC-0016](0016-the-canvas.md) drew for policy
edits: the objection was never to a person granting something, it was to granting
something they had not been shown. *"It is the difference between `allow
internet` and `network deny` → `network allow ["arxiv.org"]`, and the second is
what the file will say."*

Two mechanical requirements follow from the studio's existing guard:

- **An answer names the question it answers.** A pending question carries an id;
  an answer that names an id which is not the one outstanding is refused rather
  than applied. Without it, a stale tab answers whatever gate happens to be open
  now — the same class of bug the canvas's expected-text check exists to prevent.
- **The route is guarded exactly as every other route is.** Session token, the
  loopback `Host` check with the port, `deny_unknown_fields`. A route that
  unblocks a run is not a new trust boundary and must not become one.

## IR and versions

A new node kind, `Consult`, carrying `question`, optional `context` node
references, optional `choices`, and the binding it produces. Its effect is
`human`.

- **IR version moves to 0.3.** A new node kind is a document an 0.2 reader must
  not accept, and refusing beats mis-reading.
- **Language version moves to 0.3.** `consult` is a new reserved word.
- **Runtime spec moves to 0.6.**
- **Cassette version moves to 0.3**, as above.

**One compatibility break, and it is small but real.** Effect names and other
identifiers are ordinary identifiers today, so a program that binds a name
`consult` compiles now and stops compiling under 0.3. It is a keyword or it is
not; a contextual keyword here would make the grammar depend on what is in scope,
which costs more than the break does.

## Static bounds

A `consult` costs one step, like any node. A `consult` inside `loop max 10` can
ask ten questions, bounded by the loop bound the language already requires — so
nothing new bounds it, and nothing new needs to.

## Where a `consult` may not appear

Two places, and both already have the rule and the reason written down.

**Inside `parallel map` (`ING6008`)**, joining `emit`, `checkpoint` and state
writes. Those three are refused because iterations are concurrent and they would
make the result depend on scheduling. Questions to a person are worse than
order-dependent: the order is *observable to the person*, who sees three
questions arrive at once with no way to know which branch each belongs to. The
existing rule covers it and the existing reason is the right one.

**Inside a verifier body (`ING2019`)**, joining `ask`, `call`, `parallel map`,
working memory and `emit`. [Language 0.2 §10.1](../specs/language/v0.2.md) gives
the reason and it needs no extension: *"a verifier's outcome has to be
reproducible from the run record alone, and a body that could reach outside the
run would not be."* A question reaches outside the run — that is the entire point
of it — so the existing sentence decides this case without being edited. A check
that genuinely needs a person is a `consult` in the flow followed by a `verify`
of what came back.

## Target lowering

Both backends lower `Consult` to a call on their host's channel and block on the
reply — the reference runtime through `Interlocutor`, the Python backend through
the same JSON-RPC method the supervisor protocol defines.

**A backend with no channel must refuse the artifact at build time**, reporting
`human` as an unsupported effect, rather than accepting it and failing at the
node. An artifact that builds and cannot run is the failure mode the effect
system exists to prevent, and a backend gets no exemption from it.

## Security and policy impact

One new effect, `human`, default-denied like every other. It grants nothing else:
a run that may ask a person may not thereby write a file.

Two properties worth stating because they are structural rather than decisions
anybody has to keep making:

- **An answer is data, not a capability.** A person choosing `"executive"` binds
  a string. It cannot widen a policy, name a tool or reach anything — the flow
  does with it what the flow does with any string, under the same checks.
- **A consultation cannot bypass an approval.** They are separate nodes with
  separate effects. An artifact that asks *may I write this file?* through
  `consult` and then writes it still meets the `filesystem_write` gate, because
  the gate is on the effect and not on whether somebody was asked something
  nearby.

**A person's answer is written down.** It lands in the event stream, in the run
history and in a cassette that is checked in and reviewed by other people. The
free-text form is exactly where somebody types something they would not have
committed — a name, a path, a key they were holding. This is the same review
burden the cassette format already carries for tool results, and it gets the same
treatment: **the build-time secret scan, which already reads cassettes as well as
source, reads `consultations` too.**

It is also one more argument for the choice form. When the possible answers are
in the source before anybody is asked, the recording can contain nothing that was
not already committed — the leak is not mitigated, it is structurally absent.

The channel itself is the new attack surface, and it is the studio's existing one
plus a route. The guard is unchanged and the question-id rule above is what keeps
a stale answer from landing on a fresh question.

## What this does not do

- **Make an Ingot agent conversational.** `Agent(inputs) -> outputs` is
  unchanged. A consultation is a node in a flow that was fixed before the run
  started, not a turn in a dialogue, and the flow does not branch on having been
  asked. GAP-042 was right that a chat is a second program model; this does not
  add one.
- **Let a person volunteer something.** The run asks; a person answers. There is
  no inbound message with no question in front of it, because there is nowhere in
  a fixed flow for one to land.
- **Time a person out.** See above — the surfaces make a pending question
  visible, and a human decides.
- **Re-record part of a cassette.** Named as the cost it is, and left open.
- **Give the terminal a new surface.** `ingot run` at a terminal already prompts
  for approvals; it gains a question prompt in the same place, and that is all.

## Alternatives

**Do nothing.** The status quo is defensible for approvals — a terminal is a real
surface — and it is not defensible for questions, which have no surface at all.
It also leaves the studio in the position GAP-041 names, where the surface built
for people is the one that cannot serve an artifact that needs one.

**A tool instead of an expression.** `call human.ask(question)` needs no new
keyword, no language version and no IR node. It was rejected because a tool's
answer is recorded in `toolCalls` and a tool is served by a host — so the design
would either lie about what a person is, or special-case one tool name
throughout the runtime, which is a keyword with extra steps and no compile-time
`human` effect to read off the policy. The thing that makes the effect worth
having is that you can answer *can this run unattended?* by reading the source,
and a tool call does not give you that.

**Make a consultation a resumption point.** Rejected under
["Pause, not resumption"](#pause-not-resumption): it requires the snapshot at
arbitrary depth that RFC-0018 refused, and `checkpoint` already covers the case
that needs it.

**Record the person's answer as a model interaction.** It would need no new
cassette list and would replay today. Rejected because a reviewer reading a
cassette could no longer tell which answers a machine produced and which a person
did, and that is the single most important thing to know about a recorded run.
The lists are separate for the same reason `toolCalls` is separate.

**Let `--yes` answer a consultation with the first choice.** Rejected: it puts a
value nobody chose into a flow and a recording, and it would make the CI story
"the artifact ran" when nothing answered.

## Conformance tests

The property is not "a person can be asked". It is that **a run with a person in
it is as reproducible as one without**. So:

- [ ] A recorded consultation replays with no channel attached, no prompt shown,
      and the same bound value.
- [ ] A replay whose question text changed is refused, naming the node, and the
      message says re-record.
- [ ] A replay whose `context:` argument changed is refused, though the question
      text is byte-identical — the digest covers what determined the answer.
- [ ] A replay whose `choices:` list changed is refused.
- [ ] A 0.2 cassette replays unchanged under 0.3; a cassette declaring 0.2 and
      carrying consultations is refused.
- [ ] Consultations and interactions are matched independently: an agent that
      asks a model twice between two questions replays correctly.
- [ ] `consult` without `human` in the policy is `ING4007`, before the run.
- [ ] `human require approval` is `ING4011`.
- [ ] `consult` inside `parallel map` is `ING6008`.
- [ ] `consult` inside a verifier body is `ING2019`.
- [ ] An agent calling a sub-agent that consults, whose own policy does not grant
      `human`, is refused before the run.
- [ ] `ConsultationAsked` reaches the event stream before the run blocks, and
      `ConsultationAnswered` carries the same `id`.
- [ ] A replayed run emits both events, so a recorded stream and a live one
      contain the same sequence.
- [ ] The secret scan reads `consultations` and fails a package whose recorded
      answer carries a secret shape.
- [ ] `--yes` approves an approval gate and fails at a consultation, naming the
      question.
- [ ] A run with no channel and no terminal refuses at a consultation with a
      message naming the question, and does not hang.
- [ ] The choice form binds one of the listed strings and nothing else, including
      when a channel returns something not on the list.
- [ ] A run blocked on a question appears in `ingot runs` as blocked, with the
      question.
- [ ] An answer naming a question id that is not outstanding is refused.
- [ ] A `checkpoint` before a `consult` produces a snapshot; the resumed half asks
      the question and completes.
- [ ] An artifact using `consult` is refused at build time by a backend that
      declares no `human` support.
