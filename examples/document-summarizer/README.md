# document-summarizer

The smallest complete Ingot agent: typed inputs, one model call, one artifact.
No tools, so it needs no capability grants — and it is the example that runs
end to end today.

## Check and build

```bash
ingot check
ingot build          # -> target/ingot/DocumentSummarizer.ir.json
```

## Run it

```bash
export ANTHROPIC_API_KEY=...
ingot run \
  --input document=@some-document.txt \
  --input "audience=engineering leads"
```

The artifact declares `-> summary<markdown>`, so the runtime asks for prose and
writes the answer out as markdown. Its `policy` block denies everything, which
the interpreter re-checks at run time: this agent cannot reach the network, the
filesystem, or anything outside itself, whoever runs it.

## Test it offline

`tests/cassettes/brief.json` is a recorded run. `ingot test` replays it with no
API key and no network:

```bash
ingot test
```

The cassette stores the inputs alongside the recorded exchange, so it is
self-contained, and replay verifies a digest of each request — editing the
prompt in `main.ing` makes the test fail loudly rather than quietly reusing a
stale answer.

### Re-recording

```bash
ingot run \
  --input document=@some-document.txt \
  --input "audience=engineering leads" \
  --record tests/cassettes/brief.json
```

Review the diff before committing it. A cassette is checked-in test data: it
contains whatever the inputs contained.
