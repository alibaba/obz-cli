---
name: obz-greptimedb
description: >
  GreptimeDB provider for obz. Covers PromQL metric query, labels,
  label-values, and series commands via GreptimeDB's Prometheus-compatible API.
  Use this skill when the user mentions "GreptimeDB", "Greptime",
  "obz metric -p greptimedb", or needs to query metrics stored in a
  GreptimeDB instance.
---

# obz-greptimedb: GreptimeDB Provider

## Quick Reference

| Field            | Value                                              |
|------------------|----------------------------------------------------|
| Aliases          | `greptimedb`, `greptime`                           |
| Signal           | Metric                                             |
| Query language   | PromQL                                             |
| Default port     | 4000 (HTTP)                                        |
| Auth             | Bearer token, Basic auth, or no auth               |
| Provider flags   | None                                               |
| Config           | `endpoint` root URL, required `db`                    |
| Supported cmds   | query, labels, label-values, series                |

## Supported Commands

GreptimeDB supports these metric commands:

```
obz metric query        # Run an instant or range PromQL query
obz metric labels       # List all known label names
obz metric label-values # List values for a given label (requires --match)
obz metric series       # List matching time series
```

`obz metric info` is not declared as supported because GreptimeDB does not
currently expose the Prometheus metadata endpoint used by that command.

`obz metric list` is also not declared as supported. Local E2E against
GreptimeDB v1.0.1 showed `/label/__name__/values` can return an empty list, and
GreptimeDB documentation does not guarantee reliable metric-list semantics.
Use `obz metric series --match ...` or query known metric names instead.

## Quick Start

```bash
# Instant query
obz metric query -p greptimedb --endpoint http://localhost:4000 -q 'up'

# Range query (last hour, auto step)
obz metric query -p greptimedb --endpoint http://localhost:4000 \
  -q 'rate(http_requests_total[5m])' --from now-1h

# List label names
obz metric labels -p greptimedb --endpoint http://localhost:4000

# Get label values. GreptimeDB requires an explicit series selector.
obz metric label-values -p greptimedb --endpoint http://localhost:4000 job \
  --match 'up'

# Discover matching series when you know a selector.
obz metric series -p greptimedb --endpoint http://localhost:4000 \
  --match '{__name__="up"}'
```

## Authentication

GreptimeDB supports bearer token auth, basic auth (username + password), or no
auth. Configure credentials in `~/.config/obz/config.yaml`:

```yaml
providers:
  greptime-prod:
    provider: greptimedb
    endpoint: http://localhost:4000
    db: metrics
    auth:
      username: ${env:GREPTIME_USER}
      password: ${env:GREPTIME_PASS}
```

For token-based deployments:

```yaml
providers:
  greptime-cloud:
    provider: greptimedb
    endpoint: https://greptime.example.com
    db: public
    auth:
      token: ${env:GREPTIME_TOKEN}
```

Then query with just `-p`:

```bash
obz metric query -p greptime-prod -q 'up'
```

For unauthenticated local development instances:

```yaml
providers:
  greptime-local:
    provider: greptimedb
    endpoint: http://localhost:4000
    db: public
```

## API Endpoints

GreptimeDB exposes a Prometheus-compatible HTTP API under `/v1/prometheus`.
Configure `endpoint` as the root HTTP(S) URL only. It must not include any path,
query string, or fragment; obz appends the API path and injects the `db` query
parameter automatically.

Valid:

```yaml
endpoint: http://localhost:4000
db: metrics
```

Invalid:

```yaml
endpoint: http://localhost:4000?db=metrics
endpoint: http://localhost:4000/dashboard
endpoint: http://localhost:4000/v1/prometheus
endpoint: http://localhost:4000/api/v1
```

obz routes supported commands to these paths:

| Command               | Endpoint                                          |
|-----------------------|---------------------------------------------------|
| `metric query` (instant) | `GET /v1/prometheus/api/v1/query`            |
| `metric query` (range)   | `GET /v1/prometheus/api/v1/query_range`      |
| `metric labels`          | `GET /v1/prometheus/api/v1/labels`           |
| `metric label-values`    | `GET /v1/prometheus/api/v1/label/{name}/values` |
| `metric series`          | `GET /v1/prometheus/api/v1/series`           |
| `provider check`         | `GET /v1/health`                             |

## Database Selection

GreptimeDB routes Prometheus API requests by the `db` query parameter. obz
requires `db` in the provider config and injects it into every GreptimeDB
Prometheus API request. Use `db: public` when targeting GreptimeDB's standard
public database:

```yaml
providers:
  greptime-prod:
    provider: greptimedb
    endpoint: http://localhost:4000
    db: metrics
```

## PromQL Guide

GreptimeDB supports standard PromQL. Common patterns:

```promql
# Current value of a gauge
node_memory_MemAvailable_bytes

# Per-second rate over 5 minutes
rate(http_requests_total[5m])

# Aggregation
sum by (job) (up)

# Histogram quantile
histogram_quantile(0.99, rate(http_request_duration_seconds_bucket[5m]))
```

## Provider Check

Verify connectivity:

```yaml
providers:
  greptimedb:
    provider: greptimedb
    endpoint: http://localhost:4000
    db: public
```

```bash
obz provider check greptimedb
```

This issues a `GET /v1/health` request to confirm the GreptimeDB instance
is reachable.
