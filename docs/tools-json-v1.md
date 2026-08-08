# `ingot tools --json` contract, version 1

`ingot tools --json` writes exactly one JSON document to standard output after
compiling the selected project and performing live MCP discovery. It starts only
the server commands already present in `ingot.toml`; it never installs a server,
changes a manifest, or reads credential values for the report.

The top-level shape is:

```json
{
  "schemaVersion": 1,
  "ready": true,
  "requiredEnvironment": ["SEARCH_API_KEY"],
  "servers": [],
  "declaredTools": [],
  "diagnostics": []
}
```

Fields are emitted in deterministic order. Arrays whose values are sets, such
as `requiredEnvironment` and `declaredTools`, are lexically sorted. Consumers
must use `schemaVersion` before interpreting fields and ignore new fields added
within version 1.

## Readiness and exit codes

- `ready: true` and exit code `0` mean every declared MCP tool has a live route
  and no blocking input-schema drift was found.
- `ready: false` and exit code `1` mean a route is missing, discovery failed, or
  at least one source declaration conflicts with the live schema.
- A compatibility status of `unverified` is non-blocking. It means the server
  used a schema construct Language 0.1 cannot prove, not that the declarations
  match.
- CLI usage, project resolution and compilation failures retain exit code `2`
  or the compiler diagnostic exit and may occur before a JSON report exists.

## Servers

Each `servers` entry contains:

- `manifestName`: the `[[mcp.server]]` name used for routing.
- `server`: the MCP handshake identity and protocol version.
- `requiredEnvironment`: names from that server's `pass-env`; values are never
  included.
- `tools`: the descriptors returned by `tools/list`, including `inputSchema`
  and `outputSchema` when the server publishes one.

The top-level `requiredEnvironment` is the sorted union of the per-server names.
Presence in this array does not reveal whether a value exists in the process
environment.

## Declared tools

Each `declaredTools` entry contains the checked source `signature`, its resolved
`route` when one exists, and `schemaCompatibility`:

```json
{
  "name": "search.query",
  "signature": {
    "params": [{"name": "query", "type": "string"}],
    "result": "search_result[]"
  },
  "route": {
    "server": "search",
    "remote": "query",
    "aliased": true
  },
  "schemaCompatibility": {
    "status": "match",
    "issues": []
  }
}
```

The four statuses are:

- `match`: the input shape inspected by version 1 is compatible.
- `drift`: at least one blocking mismatch exists.
- `unverified`: the route exists, but part of its schema cannot be proven.
- `unavailable`: no live route exists.

Every issue has a stable `code`, an `error` or `warning` severity, and a human
message. Version 1 checks top-level object schemas, required parameter names,
whether additional parameters are rejected, primitive types, lists, and the
object shape of named Ingot records. It exposes output schemas for consumers but
does not yet compare result types or recursively compare record fields.

## Top-level diagnostics

`diagnostics` contains failures that apply to discovery as a whole rather than
one declaration, currently `MCP_NO_SERVERS` and `MCP_DISCOVERY_FAILED`. Tool-
specific route and schema issues remain beside the affected declaration so an
editor does not have to reconstruct that relationship from prose.
