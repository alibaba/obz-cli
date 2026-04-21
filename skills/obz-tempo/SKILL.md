---
name: obz-tempo
description: >
  Grafana Tempo provider for obz. Covers trace search, trace retrieval by ID,
  and extension commands for tag discovery. This skill should be used when the
  user mentions "Tempo", "Grafana Tempo", "TraceQL", "obz trace -p tempo", or
  needs to search traces from a Tempo backend.
---

# obz-tempo: Grafana Tempo Provider

## Quick Reference

| Field            | Value                                          |
|------------------|-------------------------------------------------|
| Aliases          | `tempo`                                         |
| Signal           | Trace                                           |
| Query language   | TraceQL                                         |
| Auth             | Bearer token or Basic auth                      |
| Provider flags   | None                                            |
| Supported cmds   | search, get, tags, tag-values                   |

## Supported Commands

### Core trace commands

```
obz trace search      # Search traces by service name or TraceQL
obz trace get         # Retrieve a single trace by ID
```

### Extension commands

```
obz trace tags        # List available tag names grouped by scope
obz trace tag-values  # List values for a specific tag (requires --tag)
```

## Authentication

Configure auth in `config.yaml` under `providers.<name>.auth`. Supports
bearer token, basic auth, or no auth.

```yaml
# config.yaml
providers:
  tempo:
    endpoint: http://localhost:3200
    auth:
      token: ${env:TEMPO_TOKEN}
```

For basic auth (common with Grafana Cloud):
```yaml
    auth:
      username: user
      password: ${env:TEMPO_PASS}
```

Then query with just `-p`:
```bash
obz trace search -p tempo -q '{ resource.service.name = "frontend" }' --from now-1h
```

## Query Format

The `-q` value must be a TraceQL expression.

```bash
obz trace search -p tempo -q '{ resource.service.name = "frontend" }' --from now-1h
```

## TraceQL Basics

TraceQL is Tempo's native query language for filtering spans.

### Span Selectors

```
{ span.http.status_code = 500 }
{ resource.service.name = "api" && span.http.method = "POST" }
{ duration > 2s }
{ status = error }
```

### Common Attributes

`resource.service.name`, `span.http.method`, `span.http.status_code`,
`span.http.url`, `duration`, `status` (ok/error).

## Examples

**Search traces for a service**:
```bash
obz trace search -p tempo -q '{ resource.service.name = "frontend" }' --from now-1h
```

**Search for slow spans**:
```bash
obz trace search -p tempo -q '{ duration > 5s }' --from now-6h
```

**Search for error spans in a service**:
```bash
obz trace search -p tempo \
  -q '{ resource.service.name = "api" && status = error }' --from now-1h
```

**Retrieve a specific trace by ID**:
```bash
obz trace get -p tempo abc123def456
```

**List available tag names**:
```bash
obz trace tags -p tempo
```

**List values for a tag**:
```bash
obz trace tag-values -p tempo --tag resource.service.name
```

