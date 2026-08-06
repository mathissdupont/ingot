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

## The network allowlist is not enforced by the transport

`policy` says `network allow ["arxiv.org", "github.com"]`. The runtime checks
that the `network` effect is granted before the call; it does **not** inspect
which hosts the server actually contacts, because it cannot see inside another
process. The allowlist is a statement of intent that a compliant backend, or the
server itself, is expected to honour.

This is a real limitation and worth knowing before relying on it. Confining a
tool server's reach is the server's job, the same way `--root` is for
`ingot-mcp-fs`.
