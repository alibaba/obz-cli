---
name: obz-mimir
description: >
  Grafana Mimir provider for obz. Covers all 6 metric commands using standard
  PromQL. Mimir is a horizontally-scalable Prometheus-compatible TSDB. This
  skill should be used when the user mentions "Mimir", "Grafana Mimir",
  "obz metric -p mimir", or needs to query metrics from a Mimir cluster.
---

# obz-mimir: Grafana Mimir Provider

## Quick Reference

| Field            | Value                                              |
|------------------|----------------------------------------------------|
| Aliases          | `mimir`                                            |
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
  mimir:
    endpoint: https://mimir.example.com
    auth:
      token: ${env:MIMIR_TOKEN}
```

Then query with just `-p`:
```bash
obz metric query -p mimir -q 'up'
```

## PromQL with Mimir

Mimir implements standard PromQL with no proprietary extensions. All valid
PromQL expressions work as-is.

### Common Patterns

```
rate(http_requests_total[5m])
sum by (job) (up)
histogram_quantile(0.99, rate(http_duration_seconds_bucket[5m]))
avg_over_time(cpu_usage[1h])
```

Mimir's strength is horizontal scalability. Queries that touch large
time ranges or high-cardinality series benefit from its distributed
query engine.

## Mimir vs Prometheus vs VictoriaMetrics

| Feature                  | Mimir   | Prometheus | VM    |
|--------------------------|:-------:|:----------:|:-----:|
| PromQL                   | Yes     | Yes        | Yes   |
| MetricsQL extensions     | No      | No         | Yes   |
| Horizontal scaling       | Yes     | No         | Yes   |
| Multi-tenancy            | Yes     | No         | Yes   |

Choose `mimir` for Grafana Mimir deployments. Use `prom` for standalone
Prometheus, or `vm` for VictoriaMetrics.

## Examples

**Instant query**:
```bash
obz metric query -p mimir -q 'up' --from now-1h
```

**Range query with step**:
```bash
obz metric query -p mimir -q 'rate(http_requests_total[5m])' \
  --from now-1d --to now --step 5m
```

**List metric names**:
```bash
obz metric list -p mimir
```

**Find series matching a selector**:
```bash
obz metric series -p mimir --match '{job="api", env="prod"}'
```

**List values of a label**:
```bash
obz metric label-values -p mimir job
```

