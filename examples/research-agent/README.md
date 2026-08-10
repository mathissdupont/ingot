# research-agent

A research workflow: generate diverse queries, search in parallel, draft a
source-grounded report, verify it cites enough distinct sources. Exercises MCP
tools, a network allowlist, working memory, `parallel map` fan-out, record field
access, verification and all three budget kinds.

## Running it needs a search server

```bash
ingot check          # clean
ingot build          # produces ResearchAgent.ir.json
ingot tools          # web.search: nothing serves it
```

`web.search` needs a web search MCP server. This repository ships only
`ingot-mcp-fs`, a filesystem server, so there is nothing here that can answer —
and rather than pretending, `ingot tools` exits non-zero and `ingot run` stops
at the call naming the tool.

Point it at a search server you trust:

```toml
[[mcp.server]]
name = "search"
command = "your-search-mcp-server"
pass-env = ["SEARCH_API_KEY"]

[mcp.server.tools]
"web.search" = "search"
```

Two things the server must satisfy, because the artifact declares them:

* `web.search(query: string) -> search_result[]`, where `search_result` has
  `title`, `url` and `snippet`, all strings. A result that does not match ends
  the run rather than being coerced.
* Since `search_result[]` is a list, the server returns it as
  `structuredContent: {"value": [...]}`, or as text that parses to a JSON array.
  See the [MCP binding](../../specs/tools/mcp-v0.1.md#7-results).

`pass-env` names the variable; the value is read from your environment at spawn
time. A server started by Ingot inherits nothing else.

## The network allowlist, and when it is enforced

`policy` says `network allow ["arxiv.org", "github.com"]`. Under
`ingot run --sandbox` that list is kept: the tool server joins a container
network with no route out, and the only thing it can reach is a proxy that
refuses every host the policy does not name. Ignoring the proxy reaches nothing
at all, because the bound is the network rather than an environment variable
([GAP-001](../../docs/gaps.md#gap-001), closed).

Without `--sandbox` there is no boundary and no proxy. The runtime still checks
that the `network` effect is granted before the call, but nothing inspects which
hosts the server contacts — it is an ordinary process with the operator's
network. Run it contained, or treat the list as intent.

The `verify CitationCheck(...)` line has the same shape of caveat:
[GAP-002](../../docs/gaps.md#gap-002). It evaluates its arguments and reports a
pass, because IR 0.1 carries no way to run a verifier.
