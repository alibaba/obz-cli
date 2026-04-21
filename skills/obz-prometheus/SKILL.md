---
name: obz-prometheus
description: >
  Prometheus provider for obz. Covers all 6 metric commands using standard
  PromQL. This skill should be used when the user mentions "Prometheus",
  "PromQL", "obz metric -p prom", or needs to query metrics from a Prometheus
  server.
---

# obz-prometheus: Prometheus Provider

## Quick Reference

| Field            | Value                                              |
|------------------|----------------------------------------------------|
| Aliases          | `prom`, `prometheus`                               |
| Signal           | Metric                                             |
| Query language   | PromQL                                             |
| Auth             | Bearer token or Basic auth                         |
| Provider flags   | None                                               |
| Supported cmds   | query, list, info, labels, label-values, series    |

## Supported Commands

All six core metric commands work with this provider:

```
obz metric query        # Run an instant or range PromQL query
obz metric list         # List available metric names
obz metric info         # Show metadata for a specific metric
obz metric labels       # List all known label names
obz metric label-values # List values for a given label
obz metric series       # List matching time series
```

## Authentication

Configure auth in `config.yaml` under `providers.<name>.auth`. Supports
bearer token, basic auth, or no auth.

```yaml
# config.yaml
providers:
  prom:
    endpoint: http://localhost:9090
    auth:
      token: ${env:PROM_TOKEN}
```

For basic auth:
```yaml
    auth:
      username: admin
      password: ${env:PROM_PASS}
```

Then query with just `-p`:
```bash
obz metric query -p prom -q 'up'
```

## PromQL Guide

Prometheus uses standard PromQL. Unlike VictoriaMetrics, there are no
MetricsQL extensions like `keep_metric_names` or `range_normalize`.

### Common Patterns

```
rate(http_requests_total[5m])
sum by (job) (up)
histogram_quantile(0.99, rate(http_duration_seconds_bucket[5m]))
avg_over_time(cpu_usage[1h])
```

### Duration Units

PromQL supports `ms`, `s`, `m`, `h`, `d`, `w`, `y`. Note that `d` always
means exactly 24h and `w` means 7d, with no calendar awareness.

## Examples

**Instant query**:
```bash
obz metric query -p prom -q 'up' --from now-1h
```

**Range query with step**:
```bash
obz metric query -p prom -q 'rate(http_requests_total[5m])' \
  --from now-1d --to now --step 5m
```

**List metric names**:
```bash
obz metric list -p prom
```

**Find series matching a selector**:
```bash
obz metric series -p prom --match '{job="api", env="prod"}'
```

**List values of a label**:
```bash
obz metric label-values -p prom job
```

## Prometheus vs VictoriaMetrics

| Feature                  | Prometheus | VictoriaMetrics |
|--------------------------|:----------:|:---------------:|
| PromQL                   | Yes        | Yes             |
| MetricsQL extensions     | No         | Yes             |
| `keep_metric_names`      | No         | Yes             |
| `range_normalize`        | No         | Yes             |
| `d`/`w` duration units   | Yes        | Yes             |

If you find yourself needing MetricsQL-only features, switch to the `vm`
provider instead.

