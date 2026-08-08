# RFC-0007: The Ingot product loop

- Status: **Accepted**
- Created: 2026-08-08
- Affects: CLI, authoring, distribution
- Coordinates: M6, M7, M10, M11 and the future Language 0.2 RFCs

## Problem

Ingot can compile, test, execute, contain and cross-compile an agent, but using
those capabilities still feels like operating five separate systems. A new user
has to discover the language, provider environment, MCP manifest, cassette
workflow, boundary image and build target independently. The machinery is
stronger than the path through it.

That means the project currently answers this question well:

> What is this agent permitted to do, and can the runtime enforce it?

It does not yet answer the question that earns regular use:

> How quickly can I turn a repeatable job into an agent I can understand, test,
> run safely and move elsewhere?

Security alone is not the product. It is the trust layer underneath a product
that makes agent development easier. Portability alone is not the product
either. It matters when an agent worth keeping is easy to create in the first
place.

There is a second risk in solving this badly. A natural-language generator or a
web application could hide the source and make `.ing` an incidental build
format. That would discard the language, compiler and diagnostics as the user
interface just when they have become useful. Ingot source must remain the
durable, reviewable source of truth.

## Product claim

**Write an agent in a small, readable language; Ingot helps you understand it,
test it, run it inside the boundary its own policy describes, and package the
same artifact for another environment.**

The intended loop is:

```text
create or write .ing
        ↓
develop with immediate diagnostics and traces
        ↓
replay a deterministic test
        ↓
run inside a policy-derived boundary
        ↓
package the same checked artifact
```

In command-line terms, the destination is one coherent path:

```bash
ingot init weekly-report --template document-workflow
cd weekly-report
ingot dev
ingot test
ingot run --contained --input document=@report.md
ingot package
```

The exact new command names remain subject to their implementation issues. The
sequence and the ownership boundaries in this RFC do not.

## Principles

1. **`.ing` remains the source of truth.** Templates, editor actions and model
   assistance produce ordinary Ingot source. There is no hidden workflow graph
   with generated source as an export format.
2. **The language is usable without a model.** Formatting, completion, traces,
   templates and tests cannot require an API key.
3. **Model assistance is optional and inspectable.** It proposes a source diff;
   it does not mutate a project invisibly.
4. **A model never grants itself reach.** A proposed policy change is shown
   separately and needs explicit operator acceptance. Repairing a compiler
   diagnostic cannot widen policy as a shortcut.
5. **Containers stay.** `ingot-sandbox` and `ingot-supervisor` become the
   implementation behind a simple safe-run experience. Users should benefit
   from the boundary without learning its plumbing.
6. **Local iteration stays fast.** Containment must not make every edit require
   an image build. The development loop says clearly which guarantees are being
   checked and which are being enforced.
7. **One artifact travels through the loop.** Testing, contained execution,
   Python emission and future OCI packaging consume the same Agent IR rather
   than compiling parallel product-specific representations.
8. **Broad use comes from horizontal primitives.** Ingot does not become a
   code-review product, research product or support product. Those are templates
   over the same language, tool and runtime contracts.

## The user journey

### 1. Start from source or a template

`ingot init` remains the deterministic, model-free entry point. It grows a small
set of maintained templates that demonstrate language patterns rather than
vendor products: document transformation, tool-using workflow, bounded research
and multi-agent coordination.

Every generated project must contain:

- readable `main.ing` source;
- a manifest with placeholders, never embedded credentials;
- example inputs;
- at least one offline test fixture where the workflow permits it;
- a README containing the next three commands that actually work.

### 2. Develop the language, not a configuration format

`ingot dev` is the integrated local loop. Its first useful version watches the
entry source, runs the existing compiler, keeps the last good artifact, and
prints a compact change-oriented status. Later versions add a run trace and an
interactive input set, but do not become a separate IDE protocol.

M7 supplies the editor-facing half: syntax highlighting, format-on-save,
diagnostics, completion, hover and navigation. Both surfaces use the compiler's
existing diagnostics; the editor must not implement a second type checker.

### 3. Understand a run

