---
name: obz-vt
description: >
  VictoriaTraces provider for obz. Covers trace search, trace retrieval by ID,
  and extension commands for service and operation discovery via the
  Jaeger-compatible HTTP API. This skill should be used when the user mentions
  "VictoriaTraces", "obz trace -p vt", or needs to search distributed traces
  from a VT backend.
---

# obz-vt: VictoriaTraces Provider

## Quick Reference

| Field            | Value                                          |
|------------------|-------------------------------------------------|
| Aliases          | `vt`, `victoriatraces`                          |
| Signal           | Trace                                           |
| Query language   | None (query is a service name)                  |
| Auth             | Bearer token, Basic auth, or none               |
| Provider flags   | None                                            |
| Supported cmds   | search, get, services, operations               |

## Supported Commands

### Core trace commands

```
obz trace search      # Search traces by service name
obz trace get         # Retrieve a single trace by ID
```

### Extension commands

```
obz trace services    # List all known service names
obz trace operations  # List operations for a given service (requires --service)
```

## Authentication

Configure auth in `config.yaml` under `providers.<name>.auth`. Supports
bearer token, basic auth, or no auth.

```yaml
# config.yaml
providers:
  vt:
    endpoint: http://localhost:10428
    auth:
      token: ${env:VT_TOKEN}
```

For basic auth:
```yaml
    auth:
      username: user
      password: ${env:VT_PASS}
```

Then query with just `-p`:
```bash
obz trace search -p vt -q 'frontend' --from now-1h
```

## Query Format

The `-q` value is a plain service name. There's no structured query language.

```bash
obz trace search -p vt -q 'frontend' --from now-1h
```

## Examples

**Search traces for a service**:
```bash
obz trace search -p vt -q 'frontend' --from now-1h
```

**Retrieve a specific trace by ID**:
```bash
obz trace get -p vt abc123def456
```

**List all services**:
```bash
obz trace services -p vt
```

**List operations for a service**:
```bash
obz trace operations -p vt --service frontend
```

## Tips

- Use `trace services` to discover what's instrumented before searching.
- Time ranges accept relative syntax: `now-30m`, `now-6h`, `now-1d`.
- VictoriaTraces speaks the Jaeger HTTP Query API, so any Jaeger-compatible
  tooling will work with the same endpoint.

