---
name: obz-dd
description: >
  Datadog provider for obz. Supports metrics (Datadog Query Language), logs
  (Datadog Log Query), and traces. Requires api-key and app-key configured
  under auth in config.yaml. Does not support metric labels, label-values, or series
  commands. This skill should be used when the user mentions "Datadog", "DD",
  "obz -p dd", Datadog Query Language, or needs to configure Datadog provider
  instances with regional endpoints.
---

# obz-dd: Datadog Provider

## Quick Reference

| Field            | Value                                              |
|------------------|----------------------------------------------------|
| Aliases          | `dd`, `datadog`                                    |
| Signals          | Metric, Log, Trace                                 |
| Metric query     | Datadog Query Language                             |
| Log query        | Datadog Log Query                                  |
| Auth             | API key + Application key (both required)          |
| Default endpoint | `https://api.datadoghq.com`                        |

## Supported Commands

| Signal  | Commands                            | Not Supported                        |
|---------|-------------------------------------|--------------------------------------|
| Metric  | query, list, info                   | labels, label-values, series         |
| Log     | search, aggregate                   |                                      |
| Trace   | search, get, aggregate              |                                      |

Datadog's API doesn't expose label/series enumeration endpoints, so those
three metric commands are unavailable.

### Extension commands

```
obz log aggregate    # Aggregate logs (requires --query, optional --compute, --group-by, both repeatable)
obz trace aggregate  # Aggregate traces (same flags as log aggregate)
```

## Authentication

Datadog requires two keys for every request. Configure them in `config.yaml`
under `providers.<name>.auth`:

```yaml
# config.yaml
providers:
  dd:
    endpoint: https://api.datadoghq.com
    auth:
      api-key: ${env:DD_API_KEY}
      app-key: ${env:DD_APP_KEY}
```

Then query with just `-p`:
```bash
obz metric query -p dd -q 'avg:system.cpu.user{*}' --from now-1h
```

## Regional Endpoints

Datadog operates multiple regional sites. Set `--endpoint` to the correct one:

| Region        | Endpoint                              |
|---------------|---------------------------------------|
| US1 (default) | `https://api.datadoghq.com`           |
| US3           | `https://api.us3.datadoghq.com`       |
| US5           | `https://api.us5.datadoghq.com`       |
| EU            | `https://api.datadoghq.eu`            |
| AP1           | `https://api.ap1.datadoghq.com`       |

If `--endpoint` is omitted, the default US1 endpoint is used.

## Config File Setup

```yaml
# ~/.config/obz/config.yaml
providers:
  dd-prod:
    provider: dd
    endpoint: https://api.datadoghq.com
    auth:
      api-key: ${env:DD_API_KEY}
      app-key: ${env:DD_APP_KEY}
```

Then:
```bash
obz metric query -p dd-prod -q 'avg:system.cpu.user{*}' --from now-1h
```

## Datadog Query Language (Metrics)

Datadog metric queries follow the pattern:
```
<aggregation>:<metric_name>{<filter>}
```

### Aggregation Functions

`avg`, `sum`, `min`, `max`, `count`

### Tag Filters

Tags go inside curly braces, comma-separated:
```
avg:system.cpu.user{host:web-01}
sum:http.requests{service:api, env:prod}
avg:system.mem.used{*}
```

The wildcard `{*}` matches all hosts/tags.

### Arithmetic and Functions

```
avg:system.cpu.user{*} + avg:system.cpu.system{*}
avg:system.cpu.user{*}.rollup(avg, 300)
per_second(sum:http.requests{*})
```

### Space Aggregation (by/as)

```
avg:system.cpu.user{*} by {host}
sum:http.requests{*} by {service, env}
```

### Common Pitfalls

- The aggregation prefix (`avg:`, `sum:`, etc.) is required. Bare metric
  names like `system.cpu.user{*}` will fail.
- Tag values don't use quotes: `{host:web-01}` not `{host:"web-01"}`.
- Datadog queries are NOT PromQL. Don't use `rate()` or `[5m]` syntax.
- `.rollup()` controls time aggregation. Without it, Datadog picks
  automatic rollup intervals.

## Datadog Log Query (Logs)

Datadog log queries filter on facets and tags.

### Basic Syntax

```
service:web
status:error
service:web status:error
```

Multiple terms are AND-ed by default.

### Field Queries

```
@http.status_code:500
@duration:>1000
host:web-01
```

Prefixed `@` fields are log attributes. Unprefixed fields are reserved
tags (`service`, `status`, `host`, `source`).

### Boolean and Grouping

```
service:web AND (status:error OR status:warn)
service:web NOT @http.url:"/health"
```

### Wildcards

```
service:web-*
@http.url:"/api/*"
```

### Common Pitfalls

- Reserved tags (`service`, `status`, `host`, `source`) don't take the
  `@` prefix. Custom attributes do.
- String values with special characters need quotes:
  `@http.url:"/api/v1/users"`.
- The default combiner is AND. Explicit `OR` requires parentheses for
  correct grouping.

## Examples

**Metric query**:
```bash
obz metric query -p dd -q 'avg:system.cpu.user{*}' --from now-1h
```

**Metric query with tag filter and grouping**:
```bash
obz metric query -p dd-prod \
  -q 'avg:system.cpu.user{env:prod} by {host}' --from now-6h
```

**List available metrics**:
```bash
obz metric list -p dd-prod
```

**Get metric metadata**:
```bash
obz metric info -p dd-prod system.cpu.user
```

**Log search**:
```bash
obz log search -p dd -q 'service:web status:error' --from now-1h
```

**Log search with attribute filter (pre-configured)**:
```bash
obz log search -p dd-prod \
  -q 'service:api @http.status_code:500' --from now-30m
```

**Trace search**:
```bash
obz trace search -p dd-prod -q 'service:api' --from now-1h
```

**Get a specific trace**:
```bash
obz trace get -p dd-prod abc123def456
```

**Aggregate log counts by service**:
```bash
obz log aggregate -p dd -q 'status:error' --compute count --group-by service --from now-1h
```

**Aggregate trace durations**:
```bash
obz trace aggregate -p dd -q 'service:api' --compute 'avg:@duration' --group-by resource_name --from now-1h
```

**EU region endpoint** (set endpoint in config or override with `--endpoint`):
```bash
obz metric query -p dd --endpoint https://api.datadoghq.eu \
  -q 'avg:system.cpu.user{*}' --from now-1h
```