The event stream already contains the portable facts. The product loop turns it
into a useful trace:

- source node and execution order;
- rendered prompt and named context, with secrets excluded;
- provider/model selection;
- step and token consumption against the declared ceiling;
- tool and sub-agent boundaries;
- emitted artifacts and the node that produced them;
- the exact failure, attached to source where possible.

The JSON event stream remains the stable interface. A terminal renderer and a
future graphical viewer are consumers, not new runtime semantics.

### 4. Turn a run into a test

Recording and replay are promoted from specialist flags into the normal loop.
After a successful record, the CLI should be able to create or update a named
test fixture and print the command that replays it. Tool-result recording remains
GAP-006 and must be named when it prevents a self-contained test.

### 5. Run safely without operating the container layer

The container system is retained. The usability work removes accidental setup:

- `ingot doctor` reports provider, MCP and container readiness together;
- the reference run image has a reproducible, version-matched acquisition path;
- a contained run chooses that image when the manifest does not deliberately
  choose another;
- policy-derived mounts and network remain reviewable before execution;
- local and contained runs use the same inputs, events, outputs and failures;
- an unavailable boundary is never silently replaced by a host run.

The operator may still select an image because deployment is their concern. A
project should not require reading a repository Dockerfile merely to try the
reference experience.

### 6. Add model assistance without replacing the language

M10 adds an optional authoring assistant. `ingot new "review pull requests"`
or an equivalent command creates normal project files, then shows them. On an
existing project the assistant proposes a diff.

Its repair loop is bounded:

1. produce or edit Ingot source;
2. run `ingot check`;
3. return structured diagnostics to the authoring model;
4. retry up to an operator-visible ceiling;
5. stop with the source and diagnostics if it still does not compile.

Policy additions, credentials, tool installation and execution remain outside
that automatic repair loop. The assistant can request them and explain why; it
cannot approve them.

### 7. Package only what the loop has proved

M6 follows the usable authoring loop. `ingot package` carries the checked Agent
IR, metadata, lock information and reproducible digest. Packaging does not
capture a hidden editor state, model conversation or host credential. The
package is another destination for the same artifact already tested and run.

## Language growth

Making the language central also means removing the limits that force serious
programs to copy source. Language 0.2 is not folded into this CLI RFC, but its
RFCs should be prioritised by authoring evidence in this order:

1. modules/imports, closing GAP-011;
2. optional and union values needed by real tool schemas;
3. small pure user-defined functions for reusable transformations;
4. generics only after repeated source demonstrates a need.

This order keeps Ingot an agent language rather than growing a second Python by
default. Each language feature still needs its own syntax, IR, backend and
compatibility RFC.

## Proposed syntax

None in this RFC. The command lines above describe the intended product path,
not a frozen CLI grammar, and no `.ing` syntax changes here. Language 0.2 work
follows the existing RFC process one feature at a time.

## IR semantics

None. The integrated loop consumes the existing canonical Agent IR. If a useful
trace cannot map a node back to source with the information IR 0.1 carries, that
is recorded and specified separately rather than adding an implementation-only
side channel.

## Target lowering

Unchanged. The reference interpreter, generated Python and future backends keep
their current portability reports. Authoring features may inspect a report but
cannot hide, rewrite or reinterpret an unsupported construct for a target.

## Work packages

These are deliberately issue-sized boundaries. Detailed issues may split them,
but should retain the acceptance criteria and ordering.

### P1. The golden first-use path (M11)

