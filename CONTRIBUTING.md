# Contributing to Ingot

Thank you for considering a contribution. This document explains how work gets
proposed, reviewed and merged.

## Spec-first

Ingot is a compiler. Its output is a contract other people build against, so a
change to observable behaviour is a change to that contract.

**Behaviour changes go through an RFC.** That means anything touching:

* the language: syntax, types, effects, policy, budgets
* the Agent IR: node kinds, values, the canonical encoding
* the artifact format or the lockfile
* diagnostic codes, once they exist

Everything else — bug fixes, diagnostics wording, performance, refactoring,
tests, documentation — can start as an issue and a pull request.

The flow:

1. **Issue.** Describe the user problem with a concrete example, not a proposed
   API.
2. **RFC.** Copy [`rfcs/0000-template.md`](rfcs/0000-template.md) to
   `rfcs/NNNN-title.md`. It must state the proposed syntax, the IR semantics,
   how each backend lowers it, the security impact, the compatibility story and
   the conformance tests it needs.
3. **Review.** At least two maintainers, covering the language, the runtime and
   the security perspective.
4. **Prototype.** Fine before acceptance, behind a feature flag or on a branch.
5. **Accept.** Once accepted, split into implementation issues.
6. **Conformance.** A feature is not complete until a normative test covers it.
7. **Release.** Note the language, IR and CLI version implications explicitly.

## Getting set up

```bash
cargo build
cargo test
```

On Windows, if the repository lives inside OneDrive or another synced folder,
move the build directory out of it:

```bash
export CARGO_TARGET_DIR=/c/build/ingot
```

## Before opening a pull request

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
./target/debug/ingot fmt --check examples/research-agent
```

Every one of these runs in CI, so running them locally only saves you a round
trip.

## Standards we hold to

**Every rule has a test.** A check without a test is a check that will be
deleted by accident. Name the test after the behaviour, not the function:
`an_absent_policy_rule_denies_the_effect`, not `test_policy_3`.

**Diagnostics are a product surface.** A new diagnostic needs a stable code, a
primary label pointing at the offending span, and a `help` that says what to
write instead. If it is a rule a user could reasonably not know, add an entry to
`ingot explain`.

**Codes are permanent.** Never reuse a diagnostic code for a different meaning.
Retiring a check retires its code.

**Default-deny stays default-deny.** A change that lets an agent reach something
without an explicit grant in its source needs security review, whatever else it
does.

**Determinism is a feature.** Anything reaching the IR must be sorted or
otherwise stably ordered. Iterating a `HashMap` into output is a bug even when
the tests happen to pass.

**Golden files are reviewed, not regenerated.** `INGOT_UPDATE_GOLDEN=1` exists
to produce a diff you then read. An unexplained change in `tests/golden-ir/`
means the compiled meaning of every existing agent moved.

## Commits and pull requests

Conventional Commits:

```
feat(parser): accept trailing commas in argument lists
fix(semantic): report denied capabilities on sub-agent calls
docs(spec): clarify parallel map result typing
```

A pull request should state the linked issue or RFC, what it changes, how it was
tested, and whether it affects compatibility. Small and reviewable beats
complete and enormous.

`main` is protected. Spec changes need two maintainer approvals; anything
touching effects, policy or the artifact format also needs the security owner.

## Where things live

| Directory | Contents |
|-----------|----------|
| `specs/` | normative behaviour; no implementation detail |
| `crates/` | the compiler, one responsibility per crate |
| `examples/` | reference agents, also used as test fixtures |
| `tests/golden-ir/` | checked-in IR the examples must reproduce |
| `rfcs/` | proposals that change observable behaviour |
| `docs/adr/` | records of decisions already made |

Crate boundaries are deliberate. `ingot-parser` knows nothing about targets;
`ingot-semantic` knows nothing about any particular runtime; `ingot-ir` knows
nothing about providers; `ingot-runtime` knows about a `ToolHost` trait and
nothing about MCP. A pull request that blurs one of those lines will be asked to
move code instead.

Two crates ship binaries: `ingot-cli` produces `ingot`, and `ingot-mcp` produces
`ingot-mcp-fs`. `cargo test -p ingot-cli` does not build the latter, so the
tests that need it build it themselves; `cargo test --workspace` builds
everything and is the normal thing to run.

## Reporting security issues

Do not open a public issue. See [SECURITY.md](SECURITY.md).

## Licence

Contributions are accepted under Apache-2.0, as stated in
[LICENSE](LICENSE). By submitting a contribution you confirm you have the right
to license it under those terms.
