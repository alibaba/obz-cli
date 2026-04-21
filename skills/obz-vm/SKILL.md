---
name: obz-vm
description: >
  VictoriaMetrics provider for obz. Covers all 6 metric commands (query, list,
  info, labels, label-values, series) using MetricsQL, a PromQL superset with
  extensions like keep_metric_names and range_normalize. This skill should be
  used when the user mentions "VictoriaMetrics", "MetricsQL", "obz metric -p vm",
  or needs to query metrics from a VM backend.
---

# obz-vm: VictoriaMetrics Provider

## Quick Reference

| Field            | Value                                              |
|------------------|----------------------------------------------------|
| Aliases          | `vm`, `victoriametrics`                            |
| Signal           | Metric                                             |
| Query language   | MetricsQL (PromQL superset)                        |
| Auth             | Bearer token, Basic auth, or none                  |
| Provider flags   | None                                               |
| Supported cmds   | query, list, info, labels, label-values, series    |

## Supported Commands

All six core metric commands work with this provider:

```
obz metric query        # Run an instant or range MetricsQL query
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
  vm:
    endpoint: http://localhost:8428
    auth:
      token: ${env:VM_TOKEN}
```

Then query with just `-p`:
```bash
obz metric query -p vm -q 'up'
```

## MetricsQL Guide

MetricsQL is fully compatible with PromQL and adds useful extensions.

### Standard PromQL (works as-is)

```
rate(http_requests_total[5m])
sum by (job) (up)
histogram_quantile(0.99, rate(http_duration_seconds_bucket[5m]))
```

### MetricsQL Extensions

**keep_metric_names** preserves the original metric name through functions
that normally drop it:
```
rate(http_requests_total[5m]) keep_metric_names
```

**label_set** adds or overrides labels in results:
```
label_set(up, "env", "production")
```

**range_normalize** scales values to a 0..1 range across the time window:
```
range_normalize(cpu_usage_percent)
```

**default** fills gaps with a fallback value:
```
absent_over_time(up[5m]) default 0
```

**Day/week suffixes**: MetricsQL supports `d` for days and `w` for weeks,
which standard PromQL doesn't:
```
avg_over_time(cpu_idle[7d])
```

**Subquery shorthand** with trailing `[range:step]`:
```
max_over_time(rate(errors_total[5m])[1h:1m])
```

### Common Pitfalls

- MetricsQL auto-aligns `[range]` to the query step. If you see unexpected
  smoothing, set an explicit step with `--step`.
- `keep_metric_names` only works on functions returning a single series
  per input series. Aggregations like `sum()` ignore it.
- Label filters are case-sensitive. `{job="API"}` won't match `job="api"`.

## Examples

**Instant query**:
```bash
obz metric query -p vm -q 'rate(http_requests_total[5m])' --from now-1h
```

**Range query, 5m step**:
```bash
obz metric query -p vm -q 'avg by (instance) (node_cpu_seconds_total)' \
  --from now-1d --to now --step 5m
```

**List metric names matching a prefix**:
```bash
obz metric list -p vm --match 'http_*'
```

**Get metadata for a metric**:
```bash
obz metric info -p vm http_requests_total
```

**List all label names**:
```bash
obz metric labels -p vm
```

**List values of a specific label**:
```bash
obz metric label-values -p vm job
```

**Find series matching a selector**:
```bash
obz metric series -p vm --match '{job="api", env="prod"}'
```