Tracking issue: [#1](https://github.com/mathissdupont/ingot/issues/1).

- Extend `ingot init` with at least two maintained templates.
- Make every generated README executable as written in CI.
- Add an end-to-end test from empty directory to offline replay.
- Document a ten-minute first-agent path without requiring language knowledge
  beyond reading the generated source.

**Done when:** a clean machine with an Ingot binary can create, check, build and
replay a useful agent without editing generated files or obtaining an API key.

### P2. Project readiness (`ingot doctor`) (M11)

Tracking issue: [#2](https://github.com/mathissdupont/ingot/issues/2).

- Check provider selection without printing secret values.
- Resolve declared tools to MCP servers and name missing routes.
- Detect the container runtime and version-matched reference image.
- Offer machine-readable output for editor and CI consumers.

**Done when:** every prerequisite failure currently discovered during `run` is
available before the run in one report with an actionable fix.

### P3. Integrated development loop (`ingot dev`) (M11)

Tracking issue: [#3](https://github.com/mathissdupont/ingot/issues/3).

- Watch source and manifest inputs without busy polling.
- Reuse `check`, canonical build and diagnostic rendering.
- Keep the last successful artifact and never run a failed build.
- Add an opt-in example-input run and compact event trace.

**Done when:** editing a prompt, type or policy produces one immediate result
without manually alternating between `check`, `build` and `run`.

### P4. Human-readable trace (M11)

Tracking issue: [#4](https://github.com/mathissdupont/ingot/issues/4).

- Render existing events without changing their portable JSON form.
- Map node ids back to source spans where the artifact provides enough data;
  record a separate gap/RFC if it does not.
- Redact credentials and keep cassette-sensitive content explicit.
- Show budget progress and artifact provenance.

**Done when:** a failed multi-node run can be diagnosed without reading raw JSON
or adding logging to a backend.

### P5. Contained-run readiness (M11, then M6)

Tracking issue: [#5](https://github.com/mathissdupont/ingot/issues/5).

- Define how a version-matched reference image is built or acquired.
- Make the default path reproducible and verify its identity.
- Preserve explicit custom images and all current refusal behaviour.
- Test the documented one-command path against a real container runtime in CI.

**Done when:** a user with a supported container runtime can execute the
reference contained run without manually cloning Dockerfile commands, and a
machine without one receives a precise refusal rather than a weaker run.

### P6. Editor foundation (M7)

Tracking issue: [#6](https://github.com/mathissdupont/ingot/issues/6).

- Publish a grammar for `.ing` highlighting.
- Add an LSP that delegates parsing and analysis to existing crates.
- Support diagnostics, formatting, completion, hover and definition navigation.
- Ship one reference editor extension without making the protocol editor-specific.

**Done when:** the reference examples can be authored without a separate
terminal check loop and every displayed diagnostic matches the CLI.

### P7. Reuse in the language (Language 0.2 RFCs)

Tracking issue: [#7](https://github.com/mathissdupont/ingot/issues/7).

- Specify modules/imports first.
- Use real MCP schemas and examples to specify optional/union values.
- Specify pure helper functions separately from agent calls.
- Require lowering and both-backend portability reports for every addition.

**Done when:** shared types, tool declarations and pure transformations no
longer have to be copied between agent source files.

### P8. Tool onboarding (M11)

Tracking issue: [#8](https://github.com/mathissdupont/ingot/issues/8).

- Turn `ingot tools` into a preflight path for authoring as well as execution.
- Generate manifest stanzas from discovered MCP schemas where unambiguous.
- Keep installation commands and credentials operator-controlled.
- Provide typed source snippets for tools the project can actually route.

**Done when:** adding an existing MCP tool does not require manually duplicating
its name, schema and server route across unrelated files.

### P9. Model-assisted source authoring (M10)

Tracking issue: [#9](https://github.com/mathissdupont/ingot/issues/9).

- Generate ordinary projects and source diffs.
- Feed structured compiler diagnostics into a bounded repair loop.
- Separate policy proposals from ordinary source repairs.
- Generate example inputs and tests when the selected tools can be replayed.

**Done when:** a person unfamiliar with Ingot can describe a horizontal workflow,
receive compiling `.ing`, understand the requested reach, and continue editing
the result without the authoring model.

### P10. Packaging and sharing (M6)

Tracking issue: [#10](https://github.com/mathissdupont/ingot/issues/10).

- Package only canonical, checked Agent IR and declared metadata.
- Add the lockfile, digest and build-time secret scan already assigned to M6.
- Carry target portability reports alongside, not as a claim of universal support.
- Connect packages to the contained-run image contract without embedding secrets.

**Done when:** the artifact tested locally is the artifact identified, moved and
executed elsewhere.

## Delivery order

Milestone numbers are stable identifiers, not priority. The proposed order is:

```text
M11 product loop foundation
  → M7 editor foundation
  → Language 0.2 reuse RFCs
  → M10 optional model assistance
  → M6 packaging and distribution
  → M8 third-party backend conformance
```

P1–P5 establish the path before broadening the language or adding a generator.
P6 and P7 make direct `.ing` authoring pleasant and reusable. P9 accelerates
that workflow without replacing it. P10 packages something people have already
created, tested and run.

## Security and policy impact

This RFC grants nothing new. Its implementation must preserve four boundaries:

- a host run cannot be described as contained;
- a missing reference image cannot fall back to an uncontained run;
- templates and model assistance cannot write credential values;
- accepting ordinary generated source cannot implicitly accept a policy change.

Automating image acquisition adds a supply-chain decision. M6 must define digest
and signature verification before a downloaded image can become the default.
Until then, the product loop may build the reference image locally or require an
explicitly selected image, but must make that one actionable step.

## Static bounds

The product loop adds no agent execution construct. Authoring repair attempts,
file watching, trace retention and readiness probes each need their own finite
operator-visible bounds. `ingot dev` must not start a new run while the previous
one is live unless concurrency is explicitly designed.

## Compatibility

The direction is additive. Existing `.ing`, Agent IR 0.1 documents and commands
retain their meaning. New language features are excluded until their own RFCs.
The current `--contained` spelling remains valid even if a later CLI RFC adds a
friendlier alias.

## Non-goals

- A hosted no-code workflow editor as the source of truth.
- A vertical product for one kind of agent.
- Hiding Ingot source from people who began with a natural-language description.
- Replacing MCP, OCI or the existing backend interface.
- Making containers mandatory for formatting, compilation or fast local tests.
- Expanding Language 0.2 to general-purpose programming without demonstrated
  agent-authoring needs.

## Success measures

- A first useful project reaches offline replay in under ten minutes.
- The documented first-use path is an end-to-end CI test, not a screenshot.
- A user can diagnose missing provider, tool and container prerequisites before
  execution.
- Direct `.ing` authors receive the same diagnostics in the editor and CLI.
- Every model-assisted result is ordinary source and remains usable without the
  authoring model.
- A contained run requires no manual reconstruction of repository-specific
  Docker commands.
- Testing, contained execution, Python emission and packaging consume the same
  canonical Agent IR.

## Alternatives

**Lead with the container system.** It differentiates Ingot, but asks users to
care about implementation before they have an agent worth containing. The
boundary remains; it moves behind the safe-run step in the product loop.

**Lead with natural-language generation.** Fast to demonstrate and easy to make
indistinguishable from every framework wrapper. It also makes `.ing` incidental.
Model assistance follows the deterministic authoring path and produces source.

**Package first.** M6 is technically ready to be next, but distribution does not
remove the friction of creating and understanding what is distributed.

**Build a web studio first.** A UI could consume the diagnostics and event stream
later. Building it before those interfaces support the product loop would encode
missing semantics in the UI and create a second source of truth.

**Grow the language first.** Some growth is necessary, especially modules.
Starting with the loop shows which additions remove real repetition and prevents
turning a deliberately small agent language into a general-purpose one by guess.

## Conformance and acceptance tests

- [x] `a_template_project_checks_builds_and_replays_without_a_key`
- [x] `doctor_names_every_missing_run_prerequisite_without_revealing_a_secret`
- [ ] `dev_never_runs_a_source_revision_that_failed_to_compile`
- [ ] `the_human_trace_preserves_the_json_event_order`
- [ ] `a_reference_contained_run_needs_no_repository_specific_build_command`
- [ ] `a_missing_boundary_never_falls_back_to_a_host_run`
- [ ] `editor_and_cli_diagnostics_are_identical`
- [ ] `an_authoring_repair_cannot_accept_its_own_policy_widening`
- [ ] `model_assistance_leaves_a_project_that_works_without_the_model`
- [ ] `the_packaged_ir_is_the_same_bytes_that_the_tested_build_produced`
