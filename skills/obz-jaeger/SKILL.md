---
name: obz-jaeger
description: >
  Jaeger provider for obz. Covers trace search, trace retrieval by ID, and
  extension commands for service and operation discovery. This skill should be
  used when the user mentions "Jaeger", "obz trace -p jg", or needs to search
  distributed traces from a Jaeger backend.
---

# obz-jaeger: Jaeger Provider

## Quick Reference

| Field            | Value                                          |
|------------------|-------------------------------------------------|
| Aliases          | `jg`, `jaeger`                                  |
| Signal           | Trace                                           |
| Query language   | None (query is a service name)                  |
| Auth             | Bearer token or Basic auth                      |
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
  jg:
    endpoint: http://localhost:16686
    auth:
      token: ${env:JAEGER_TOKEN}
```

Then query with just `-p`:
```bash
obz trace search -p jg -q 'frontend' --from now-1h
```

## Query Format

The `-q` value is a plain service name. Jaeger doesn't have a structured
trace query language for searches.

```bash
obz trace search -p jg -q 'frontend' --from now-1h
```

## Examples

**Search traces for a service**:
```bash
obz trace search -p jg -q 'frontend' --from now-1h
```

**Retrieve a specific trace by ID**:
```bash
obz trace get -p jg abc123def456
```

**List all services**:
```bash
obz trace services -p jg
```

**List operations for a service**:
```bash
obz trace operations -p jg --service frontend
```

## Jaeger vs VictoriaTraces

Both providers speak the same Jaeger HTTP Query API and expose the same
set of commands. The key difference is the backend:

| Aspect             | Jaeger               | VictoriaTraces       |
|--------------------|----------------------|----------------------|
| Storage            | Configurable         | Built-in             |
| Default port       | 16686                | 10428                |
| API compatibility  | Native               | Jaeger-compatible    |

Choose `jg` when pointing at a standalone Jaeger deployment. Choose `vt`
for VictoriaTraces.

