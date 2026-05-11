---
name: obz-greptimedb
description: >
  GreptimeDB provider for obz. Covers PromQL metric query, list, labels,
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
| Supported cmds   | query, list, labels, label-values, series          |

## Supported Commands

GreptimeDB supports these metric commands:

```
obz metric query        # Run an instant or range PromQL query
obz metric list         # List available metric names
obz metric labels       # List all known label names
obz metric label-values # List values for a given label
obz metric series       # List matching time series
```

`obz metric info` is not declared as supported because GreptimeDB does not
currently expose the Prometheus metadata endpoint used by that command.

## Quick Start

```bash
# Instant query
obz metric query -p greptimedb --endpoint http://localhost:4000 -q 'up'

# Range query (last hour, auto step)
obz metric query -p greptimedb --endpoint http://localhost:4000 \
  -q 'rate(http_requests_total[5m])' --from now-1h --range

# List metric names
obz metric list -p greptimedb --endpoint http://localhost:4000

# List label names
obz metric labels -p greptimedb --endpoint http://localhost:4000

# Get label values
obz metric label-values -p greptimedb --endpoint http://localhost:4000 --label __name__
```

## Authentication

GreptimeDB supports bearer token auth, basic auth (username + password), or no
auth. Configure credentials in `~/.config/obz/config.yaml`:

```yaml
providers:
  greptime-prod:
    provider: greptimedb
    endpoint: http://localhost:4000
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
```

## API Endpoints

GreptimeDB exposes a Prometheus-compatible HTTP API under `/v1/prometheus`.
obz automatically routes to the correct paths:

| Command               | Endpoint                                          |
|-----------------------|---------------------------------------------------|
| `metric query` (instant) | `GET /v1/prometheus/api/v1/query`            |
| `metric query` (range)   | `GET /v1/prometheus/api/v1/query_range`      |
| `metric list`            | `GET /v1/prometheus/api/v1/label/__name__/values` |
| `metric labels`          | `GET /v1/prometheus/api/v1/labels`           |
| `metric label-values`    | `GET /v1/prometheus/api/v1/label/{name}/values` |
| `metric series`          | `GET /v1/prometheus/api/v1/series`           |
| `provider check`         | `GET /v1/health`                             |

## Database Selection

GreptimeDB routes queries to the `public` database by default. This is
correct for most single-tenant deployments. If your metrics are stored in
a different database, include the `db` query parameter directly in your
PromQL query URL or consult the GreptimeDB documentation for multi-tenant
configuration.

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
```

```bash
obz provider check greptimedb
```

This issues a `GET /v1/health` request to confirm the GreptimeDB instance
is reachable.
