## What this changes

<!-- One or two sentences. What behaviour is different afterwards? -->

Closes #

## Kind of change

- [ ] Bug fix
- [ ] New feature
- [ ] Language, IR or artifact-format change (needs an accepted RFC)
- [ ] Documentation
- [ ] Refactor, tests or tooling

## Compatibility

- [ ] No effect on existing source or existing IR documents
- [ ] Language version affected: <!-- 0.1 -> ? -->
- [ ] IR version affected: <!-- 0.1 -> ? -->
- [ ] Existing programs change meaning (explain the migration below)

## Security and capabilities

- [ ] Does not change what an agent can reach
- [ ] Adds or changes an effect, a policy subject or a policy decision
- [ ] Touches secret handling, approvals or sandboxing

<!-- If any of the last two are ticked, request review from the security owner. -->

## Tests

<!-- Name them. A new rule without a test is a rule that will be deleted by
     accident. -->

- [ ] `cargo test --workspace` passes
- [ ] New behaviour has a test named after the behaviour
- [ ] Golden IR unchanged, or the diff is explained below

<!-- If golden files changed, paste the summary and say why every affected
     agent's compiled meaning should move. -->

## Checks

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] `ingot fmt --check` passes on the examples
- [ ] Specification updated, if behaviour changed
- [ ] `ingot explain` entry added, for a diagnostic a user could not guess
