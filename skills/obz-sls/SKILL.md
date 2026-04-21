---
name: obz-sls
description: >
  Alibaba Cloud SLS (Simple Log Service) provider for obz. Supports metrics
  (PromQL), logs (SLS Query), and traces across project/store pairs. Requires
  AccessKey auth and provider-specific flags like --project, --logstore,
  --metricstore, and --trace-logstore. This skill should be used when the user
  mentions "SLS", "Alibaba Cloud", "obz -p sls", or needs to configure SLS
  provider instances.
---

# obz-sls: Alibaba Cloud SLS Provider

## Quick Reference

| Field            | Value                                              |
|------------------|----------------------------------------------------|
| Aliases          | `sls`                                              |
| Signals          | Metric, Log, Trace                                 |
| Metric query     | PromQL                                             |
| Log query        | SLS Query                                          |
| Auth             | AccessKey (access-key-id + access-key-secret)      |
| Endpoint format  | `https://<project>.<region>.log.aliyuncs.com`      |

## Supported Commands

| Signal  | Commands                                                    |
|---------|-------------------------------------------------------------|
| Metric  | query, list, labels, label-values, series                   |
| Log     | search                                                      |
| Trace   | search, get                                                 |

`metric info` is not supported by SLS.

## Authentication

SLS always requires AccessKey credentials. Configure them in `config.yaml`
under `providers.<name>.auth`:

```yaml
# config.yaml
providers:
  sls:
    endpoint: http://cn-hangzhou.log.aliyuncs.com
    project: my-project
    logstore: my-logs
    metricstore: my-metrics
    auth:
      access-key-id: ${env:SLS_AK}
      access-key-secret: ${env:SLS_SK}
```

Then query with just `-p`:
```bash
obz metric query -p sls -q 'up'
```

## Provider-Specific Flags

| Flag                | Type   | Required For     | Description                   |
|---------------------|--------|------------------|-------------------------------|
| `--project`         | String | All commands     | SLS project name              |
| `--metricstore`     | String | Metric commands  | Metric store within project   |
| `--logstore`        | String | Log commands     | Log store within project      |
| `--trace-logstore`  | String | Trace commands   | Trace log store within project|

These flags are declared `required: false` at the CLI level (so other
providers don't error), but SLS validates them at runtime. Missing a
required flag produces a clear error message.

All flags can be pre-configured in `config.yaml` — when configured, the
CLI flag can be omitted.

## Config File Setup

Pre-configuring SLS avoids repeating long flag lists:

```yaml
# ~/.config/obz/config.yaml
providers:
  sls-prod:
    provider: sls
    endpoint: https://my-proj.cn-hangzhou.log.aliyuncs.com
    project: my-proj
    metricstore: prom-store
    logstore: nginx-access
    trace-logstore: trace-store
    auth:
      access-key-id: ${env:SLS_AK}
      access-key-secret: ${env:SLS_SK}
```

Then query with just a name:
```bash
obz metric query -p sls-prod -q 'up'
obz log search -p sls-prod -q 'error' --from now-1h
obz trace search -p sls-prod -q '*' --from now-1h
```

## SLS Query Language (Logs)

SLS Query supports full-text search, field filters, and boolean logic.

### Full-text Search

```
error
"connection refused"
```

### Field Filters

```
status:500
host:web-01
request_method:POST
```

### Boolean Operators

```
error AND status:500
error OR warn
error NOT "health check"
```

### Wildcards

```
host:web-*
status:5??
```

### Combining Patterns

```
error AND status:500 AND request_method:POST NOT uri:/health
```

### Common Pitfalls

- Field names are case-sensitive and must match the indexed field exactly.
- Bare words search across all text fields. Use `field:value` to target
  a specific field.
- Wildcards work only in field filters, not in quoted phrases.
- SLS Query is NOT the same as LogsQL or Datadog Log Query. Each provider
  has its own syntax.

## PromQL (Metrics)

SLS metric stores support standard PromQL. MetricsQL extensions like
`keep_metric_names` are not available here.

```
rate(http_requests_total[5m])
sum by (job) (up)
histogram_quantile(0.99, rate(http_duration_seconds_bucket[5m]))
```

## Examples

**Metric query**:
```bash
obz metric query -p sls-prod -q 'rate(http_requests_total[5m])' --from now-1h
```

**Log search**:
```bash
obz log search -p sls-prod -q 'error AND status:500' --from now-1h
```

**Trace search**:
```bash
obz trace search -p sls-prod -q '*' --from now-1h
```

**Trace get by ID**:
```bash
obz trace get -p sls-prod abc123def456
```

**List metric names**:
```bash
obz metric list -p sls-prod
```

**List label values**:
```bash
obz metric label-values -p sls-prod job
```
