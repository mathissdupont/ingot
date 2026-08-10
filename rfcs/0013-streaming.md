# RFC-0013: Streaming

- Status: **Accepted**
- Created: 2026-08-10
- Affects: runtime spec, CLI
- Closes: [GAP-005](../docs/gaps.md#gap-005)
- Opens: [GAP-031](../docs/gaps.md#gap-031) (a contained run does not stream),
  [GAP-032](../docs/gaps.md#gap-032) (the Python backend does not stream)
- Specified in: [Runtime 0.3](../specs/runtime/v0.3.md)

## Problem

Every `ask` is one request that is answered all at once. Two costs follow from
that, and they have the same cause.

**A run shows nothing while it works.** `ingot run` prints a trace — a node
started, a model was called, an artifact was emitted — and between the first two
of those there is a gap the length of a completion. On a summarizer over a long
document that is thirty seconds of nothing, and while it lasts the operator
cannot tell a model that is thinking from a connection that has stalled. The
information exists; the service has been producing text the whole time. It is
being withheld by the shape of the call.

**An answer longer than 16,000 output tokens ends the run.** Every call is
capped there, and a model that reaches the cap returns `ProviderError::Truncated`
rather than a result. The number is not arbitrary and it is not conservatism: a
service that composes an entire response before sending it holds the connection
open for as long as the answer takes, so a large cap over a whole-body transport
is a request that waits and may still time out. Several services refuse a larger
`max_tokens` outright unless the request streams.

So the ceiling is a property of the transport rather than of the artifact, and
the artifact has been paying for a transport limitation it never chose.

The reason this was not done sooner is the question
[GAP-005](../docs/gaps.md#gap-005) asked and did not answer: what a partially
streamed structured response means when it fails validation. Behind that sits a
harder constraint. [Runtime 0.1 §9](../specs/runtime/v0.1.md) requires a
replayed run to produce the same event sequence byte for byte, and cassettes
match by position. A design that puts token deltas into the recorded stream
breaks both, and it breaks them quietly.

## Goals and non-goals

**Goals**

1. **Text appears as the model produces it**, on a live run against a provider
   that can produce it that way.
2. **A longer answer where the transport can carry one.** 64,000 output tokens
   on a streamed call, still 16,000 otherwise.
3. **Determinism is untouched.** A replay produces the same event sequence byte
   for byte, and a cassette matches by position, exactly as before.
4. **A streamed call and a whole-body call produce the same value and the same
   errors** for the same content. Not similar ones — the same ones, from the
   same code.
5. **Every existing provider keeps working unchanged**, without being edited.

**Non-goals**

* **Streaming in the language.** No syntax, no IR field, no artifact metadata.
  An artifact does not know whether it is streaming and must not: the same
  artifact against two transports is the same program.
* **Streaming a contained run.** See *What this does not do*.
* **Streaming in the Python backend.** Same section.
* **Deltas in a cassette.** A recording holds what a model said, not the
  installments it said it in.
* **A determinism guarantee for deltas.** Two runs of one prompt may be chunked
  differently by the same service, and nothing here pretends otherwise.
* **Concurrency.** Streaming makes one call visible while it runs; it does not
  make two calls run at once. [GAP-010](../docs/gaps.md#gap-010) is unaffected.

## Two defaulted methods on the provider

```rust
trait ModelProvider {
    fn name(&self) -> &str;
    fn complete(&mut self, request: &CompletionRequest)
        -> Result<CompletionResponse, ProviderError>;

    fn streams(&self) -> bool { false }

    fn complete_streaming(
        &mut self,
        request: &CompletionRequest,
        on_delta: &mut dyn FnMut(&str),
    ) -> Result<CompletionResponse, ProviderError> {
        self.complete(request)
    }
}
```

Both are defaulted, so every provider that exists compiles and behaves as it
did. The default `complete_streaming` drops the deltas and calls `complete`,
which is the honest answer for a provider with nothing live to show: a cassette
replay produces its answer at once, and inventing deltas for it would make a
replayed run look like a call that never happened.

`streams()` is asked before a request exists, because the answer decides how
many tokens the request may ask for. A provider that dispatches to others — the
`RoutingProvider` that picks by vendor prefix — therefore reports `true` only
when every route it could pick reports `true`. One provider that cannot stream
keeps the smaller ceiling for all of them. That is conservative on purpose: the
alternative is an artifact that asks for more tokens than a service accepts and
fails on a route the operator did not think they were using.

## Two channels, and the difference is the point

`EventSink` gains two methods, also defaulted:

```rust
trait EventSink {
    fn emit(&mut self, event: RunEvent);

    fn delta(&mut self, node: &str, text: &str) {}
    fn settled(&mut self, node: &str, kept: bool) {}
}
```

`emit` carries the **event stream**: the record of what the run did. It is
ordered, it has no timestamps and no durations, and a replay reproduces it byte
for byte. It is the thing tests assert on and the thing a cassette is matched
against.

`delta` and `settled` carry the **live channel**: text as a model produces it.
That is a property of the connection, not of the run. The same run over a
different network produces different fragments in different sizes at different
moments, and none of that is a fact about the agent.

So a delta is never an event, never recorded in a cassette, and never asserted
on. A replay emits none, and that is correct — there is nothing live to watch.

### Why the recorded stream could not gain a new event

It was the obvious first design and it fails on three counts, each of which is
sufficient.

**[Runtime 0.1 §9](../specs/runtime/v0.1.md) would stop being true.** Events
carry no timestamps and no durations precisely so that replaying a recorded
exchange produces the same sequence byte for byte. A recorded run against a
service that sent forty chunks and a replay of it, which sends none, would
differ by forty events. The property that makes an event stream assertable
rather than merely inspectable would be gone for every streamed run.

**Cassette matching would break.** Replay pairs interactions by position
([Runtime 0.1 §10](../specs/runtime/v0.1.md)) and tool calls by position within
their own list ([Runtime 0.2 §2](../specs/runtime/v0.2.md)). Chunk events
interleaved into the stream move every position after them, and they move by an
amount that depends on how a service happened to segment its answer.

**The cross-backend test would fail, correctly.**
`the_event_streams_agree_on_kind_and_order` compares the reference interpreter
against the Python backend on one artifact and one cassette. The Python backend
does not stream. A reference interpreter that emitted chunk events would diverge
from it on every model call, and the honest fix would be to make the Python
backend emit fabricated ones — which is the replay lie again, in a second
implementation.

Keeping the two channels separate makes all three problems not exist rather than
solved. Nothing was added to the event stream, so nothing about it changed.

## A partial answer is not an answer

This is the question GAP-005 asked, and the answer is the least clever option
available.

The value a run uses is **always** assembled from the finished response and
validated whole, by the same code path a non-streamed response takes. Nothing
downstream may parse, repair, coerce or bind a fragment. On any failure — the
answer was cut off, or it did not match its declared type — the accumulated text
is discarded and the run fails exactly as it does today, with
`ProviderError::Truncated` or `ProviderError::InvalidResponse`.

The alternative is salvaging a prefix: take what arrived, see whether it happens
to parse against the declared type, and use it if it does. It was rejected
because it makes a run's result depend on where a connection happened to stop.
Two runs of the same prompt against the same model would produce two different
answers, and the difference between them would be a network event that nothing
records and nobody can reproduce. [Runtime 0.1 §6](../specs/runtime/v0.1.md)
already says that a response which does not validate against the declared type
is an error, and that this is the entire value of having declared it. A prefix
rule would carve an exception into exactly that sentence.

Truncation is the tempting case and the one to be firmest about. The text on
screen when a call hits its limit is the beginning of a real answer — well
formed, on topic, and worth keeping if anything is. It is still discarded,
because "worth keeping" is not a property the run can check and "the model
stopped here" is not the same statement as "the model finished".

### `settled` exists because a watcher was shown something

Discarding the text is right for the run and rude to whoever was reading it. A
half-finished answer left on screen looks like a result.

`settled(node, kept)` closes the live channel for a node and says which
happened. It is called only after at least one delta — with nothing shown there
is nothing to strike — and `kept` is false whenever the response was thrown
away. The reference CLI prints the text, then prints that the text is not the
answer. A UI can strike it, clear it, or mark it; what it must not do is leave
it standing.

## One parser, two transports

Each provider accumulates its streamed pieces into a payload with the same shape
its non-streaming response has, and hands that to the same parse function it
already used. There is one implementation of "what a response means" per vendor,
reached two ways.

This is what makes goal 4 a structural property rather than a promise. A
streamed call and a whole-body call produce identical values for the same
content because the same code produced them, and identical errors for the same
bad content for the same reason. A second parser written for the streaming path
would be a second answer to "did this response validate", and the two would
drift on exactly the inputs nobody tested.

The transport half is shared too: one event-stream reader in the HTTP layer,
which delivers each event's name and parsed payload to the provider and leaves
the meaning to it. Framing — keep-alive comments, `CRLF`, multi-line `data`, the
`[DONE]` sentinel that is not JSON — is decided once.

## The ceiling is a property of the transport

```text
NON_STREAMING_CEILING = 16_000
STREAMING_CEILING     = 64_000
```

The interpreter picks between them by asking `provider.streams()`. The cap on
one call is what remains of `budget.tokens` where the artifact sets one, clamped
to at least 1 and at most the ceiling the transport earned.

An artifact cannot choose this and must not. The same artifact against a
streaming provider and a non-streaming one is the same program; only the second
has to keep the smaller number, and that is not a fact the source should be able
to state, override or depend on.

It stays a ceiling rather than becoming no limit. A `max_tokens` above what a
model accepts is a rejected request, and a number the interpreter picked is
easier to explain than a provider's HTTP 400. Exceeding the ceiling remains
`Truncated`, unchanged.

## Retry stops at the first delivered event

The shared HTTP layer retries a failed request — 429 and 5xx, with a small fixed
backoff. A stream that fails part-way is **not** retried. The caller has already
shown that text to somebody, and a second attempt would repeat it from the
beginning, so a watcher would see the answer start twice and have no way to know
which one the run will use.

Retries therefore cover only the window before the first delivered event, where
nothing has been observed and the attempt is genuinely repeatable. Once a
fragment is out, a failure is a failure.

Resuming a broken stream is not defined and must not be improvised. Concatenating
a second call's answer onto a first call's prefix is a value derived from a
fragment, which is the thing the section above forbids.

## What this does not do

This project records its own limits, and this design has three.

**A contained run does not stream, and keeps the 16,000-token ceiling.**
`ingot run --contained` puts the interpreter inside the boundary and leaves the
provider holding the credential outside it
([RFC-0005](0005-the-contained-run.md)), so a completion already crosses the
supervisor channel. A delta would have to cross it too, as a notification rather
than a reply. That is a protocol change, and it has not been made. A contained
run therefore shows no live text and refuses the longer answer a host run would
complete. Its event stream is identical either way, which is what
[Runtime 0.1 §7.1](../specs/runtime/v0.1.md) requires — deltas are not events,
so nothing there is at risk. Recorded as
[GAP-031](../docs/gaps.md#gap-031).

**The Python backend does not stream.**
`crates/ingot-backend-python/src/prelude.py` makes one whole-body request per
`ask` and keeps the 16,000-token ceiling. A response between 16,000 and 64,000
tokens therefore succeeds on the reference interpreter and fails on the Python
backend. The event streams still agree, because a delta is not an event — but
the accepted answer length differs, and that is a portability difference worth
naming rather than discovering. Recorded as
[GAP-032](../docs/gaps.md#gap-032).

**Deltas carry no determinism guarantee, deliberately.** Two runs of the same
prompt may be chunked differently by the same service, and a run behind a
different gateway almost certainly will be. No conformance test may assert a
delta's boundaries, its size or how many arrived. This is not a gap; it is what
"not an event" means, stated so that nobody later reads the absence of a test as
an omission.

## Security and policy impact

None granted. Streaming adds no effect, no capability and no policy decision. A
model call is `model_access`, which [Runtime 0.1 §7](../specs/runtime/v0.1.md)
grants implicitly, and it was already being made — this changes how the answer
arrives, not whether the call is allowed.

Two boundaries the implementation preserves:

* **A delta never carries a credential**, for the same reason an event never
  does: secrets reach a provider from the environment and are never read back
  out of a response ([Runtime 0.1 §11](../specs/runtime/v0.1.md)). A delta is a
  fragment of the model's answer and nothing else.
* **A delta never reaches standard output.** The reference CLI writes the trace
  and the live text to standard error, because standard output carries the run's
  artifacts and half-finished text must not be spliced into a pipeline reading
  them. In JSON mode a delta line carries no `event` key, so a consumer
  selecting on `event` sees exactly what a replay would reproduce.

## Static bounds

No new execution construct. An `llm.call` still costs one step whether or not it
streamed, and the token budget is accumulated from the same reported usage on
the finished response.

The one interaction worth stating: the per-call cap is the smaller of the
transport ceiling and what remains of `budget.tokens`, so an artifact's own
budget still bounds a streamed call. A streaming transport raises the ceiling
and never raises a budget.

## Compatibility

Additive, and unusually cleanly so. Both new provider methods and both new sink
methods are defaulted, so a backend written against
[Runtime 0.2](../specs/runtime/v0.2.md) satisfies Runtime 0.3 with no edit: it
reports `streams() == false`, keeps the 16,000-token ceiling, shows no live text
and emits precisely the event stream it emitted before.

No event kind is added and no event field changes. `cassetteVersion` stays
`0.2`, the Agent IR version is untouched, and an existing cassette replays to
the same bytes it replayed to yesterday.

The one behaviour change on an existing artifact: against a provider that
streams, a call may now ask for up to 64,000 output tokens, so an answer that
used to end the run with a truncation error may now complete. That is the point
of the change, and it is the direction that turns a refusal into a result rather
than the reverse.

## Alternatives

**A `RunEvent::Delta`, recorded like everything else.** Rejected for the three
reasons above: it breaks [Runtime 0.1 §9](../specs/runtime/v0.1.md), it breaks
cassette position matching, and it breaks
`the_event_streams_agree_on_kind_and_order` in a way whose only repair is a
second backend fabricating events. The attraction was having one channel instead
of two. The price was that the one channel stops being reproducible, which is
the property the whole test strategy rests on.

**Salvage a validated prefix.** Rejected. It makes a result depend on where a
connection stopped, which is the one input to a run that nothing records and
nobody controls. It also has a plausible-looking failure mode: a truncated JSON
object that happens to close early and validate is a wrong answer that passes
every check.

**A separate `StreamingProvider` trait.** Rejected. A provider could implement
one and not the other, so every call site would need both paths and the
interpreter would carry a permanent branch on which kind of provider it holds.
Worse, the two paths would be free to differ — and the whole value here is that
they cannot. Defaulted methods on the existing trait make "does not stream" a
single answer to a single question, given by the same object.

**Let the artifact request streaming.** Rejected. It reads like configuration
and is really a portability hazard: an artifact that says `stream: true` means
something different against a provider that cannot, and the natural
implementations of that are either a refusal — a program that will not run on a
correct backend — or silence, which is worse.

**Raise the non-streaming ceiling to 64,000 as well.** Rejected. It is the same
ceiling with the reason removed. A whole-body transport holds the connection for
the length of the answer, and several services reject the larger cap outright
unless the request streams, so the result would be timeouts and HTTP 400s in
place of a clear limit.

**Do nothing.** Rejected on the second problem rather than the first. A cursor
for thirty seconds is an annoyance; an answer the toolchain cannot produce is a
class of agent that cannot be written.

## Conformance tests

- [x] `a_delta_is_not_an_event`
- [x] `a_provider_that_cannot_stream_answers_at_once_and_shows_nothing`
- [x] `the_event_stream_is_identical_across_replays`
- [x] `the_event_streams_agree_on_kind_and_order` (existing; must keep passing
      with a streaming reference interpreter)
- [ ] `a_streamed_call_and_a_whole_call_produce_the_same_value`
- [ ] `a_streamed_answer_that_does_not_validate_is_discarded_not_repaired`
- [ ] `a_truncated_stream_strikes_what_it_showed_and_fails`
- [ ] `a_streaming_provider_earns_the_larger_output_ceiling`
- [ ] `a_router_keeps_the_smaller_ceiling_unless_every_route_streams`
- [ ] `a_stream_that_fails_after_the_first_event_is_not_retried`
