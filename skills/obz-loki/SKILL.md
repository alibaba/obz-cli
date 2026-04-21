---
name: obz-loki
description: >
  Grafana Loki provider for obz. Covers log search using LogQL, a label-based
  query language for log aggregation. This skill should be used when the user
  mentions "Loki", "Grafana Loki", "LogQL", "obz log search -p loki", or needs
  to search logs from a Loki instance.
---

# obz-loki: Grafana Loki Provider

## Quick Reference

| Field            | Value                                              |
|------------------|----------------------------------------------------|
| Aliases          | `loki`                                             |
| Signal           | Log                                                |
| Query language   | LogQL                                              |
| Auth             | Bearer token or Basic auth                         |
| Provider flags   | None                                               |
| Supported cmds   | log search, labels, label-values, fields, stats    |

## Supported Commands

### Core log commands

```
obz log search         # Search log entries using LogQL
```

### Extension commands

```
obz log labels         # List label names (no flags)
obz log label-values   # List values for a label (requires --label)
obz log fields         # List detected fields (optional --query)
obz log stats          # Run instant aggregation (requires --query with LogQL metric query)
```

## Authentication

Configure auth in `config.yaml` under `providers.<name>.auth`. Supports
bearer token, basic auth, or no auth.

```yaml
# config.yaml
providers:
  loki:
    endpoint: http://localhost:3100
    auth:
      token: ${env:LOKI_TOKEN}
```

For basic auth (common with Grafana Cloud):
```yaml
    auth:
      username: user
      password: ${env:LOKI_PASS}
```

Then query with just `-p`:
```bash
obz log search -p loki -q '{job="api"} |= "error"' --from now-1h
```

## LogQL Guide

LogQL is Loki's query language. Every query starts with a log stream
selector (labels in curly braces), optionally followed by filter and
parser stages.

### Stream Selectors

```
{job="api"}                          # exact match
{job="api", env="prod"}              # multiple labels
{job=~"api|web"}                     # regex match
{job!="debug"}                       # not equal
```

### Line Filters

```
{job="api"} |= "error"              # line contains "error"
{job="api"} != "health"             # line does NOT contain "health"
{job="api"} |~ "status=[45]\\d{2}"  # line matches regex
{job="api"} !~ "debug|trace"        # line does NOT match regex
```

### Parser Stages

```
{job="api"} | json                   # parse JSON fields
{job="api"} | logfmt                 # parse logfmt fields
{job="api"} | json | level="error"   # parse then filter by field
```

### Chaining Filters

Filters chain left to right. Each stage narrows the results further:
```
{job="api"} |= "error" != "timeout" | json | status >= 500
```

## Examples

**Basic log search**:
```bash
obz log search -p loki -q '{job="api"} |= "error"' --from now-1h
```

**Search with regex filter**:
```bash
obz log search -p loki \
  -q '{job="api"} |~ "status=(500|502|503)"' --from now-6h
```

**Parse JSON logs and filter by field**:
```bash
obz log search -p loki \
  -q '{job="api"} | json | level="error"' --from now-30m
```

**Multi-label selector**:
```bash
obz log search -p loki \
  -q '{job="api", env="prod", cluster="us-east"}' --from now-1h
```

**List label names**:
```bash
obz log labels -p loki
```

**List values for a label**:
```bash
obz log label-values -p loki --label job
```

**List detected fields**:
```bash
obz log fields -p loki -q '{job="api"}' --from now-1h
```

**Stats aggregation (instant query)**:
```bash
obz log stats -p loki -q 'count_over_time({job="api"}[5m])' --from now-1h
```

## Tips

- Labels are indexed. Put your most selective label first for better
  performance.
- Line filters (`|=`, `!=`) are fast. Use them before parser stages to
  reduce the volume of data Loki has to parse.
- Quote your LogQL with single quotes on the CLI to avoid shell
  interpretation of curly braces and pipes.

