# `ingot doctor` JSON schema v1

`ingot doctor --json` writes exactly one JSON document to standard output and
uses exit code `1` when any check has status `fail`. The command is read-only: it
does not start providers, MCP servers or containers, install software, pull
images, or read credential values.

The top-level shape is:

```json
{
  "schemaVersion": 1,
  "ready": false,
  "source": "project/main.ing",
  "manifest": "project/ingot.toml",
  "checks": [
    {
      "id": "provider.default",
      "status": "fail",
      "summary": "the program makes model calls, but no provider is ready",
      "location": "project/ingot.toml",
      "fix": "export ANTHROPIC_API_KEY, OPENAI_API_KEY or GEMINI_API_KEY, or declare a reachable `[[model.provider]]`"
    }
  ]
}
```

## Fields

| Field | Type | Contract |
|-------|------|----------|
| `schemaVersion` | integer | `1` for this shape |
| `ready` | boolean | `true` exactly when no check has status `fail` |
| `source` | string | resolved source path inspected by the command |
| `manifest` | string or `null` | resolved manifest path, or `null` for a loose `.ing` file |
| `checks` | array | ordered readiness facts |
| `checks[].id` | string | stable, machine-readable check identifier |
| `checks[].status` | string | `pass`, `warn` or `fail` |
| `checks[].summary` | string | human-readable result; wording may improve within schema v1 |
| `checks[].location` | string | source, manifest, environment-variable name or executable location |
| `checks[].fix` | string, optional | actionable next step when one is useful |

Consumers should make decisions from `schemaVersion`, `ready`, `id` and
`status`, not parse `summary` or `fix`. New check identifiers may be added within
schema v1. Existing identifiers do not change meaning within v1, and new
optional fields may be added. Removing a field, changing a field type, changing
the meaning of a status, or renaming an existing identifier requires a new
schema version.

An environment location and summary contain only the variable name, for example
`environment:OPENAI_API_KEY`. A credential value is never part of the report.

## Check identifier families

- `source.*` — parsing, type checking, policy checking and lowering.
- `provider.*` — manifest validation, provider construction prerequisites and
  model routing.
- `tools.*` — MCP manifest validation, server process prerequisites and static
  routes. A `warn` route needs live publication verification with `ingot tools`.
- `container.*` — Docker/Podman availability, the selected custom-or-reference
  image, version mismatch, and local image presence. The report may recommend
  `ingot image build`, but remains read-only and never builds or pulls itself.

The human-readable command reports the same checks and uses the same exit-code
contract.
