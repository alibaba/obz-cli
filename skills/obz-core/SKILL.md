---
name: obz-core
description: "Multi-backend observability CLI for querying metrics, logs, and traces across 12 backends. This skill should be used when the user runs obz commands, constructs metric/log/trace queries, configures providers, parses obz JSON output, or troubleshoots obz errors. Covers all core commands (metric query/list/info/labels/label-values/series, log search, trace search/get), global flags, time formats, output modes, config files, and exit codes."
---

# obz Core Skill

obz is a multi-backend observability CLI. It gives you one set of commands for metrics, logs, and traces across 12 backends. Output defaults to structured JSON, built for AI agent consumption.

## Providers

| Alias | Full Name | Backend |
|-------|-----------|---------|
| `vm` | `victoriametrics` | VictoriaMetrics |
| `vl` | `victorialogs` | VictoriaLogs |
| `vt` | `victoriatraces` | VictoriaTraces |
| `sls` | `sls` | Alibaba Cloud SLS |
| `dd` | `datadog` | Datadog |
| `prom` | `prometheus` | Prometheus |
| `jg` | `jaeger` | Jaeger |
| `os` | `opensearch` | OpenSearch |
| `es` | `elasticsearch` | Elasticsearch |
| `mimir` | `mimir` | Grafana Mimir |
| `loki` | `loki` | Grafana Loki |
| `tempo` | `tempo` | Grafana Tempo |

Either the alias or the full name can be passed to `-p`.

## Global Flags

```
-p, --provider <name>    Provider name or config instance (required)
    --endpoint <url>     Backend URL (omit if configured)
-o, --output <fmt>       Output format: json | table | csv  [default: json]
    --timeout <dur>      Query timeout (e.g. 30s)
-v, --verbose            Show HTTP request/response details on stderr
```

Auth credentials are configured in `config.yaml` under `providers.<name>.auth`, not via CLI flags.

## Commands

### metric query

Execute a metric query. Omitting `--step` runs an instant query; providing it runs a range query.

```
obz metric query -p <prov> -q <expr> [--from now-1h] [--to now] [--step 1m] [-n 1000] [--timeout 30s]
```

Flags:
- `-q, --query <expr>` (required) Query expression. Language depends on the provider.
- `--from <time>` Start time. Default: `now-1h`.
- `--to <time>` End time. Default: `now`.
- `--step <duration>` Range query step (e.g. `15s`, `1m`). Omit for instant query.
- `-n, --limit <int>` Max series returned. Default: `1000`.
- `--timeout <duration>` Query timeout (e.g. `30s`).

### metric list

List available metric names.

```
obz metric list -p <prov> [-m <filter>] [--from <time>] [-n 100]
```

Flags:
- `-m, --match <expr>` Filter expression.
- `--from <time>` Start time.
- `-n, --limit <int>` Max results. Default: `100`.

### metric info

Get metric metadata (type, description, unit).

```
obz metric info -p <prov> <metric_name>
```

Positional:
- `<metric_name>` (required) The metric to look up.

### metric labels

List label names.

```
obz metric labels -p <prov> [-m <selector>] [--from <time>]
```

Flags:
- `-m, --match <expr>` Series selector to scope the results.
- `--from <time>` Start time.

### metric label-values

List values for a specific label.

```
obz metric label-values -p <prov> <label_name> [-m <selector>] [--from <time>] [-n 100]
```

Positional:
- `<label_name>` (required) The label whose values to list.

Flags:
- `-m, --match <expr>` Series selector.
- `--from <time>` Start time.
- `-n, --limit <int>` Max results. Default: `100`.

### metric series

Find time series matching one or more selectors.

```
obz metric series -p <prov> -m <selector> [-m ...] [--from <time>] [--to <time>] [-n 1000]
```

Flags:
- `-m, --match <selector>` (required, repeatable) Series selector.
- `--from <time>` Start time.
- `--to <time>` End time.
- `-n, --limit <int>` Max results. Default: `1000`.

### log search

Search for log entries.

```
obz log search -p <prov> [-q '*'] [--from now-15m] [--to now] [-n 100]
```

Flags:
- `-q, --query <expr>` Query expression. Default: `*`.
- `--from <time>` Start time. Default: `now-15m`.
- `--to <time>` End time. Default: `now`.
- `-n, --limit <int>` Max entries. Default: `100`.

### trace search

Search for spans across traces.

```
obz trace search -p <prov> -q <expr> [--from now-1h] [--to now] [-n 20]
```

Flags:
- `-q, --query <expr>` (required) Service name or query expression.
- `--from <time>` Start time. Default: `now-1h`.
- `--to <time>` End time. Default: `now`.
- `-n, --limit <int>` Max traces. Default: `20`.

### trace get

Get all spans for a specific trace by ID.

```
obz trace get -p <prov> <trace_id> [--from now-1h] [--to now]
```

Positional:
- `<trace_id>` (required) Hex trace ID.

Flags:
- `--from <time>` Start time. Default: `now-1h`.
- `--to <time>` End time. Default: `now`.

## Time Formats

The `--from` and `--to` flags accept three formats:

