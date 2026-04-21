---
name: obz-vl
description: >
  VictoriaLogs provider for obz. Supports log search using LogsQL, a purpose-built
  query language with _msg, _time, and _stream filters plus pipes for transformation.
  This skill should be used when the user mentions "VictoriaLogs", "LogsQL",
  "obz log search -p vl", or needs to search logs from a VL backend.
---

# obz-vl: VictoriaLogs Provider

## Quick Reference

| Field            | Value                                              |
|------------------|----------------------------------------------------|
| Aliases          | `vl`, `victorialogs`                               |
| Signal           | Log                                                |
| Query language   | LogsQL                                             |
| Auth             | Bearer token, Basic auth, or none                  |
| Provider flags   | `--account-id`, `--project-id`                     |
| Supported cmds   | log search, field-names, field-values, hits, stats |

## Supported Commands

### Core log commands

```
obz log search         # Search log entries using LogsQL
```

### Extension commands

```
obz log field-names    # List available field names (optional --query filter)
obz log field-values   # List values for a field (requires --field)
obz log hits           # Show log volume over time (optional --step)
obz log stats          # Run stats aggregation (requires --query)
```

## Authentication

Configure auth in `config.yaml` under `providers.<name>.auth`. Supports
bearer token, basic auth, or no auth.

```yaml
# config.yaml
providers:
  vl:
    endpoint: http://localhost:9428
    auth:
      token: ${env:VL_TOKEN}
```

Then query with just `-p`:
```bash
obz log search -p vl -q 'error' --from now-1h
```

## Provider-Specific Flags

| Flag             | Type   | Required | Description                        |
|------------------|--------|----------|------------------------------------|
| `--account-id`   | String | No       | Multi-tenant AccountID             |
| `--project-id`   | String | No       | Multi-tenant ProjectID             |

These flags are only needed when querying a multi-tenant VictoriaLogs cluster.

```bash
obz log search -p vl --account-id 12 --project-id 34 \
  -q 'error' --from now-1h
```

## Config File Setup

```yaml
# ~/.config/obz/config.yaml
providers:
  prod-vl:
    provider: vl
    endpoint: https://vl.prod.example.com
    account-id: "12"
    project-id: "34"
    auth:
      token: ${env:VL_TOKEN}
```

Then:
```bash
obz log search -p prod-vl -q 'error' --from now-1h
```

## LogsQL Guide

LogsQL queries are built from filters combined with AND/OR/NOT operators
and optional pipes for post-processing.

### Built-in Fields

- `_msg` contains the log message body
- `_time` holds the timestamp
- `_stream` identifies the log stream (typically `{key="value"}` pairs)

### Filter Types

**Word filter** (matches exact word in `_msg`):
```
error
```

**Phrase filter** (matches exact phrase):
```
"connection refused"
```

**Prefix filter** (matches word prefix):
```
err*
```

**Regex filter**:
```
_msg:re("error|warn")
```

**Field filter** (match on a specific field):
```
level:error
host:web-01
```

**Stream filter** (filter by log stream labels):
```
_stream:{job="api", instance="web-01"}
```

**Time filter** (usually set via `--from`/`--to` flags instead):
```
_time:[2024-01-01, 2024-01-02)
```

### Combining Filters

**AND** (implicit when space-separated):
```
error _stream:{job="api"}
```

**Explicit AND/OR/NOT**:
```
(error OR warn) AND _stream:{job="api"} NOT "health check"
```

### Pipes

Pipes transform results after filtering:

```
error | stats by (host) count() as errors
error | sort by (_time) desc | limit 50
error | fields _time, _msg, host
error | uniq by (host)
```

### Common Pitfalls

- Bare words match against `_msg` by default. To match a specific field,
  use `field:value` syntax.
- Stream filters use curly braces: `_stream:{job="api"}`. Don't forget
  the `_stream:` prefix.
- Regex filters require the `re()` wrapper: `_msg:re("pattern")`.
- Pipes use `|` as separator. The filter portion comes first, pipes after.

## Examples

**Simple word search**:
```bash
obz log search -p vl -q 'error' --from now-1h
```

**Stream-scoped search**:
```bash
obz log search -p vl -q 'error AND _stream:{job="api"}' --from now-1h
```

**Regex filter on message**:
```bash
obz log search -p vl -q '_msg:re("timeout|refused")' --from now-6h
```

**Field filter with pipe**:
```bash
obz log search -p vl \
  -q 'level:error | stats by (host) count() as errors' --from now-1d
```

**Multi-tenant query**:
```bash
obz log search -p vl --account-id 12 --project-id 34 \
  -q '"connection reset"' --from now-30m
```

**List field names**:
```bash
obz log field-names -p vl --from now-1h
```

**List values for a field**:
```bash
obz log field-values -p vl --field level --from now-1h
```

**Log volume histogram**:
```bash
obz log hits -p vl -q 'error' --from now-1h --step 5m
```

**Stats aggregation**:
```bash
obz log stats -p vl -q 'error | stats by (service) count() as errors' --from now-1h
```

