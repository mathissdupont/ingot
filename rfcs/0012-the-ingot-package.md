# RFC-0012: The Ingot package

- Status: **Accepted**
- Created: 2026-08-09
- Affects: artifact, CLI
- Closes: [GAP-004](../docs/gaps.md#gap-004), [GAP-015](../docs/gaps.md#gap-015)
- Delivers: M6, work package P10 of [RFC-0007](0007-the-ingot-product-loop.md)

## Problem

`ingot build` writes `target/ingot/ResearchAgent.ir.json` and stops. Everything
after that — moving the artifact to the machine that will run it, knowing which
artifact you are looking at, knowing whether it is the one you tested — is left
to whoever is holding it.

That gap is not cosmetic. Three specific things are currently impossible:

**You cannot name an artifact.** There is no identity but a file path. Two
`ResearchAgent.ir.json` files from two commits are indistinguishable without
diffing them, and "the version we shipped" is a claim about a directory rather
than a fact about bytes.

**You cannot tell what an artifact was built against.** The IR records the agent.
It does not record which compiler produced it, which source it came from, which
tool servers the project routes, or which image a contained run expects. Those
are exactly the inputs whose identity decides whether a run elsewhere means the
same thing as the run here.

**Nothing checks that a secret stayed out.** [SECURITY.md](../SECURITY.md) states
that secret values never enter source, IR, a lockfile or a layer. That holds
today by construction — there is no syntax for a secret literal and no path from
the environment into the IR — but as [GAP-004](../docs/gaps.md#gap-004) says, an
argument is not a test. Packaging is the moment the argument stops being enough,
because packaging is the moment the bytes leave the machine.

[ADR-0004](../docs/adr/0004-canonical-ir-encoding.md) already did the hard part.
The IR has exactly one valid encoding, and it was chosen for this:

> A digest is only meaningful if the bytes are reproducible: the same source and
> the same compiler must produce the same file, on any machine, on any run.

What is missing is the envelope around it.

## Goals and non-goals

**Goals**

1. **One command produces a movable artifact.** `ingot package` writes a
   directory an existing OCI tool can push, with no Ingot-specific transport.
2. **The package is the tested bytes.** The Agent IR blob is byte-identical to
   what `ingot build` wrote and what `ingot test` replayed against. Not a
   re-encoding, not a re-serialisation — the same bytes.
3. **A reproducible digest.** The same source, the same manifest and the same
   compiler produce the same package digest on Linux, macOS and Windows.
4. **A lockfile that records identity, not content.** What the artifact was built
   from and what it expects at run time, by digest and by name.
5. **A build-time secret scan** over source, IR and cassettes, which refuses
   rather than warns.
6. **Digest-pinned images are verified**, so a contained run can state which
   image it means and be told when that is not what is present.

**Non-goals**

* **A registry.** [ADR-0002](../docs/adr/0002-compiler-not-runtime.md) already
  settled this: OCI registries exist and are good. This RFC produces the standard
  on-disk layout and stops there. `oras`, `skopeo` and `crane` push it.
* **Signing.** A signature needs a trust root, a key custody story and a
  revocation story, none of which are a compiler's to invent. This RFC defines
  *where* verification belongs and what it must cover, and implements the digest
  half. See *Automatic image acquisition stays closed*.
* **Packaging source.** The package carries checked Agent IR and declared
  metadata. Source stays out; its digest goes in the lockfile. See
  *Why source is not in the package*.
* **Packaging cassettes.** A cassette holds whatever a model said. It is a test
  fixture that belongs in the repository, not in a distributed artifact.
* **A second artifact format.** Nothing here is addressable except through OCI.

## What a package is

An **OCI image layout** directory ([image-spec 1.1][layout]) holding one
**artifact manifest**. Nothing about it is Ingot-specific except the media types:

```text
target/ingot/package/
  oci-layout                     {"imageLayoutVersion": "1.0.0"}
  index.json                     one descriptor, pointing at the manifest
  blobs/sha256/<hex>             every blob, named by its own digest
```

The manifest is an ordinary `application/vnd.oci.image.manifest.v1+json` with an
`artifactType`:

```json
{
  "annotations": {
    "org.opencontainers.image.title": "research-agent",
    "org.opencontainers.image.version": "0.1.0"
  },
  "artifactType": "application/vnd.ingot.package.v1+json",
  "config": {
    "digest": "sha256:…",
    "mediaType": "application/vnd.ingot.package.config.v1+json",
    "size": 412
  },
  "layers": [
    {
      "annotations": {
        "dev.ingot.agent": "heptapus.examples.research.ResearchAgent",
        "org.opencontainers.image.title": "ResearchAgent.ir.json"
      },
      "digest": "sha256:…",
      "mediaType": "application/vnd.ingot.agent-ir.v1+json",
      "size": 8134
    },
    {
      "annotations": { "org.opencontainers.image.title": "ingot.lock" },
      "digest": "sha256:…",
      "mediaType": "application/vnd.ingot.lock.v1+json",
      "size": 733
    }
  ],
  "mediaType": "application/vnd.oci.image.manifest.v1+json",
  "schemaVersion": 2
}
```

The key order is alphabetical because every document a package writes is, which
is how "sorted object keys" stops being a convention someone has to remember.

The **package digest** is `sha256` of the manifest bytes, which is what OCI
already means by the digest of a thing. `ingot package` prints it.

[layout]: https://github.com/opencontainers/image-spec/blob/main/image-layout.md

### Why blobs rather than tar layers

A runnable image needs tar layers because a kernel needs a filesystem. An
artifact does not. OCI 1.1 lets a layer be any blob with any media type, which
is what `oras` has been doing in practice, so an Agent IR document can be a layer
directly.

That is not a shortcut, it is the reproducibility argument. A tar header carries
mtimes, uids, gids and modes — four fields that differ between machines and none
of which mean anything here. Getting a byte-stable tar means zeroing all of them
and sorting the entries, which is writing a deterministic tar writer in order to
throw away everything tar adds. A raw blob has no such fields to normalise, and
its digest is the digest of the file the user already has.

There is no compression, for the same reason: a compressor's output depends on
its version and its settings, and the artifact is small.

## The reproducibility rules

These are what make the digest mean something. Every one is a MUST in
[the specification](../specs/image/v0.1.md); the reasoning is here.

**No timestamps.** `org.opencontainers.image.created` is deliberately absent. A
creation time makes every rebuild a different artifact, which turns a digest from
an identity into a serial number. Tools that want one can add it after the fact;
the package cannot claim reproducibility and stamp the clock into itself.

**No paths from the build machine.** A layer's title is a file name, never a
path. `C:\Users\…` and `/home/…` are the same artifact.

**Canonical JSON everywhere.** The config, the lockfile and the portability
report follow the [ADR-0004](../docs/adr/0004-canonical-ir-encoding.md) rules the
IR already follows: sorted keys, two-space indentation, one trailing newline,
decimal strings for money. The IR blob is not re-encoded at all — it is the bytes
`AgentIr::to_canonical_json` produced.

**Deterministic order.** Layers are sorted by title. A `BTreeMap` iteration is
not an ordering anyone should have to trust; sorting where it is written is.

**One hash.** `sha256`, because that is what every registry accepts. A package
that needed a second algorithm would need a second identity, and there is no
question here that two hashes answer better than one.

## The lockfile

`ingot.lock` is the package's answer to "what was this built from, and what does
it expect?". It records **identity, not content**: digests and names, never file
bodies and never values.

```json
{
  "lockVersion": "1",
  "ingot": "0.3.0",
  "language": "0.2",
  "irVersion": "0.2",
  "project": { "name": "research-agent", "version": "0.1.0" },
  "sources": [{ "path": "main.ing", "digest": "sha256:…" }],
  "agents": [
    { "agent": "heptapus.examples.research.ResearchAgent", "digest": "sha256:…" }
  ],
  "toolServers": [
    {
      "name": "workspace",
      "command": "ingot-mcp-fs",
      "args": ["--root", "data", "--allow-write"],
      "passEnv": ["GITHUB_TOKEN"],
      "image": "ingot/mcp-fs:0.2"
    }
  ],
  "modelProviders": [
    { "apiKeyEnv": "OPENAI_API_KEY", "name": "openai" }
  ],
  "image": "ingot/run:0.3.0"
}
```

`passEnv` and `apiKeyEnv` are **variable names**. There is no field anywhere in
this document that can hold a value, which is the same rule the manifest already
enforces for `[[mcp.server]]` and for the same reason: a lockfile is committed.

The lockfile is not a resolver's output. Ingot has no dependency graph to solve;
what it has is a set of inputs whose identity decides whether a run elsewhere
means what a run here meant. `ingot package --verify` recomputes the source and
agent digests and reports each one that moved.

## Why source is not in the package

The obvious alternative is to ship the `.ing` alongside the IR, so an artifact
can be read by a human who received it. It was rejected for three reasons.

**The IR is the contract, and shipping both creates two.** Every backend consumes
Agent IR. A packaged source file would be a second representation that nothing
reads and nothing checks, which is precisely the failure
[RFC-0007](0007-the-ingot-product-loop.md) spends its length avoiding: a hidden
second source of truth that drifts.

**Source is the largest attack surface for the secrets rule.** A prompt is free
text. The scanner refuses the credential shapes it can recognise, and the honest
reading of that is that it catches what it catches. Not distributing the free
text at all is a stronger property than scanning it before distributing it.

**Identity is enough for the job it has to do.** The reason to want the source is
to know what produced the artifact, and `sources[].digest` answers that against a
repository that has the file. The reason not to is that the artifact is meant to
be the checked thing, moved.

Nothing stops a project shipping its source the ordinary way. This says only that
the package is not the vehicle.

## The build-time secret scan

The scan is over **values, not words** — the distinction
[GAP-004](../docs/gaps.md#gap-004) leaves open and the one that decides whether
operators route around the check. An agent may legitimately be about password
resets, API key rotation or token budgets. What it may not contain is a long
opaque run of characters after a credential marker:

* a vendor-prefixed key (`sk-…`, `ghp_…`, `github_pat_…`, `xoxb-…`) followed by
  sixteen or more opaque characters;
* `Bearer` followed by the same;
* `api_key`/`token`/`secret`/`password` followed by `=` or `:` and an opaque run.

It runs over **source, the compiled IR bytes and every cassette in the project**,
and it **refuses**: `ingot build` and `ingot package` stop, name the file, the
line and the shape, and never quote the value. Quoting it would copy the
credential into a terminal and a CI log, which is the failure the check exists to
prevent.

The same scanner already guards model-assisted authoring
([#9](https://github.com/mathissdupont/ingot/issues/9)). That is deliberate: a
generator must not be able to write what the packager would refuse, and two
implementations of one rule is one implementation too many.

**What it is not.** It is not entropy analysis, and it is not a guarantee. A
credential that looks like an English sentence passes. The commitment in
SECURITY.md is that Ingot provides no *path* for a secret to enter an artifact;
the scanner is a check on the human, not a replacement for that design.

## Automatic image acquisition stays closed

[GAP-026](../docs/gaps.md#gap-026) closed by making `ingot image build` build the
reference image locally, and deliberately left downloading open. This RFC does
the half it can do honestly.

**Implemented — digest pinning.** An image reference may be written
`ingot/run@sha256:…` in `[run] image` or `--image`. Before a contained run starts,
the local image's digest is compared with the pin. A mismatch **refuses**, naming
both. An unpinned reference behaves exactly as it does today.

**Defined, not implemented — acquisition.** A pull may only become automatic once
there is a signature over the manifest digest, a documented trust root, and a
refusal path for an unsigned or unverifiable image. Until all three exist, a
missing image stays an error with a build command attached, and never a silent
download or a host-run fallback.

The reason for the split is that digest pinning is a *complete* property on its
own — it tells you the bytes you have are the bytes you named — while signature
verification without a trust root is theatre. Shipping the first and naming the
second is better than shipping something that looks like both.

## Security and policy impact

This grants nothing. A package is inert: no effect, no capability, no policy
decision is expressed here that the IR did not already carry, and the runtime
enforces policy from the IR exactly as before.

Four boundaries the implementation must preserve:

* a package must never contain a credential value, a cassette, or a path from
  the build machine;
* a lockfile field must never be able to hold an environment *value*, only a
  name;
* a digest mismatch must refuse rather than warn;
* an unverified image must never be acquired automatically, and a missing image
  must never fall back to a host run.

## Static bounds

None. Packaging adds no agent execution construct, and reads a finite,
already-compiled set of documents.

## Compatibility

Additive. `ingot build` keeps writing exactly the files it writes today, in the
same bytes — the required test exists to prove that the packager did not quietly
become a second encoder. Existing Agent IR documents, manifests and commands
retain their meaning.

The one behaviour change is the secret scan, which can fail a build that
previously passed. That is the point of it, and the failure names the file and
the line.

`lockVersion` and the media-type versions move independently of the language, the
IR and the CLI, on the [GOVERNANCE.md](../GOVERNANCE.md) rules.

## Alternatives

**Push to a registry ourselves.** Rejected. A registry client is an HTTP
protocol, four authentication flows, token caching and a retry policy — a large
security surface bolted onto a compiler to save one `oras cp`. The layout is the
interoperable thing; the transport is somebody else's solved problem.

**Tar layers, like a runnable image.** Rejected above: tar adds four fields that
have to be normalised away and buys nothing an artifact needs.

**Ship the IR as a plain file with a `.sha256` beside it.** Simpler, and it
answers "which bytes" — but not "what was this built against", not "where does it
go", and not "how does a runtime find it". Reinventing a subset of OCI badly is
worse than using OCI.

**Put the lockfile in the repository instead of the package.** Both, in fact: it
is written to the project so it can be committed and reviewed, *and* carried in
the package so a received artifact is self-describing. A lockfile that only
existed in the package could not be diffed in a pull request.

**Include a timestamp, like most build tools.** Rejected. It is the single
easiest way to destroy reproducibility, and every build system that added one has
since added a way to turn it off.

## Conformance tests

- [x] `the_packaged_ir_is_the_same_bytes_that_the_tested_build_produced`
- [x] `a_package_digest_is_reproducible_across_runs_and_platforms`
- [x] `a_package_carries_no_credential_no_cassette_and_no_build_machine_path`
- [x] `a_secret_in_source_or_a_cassette_fails_the_build`
- [x] `a_lockfile_records_identity_and_never_an_environment_value`
- [x] `verify_reports_every_input_that_moved_since_the_package_was_written`
- [x] `a_digest_pinned_image_that_does_not_match_refuses_the_run`
