# code-review-team

A coordinator that fans a review out across files, merges the results, files a
report and posts it — exercising sub-agents with their own budgets and policies,
`parallel map`, a branch, and an approval gate the compiler inserts from
`external_write require approval`.

## What runs and what does not

```bash
ingot check          # clean
ingot build          # produces SecurityReviewer.ir.json and CodeReviewTeam.ir.json
ingot tools          # two of three tools resolve
```

`repo.read_file` and `repo.write_file` are served by `ingot-mcp-fs`, aliased in
[`ingot.toml`](ingot.toml) because the artifact and the server use different
names. `forge.comment` is not served by anything: posting a comment to a hosting
provider is an `external_write`, and this repository ships no server that can do
one.

So `ingot run` reaches the write and then stops at `forge.comment`, naming it.
That is the intended behaviour — a tool nothing serves ends the run rather than
being skipped, because the flow after it was type-checked against a value that
would not exist.

To run it fully, point a server at your hosting provider and map the name:

```toml
[[mcp.server]]
name = "forge"
command = "your-forge-mcp-server"
pass-env = ["GITHUB_TOKEN"]

[mcp.server.tools]
"forge.comment" = "create_pull_request_comment"
```

`pass-env` names the variable; the value is read from your environment at spawn
time and never written into the manifest.

## Two limits, and why one is not enough

The server is started with `--root ../..`, so it can touch anything in the
repository. What stops the agent doing so is its own policy —
`filesystem_read allow ["src", "crates"]` and
`filesystem_write allow ["target/review"]` — which the runtime re-checks before
every call.

That is the realistic configuration for a code review, and it is also the shape
of the mistake worth seeing: the two limits are independent, and a deployment
that relies on only one of them has half a control. Narrow the server as well as
the artifact.

## The approval gate

`external_write require approval` in the policy makes the compiler insert an
`approval` node before the `forge.comment` call. At run time an interactive
session is asked; an unattended one **denies by default**, because an artifact
that asked for a human does not get one silently. `--yes` approves without
asking, and is deliberately explicit.

## Sub-agent isolation

`SecurityReviewer` has its own `tools`, `budget` and `policy`. The coordinator
cannot widen any of them: a sub-agent call is a fresh run against the callee's
own artifact, not an inlined continuation of the caller's. `SecurityReviewer`
can read files; it cannot write one, and nothing the caller does changes that.