| Format | Example | Notes |
|--------|---------|-------|
| Relative | `now-1h`, `now-15m`, `now-2d` | Units: `s`, `m`, `h`, `d`, `w` |
| RFC 3339 | `2024-01-01T00:00:00Z` | UTC timestamp |
| Unix timestamp | `@1704067200` | Prefix with `@`. Accepts 10-digit (seconds) or 13-digit (milliseconds). |

No fractional or compound durations. `1h30m` is not valid; use `90m` instead.

## Output Formats

```
-o json     Structured JSON envelope (default, best for AI agents)
-o table    Human-readable table
-o csv      Comma-separated values
```

JSON responses follow a consistent envelope:

```json
{
  "status": "success",
  "data": { ... },
  "metadata": {
    "provider": "victoriametrics",
    "query_language": "MetricsQL",
    "time_range": { "from": "...", "to": "..." },
    "total_count": 42
  }
}
```

On error, the envelope contains an `error` object instead of `data`:

```json
{
  "status": "error",
  "error": {
    "category": "auth",
    "code": "auth_missing",
    "message": "...",
    "recoverable": false,
    "suggestion": "..."
  }
}
```

## Provider Support Matrix

Run `obz provider list` for a live support matrix. Summary:

| Signal | Providers |
|--------|-----------|
| Metric | vm, sls, dd, prom, mimir |
| Log    | vl, sls, dd, os, es, loki |
| Trace  | vt, sls, dd, jg, os, es, tempo |

Some providers register extension commands (e.g. `log stats` for vl/loki, `trace services` for vt/jg, `log aggregate` for dd). Run `obz log --help -p <prov>` or `obz trace --help -p <prov>` to see available extensions.

## Configuration

Config files live in `~/.config/obz/` (override with `OBZ_CONFIG_DIR`).

### config.yaml

```yaml
providers:
  vm:
    endpoint: http://localhost:8428
    auth:
      token: ${env:OBZ_VM_TOKEN}
  sls:
    endpoint: http://cn-hangzhou.log.aliyuncs.com
    project: my-project
    metricstore: my-metricstore
    logstore: my-logstore
    auth:
      access-key-id: ${file:~/.obz/sls-ak.txt}
      access-key-secret: ${file:~/.obz/sls-sk.txt}
  dd-prod:
    provider: dd
    endpoint: https://api.datadoghq.com
    auth:
      api-key: ${env:DD_API_KEY}
      app-key: ${env:DD_APP_KEY}
```

Each key under `providers` becomes a name for `-p`. If the key is a provider alias (like `vm`), the type is inferred. For custom names (like `dd-prod`), add `provider: dd`.

### Variable references

Config values support variable references resolved at load time:
- `${env:VAR}` — environment variable (error if unset)
- `${env?:VAR}` — environment variable (empty string if unset)
- `${file:path}` — file contents (`~` expansion, relative to config dir)

### credential-process

External command for dynamic credential retrieval:

```yaml
providers:
  es-prod:
    provider: es
    endpoint: https://es.prod:9200
    auth:
      credential-process:
        command: vault
        args: ["kv", "get", "-format=json", "secret/es-prod"]
        timeout: 10s
```

The command must output JSON with `"version": 1` and credential fields (`token`, `username`, `password`, `api-key`, `app-key`, `access-key-id`, `access-key-secret`, `headers`). Results are cached on disk automatically.

### Merge order

CLI flags (`--endpoint`, `--timeout`) override credential-process output, which overrides config.yaml values. A fully configured provider needs only `-p <name>`:

```bash
obz metric query -p vm -q 'up'
obz log search -p sls -q 'error' --from now-1h
```

## Error Exit Codes

| Code | Category | Meaning |
|:----:|----------|---------|
| 1 | `auth` | Authentication or authorization failure |
| 2 | `flag` | Invalid CLI arguments or flags |
| 3 | `provider` | Backend returned an error (4xx/5xx) |
| 4 | `network` | DNS, timeout, TLS, or connection error |
| 5 | `unsupported` | Command not supported by this provider |

Error responses include `recoverable` (bool) and often a `suggestion` field with a recommended fix.

## Quick Reference

| Task | Command |
|------|---------|
| Query metrics | `obz metric query -p vm -q 'up'` |
| Range query with step | `obz metric query -p vm -q 'rate(http_total[5m])' --from now-1h --step 1m` |
| List all metrics | `obz metric list -p vm` |
| Filter metric names | `obz metric list -p vm -m 'node_cpu*'` |
| Get metric metadata | `obz metric info -p vm node_cpu_seconds_total` |
| List label names | `obz metric labels -p vm` |
| List label values | `obz metric label-values -p vm job` |
| Find matching series | `obz metric series -p vm -m 'up'` |
| Search logs | `obz log search -p vl -q 'error' --from now-1h` |
| Search traces | `obz trace search -p vt -q 'frontend'` |
| Get a trace by ID | `obz trace get -p vt abc123def456` |
| Output as table | `obz metric list -p vm -o table` |
| Output as CSV | `obz log search -p vl -q '*' -o csv` |

## Skills

obz bundles per-provider skills with deeper coverage of query languages, authentication, and provider-specific flags. Install them with:

```bash
obz skills install --dir ~/.claude/skills --all
```

Or install only skills matching your configured providers:

```bash
obz skills install --dir ~/.claude/skills
```

List available skills:

```bash
obz skills list
```
