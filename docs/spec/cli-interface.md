# obz CLI Interface Specification

> Command definitions for the feature intersection reachable across 12 providers
> (VictoriaMetrics / VictoriaLogs / VictoriaTraces / SLS / Datadog / Prometheus / Jaeger / OpenSearch / Elasticsearch / Grafana Mimir / Grafana Loki / Grafana Tempo).
> Contains only final conclusions and interface specs, no decision rationale.
>
> **Implementation status labels**: `[Implemented]` means shipped in code, `[Not Implemented]` means designed but not yet built (planned for v0.2.0+).
>
> Per-provider usage details and examples are in [Provider Skills](../../skills/).

---

## 1. Command Tree

```
obz
├── metric                                [Implemented]
│   ├── query                # Metric query (instant / range)
│   ├── list                 # List metric names
│   ├── info                 # Get metric metadata
│   ├── labels               # List label names
│   ├── label-values         # List label values
│   └── series               # Find matching time series
│
├── log                                   [Implemented]
│   ├── search               # Log search
│   ├── labels               # List label names (Loki extension command)
│   ├── label-values         # List label values (Loki extension command)
│   ├── fields               # List detected field names (Loki extension command)
│   ├── field-names          # List field names (VL extension command)
│   ├── field-values         # List field values (VL extension command)
│   ├── hits                 # Log volume distribution (VL extension command)
│   ├── stats                # Statistical aggregation query (Loki / VL extension command)
│   └── aggregate            # Log aggregation (DD extension command)
│
├── trace                                 [Implemented]
│   ├── search               # Span search
│   ├── get                  # Get full trace by traceID
│   ├── services             # List service names (VT/Jaeger extension command)
│   ├── operations           # List operation names (VT/Jaeger extension command)
│   ├── tags                 # List tag names (Tempo extension command)
│   ├── tag-values           # List tag values (Tempo extension command)
│   └── aggregate            # Span aggregation (DD extension command)
│
├── provider                              [Partial]
│   ├── list                  [Implemented]
│   └── check                 [Implemented]
│
├── completions                           [Implemented]
│   └── <shell>              # Generate shell completion script (bash/zsh/fish/powershell/elvish)
│
├── generate-man-pages                    [Implemented, hidden command]
│
└── skills                                [Implemented]
    ├── list                  [Implemented]
    ├── show                  [Implemented]
    └── install               [Implemented]
```

**Query language reference**:

| Provider | Metric Query Language | Log Query Language | Trace Query Language |
|----------|----------------------|-------------------|---------------------|
| VictoriaMetrics (`vm`) | MetricsQL (PromQL superset) | — | — |
| Prometheus (`prom`) | PromQL | — | — |
| Grafana Mimir (`mimir`) | PromQL | — | — |
| VictoriaLogs (`vl`) | — | LogsQL | — |
| Grafana Loki (`loki`) | — | LogQL | — |
| VictoriaTraces (`vt`) | — | — | Jaeger API (service name query) |
| Jaeger (`jg`) | — | — | Jaeger API (service name query) |
| Grafana Tempo (`tempo`) | — | — | TraceQL |
| OpenSearch (`os`) | — | OpenSearch DSL | OpenSearch DSL |
| Elasticsearch (`es`) | — | ES Query DSL | ES Query DSL |
| Datadog (`dd`) | Datadog Query | Datadog Log Query Syntax | Span Search Syntax |
| SLS (`sls`) | PromQL (standard, via Prometheus-compatible API) | SLS keyword + SQL pipeline | SLS Query |

---

## 2. Common Flags

### 2.1 Query Command Common Flags

Inherited by all `metric` / `log` / `trace` query commands:

| Flag | Short | Type | Default | Description | Status |
|------|-------|------|---------|-------------|--------|
| `--provider` | `-p` | string | — | Provider name or config instance name (e.g. `vm`, `sls`, `my-sls-logs`) | Implemented |
| `--endpoint` | — | string | — | Provider backend URL (can be omitted when configured) | Implemented |
| `--output` | `-o` | enum | `json` | Output format: `json`/`table`/`csv` | Implemented |
| `--fields` | — | string | — | Keep only specified output fields, supports comma separation and dot paths (e.g. `labels.env`) | Implemented |
| `--truncate` | — | int | — | Truncate string values in output to N characters with a truncation marker | Implemented |
| `--timeout` | — | duration | `30s`* | HTTP request timeout (e.g. `60s`, `2m`) | Implemented |
| `--verbose` | `-v` | bool | false | Print HTTP request/response details to stderr | Implemented |
| `--view` | — | enum | `agent` | Output view: `agent` (compact) / `full` (complete) | Not Implemented |

> **Command boundaries**: `provider list`, `completions`, and `generate-man-pages` do not accept query command common flags (including `--output`, connection flags, auth flags, and `--verbose`).
>
> **Config file (Implemented)**: Provider endpoints, auth credentials, etc. are all configured in `~/.config/obz/config.yaml` (customizable via `OBZ_CONFIG_DIR`). Complete your `config.yaml` setup before querying, then reference providers with `-p <name>`. The CLI provides no auth-related flags. All credentials are managed through the config file's `providers.<name>.auth` block or `credential-process`. See the Config Resolution section in `AGENTS.md`.
>
> **Note**: `--view` is planned for v0.2.0.
>
> *`--timeout` has no `default_value` at the clap layer; the `30s` default is applied at runtime by the config / provider layer.

**Output projection examples**:

```bash
# Keep only core log fields
obz log search -q 'error' --fields timestamp,service,message

# Keep main trace span fields with long strings truncated to 200 characters
obz trace get abc123 --fields service,name,attributes --truncate 200
```

### 2.2 Query-type Common Flags

Applicable to query-type commands such as `metric query`, `log search`, etc.:

| Flag | Short | Type | Default | Description |
|------|-------|------|---------|-------------|
| `--query` | `-q` | string | varies* | Query expression (syntax depends on the current provider) |
| `--from` | — | string | varies | Time window start |
| `--to` | — | string | `now` | Time window end |
| `--limit` | `-n` | int | varies | Maximum number of results |

> *`--query`: required for `metric query` and `trace search`; optional for `log search` with default `*`.

### 2.3 Time Parameter Formats

Four formats are supported. The CLI converts all of them to Unix seconds before passing to the backend:

| Format | Example |
|--------|---------|
| RFC3339 absolute time | `2026-03-24T10:00:00+08:00`, `2026-03-24T10:00:00Z` |
| Relative time | `now-1h`, `now-30m`, `now-7d` |
| Shorthand duration | `-1h` (equivalent to `now-1h`, `--from` only) |
| Unix timestamp | `@1740280800` (10 digits = seconds, 13 digits = milliseconds, auto-converted) |

**Duration syntax**: `<integer><unit>`, units `s`/`m`/`h`/`d`/`w`. Fractions and compound durations are not supported (e.g. `1.5h`, `1h30m` are invalid).

**RFC3339 without timezone is parsed as UTC**. `--from > --to` raises a flag error.

**Backend time format conversion**:

| Backend | Format sent to API |
|---------|--------------------|
| VM / Prometheus / Mimir | Unix seconds (float64) |
| VL | Unix seconds + RFC 3339 |
| VT / Jaeger | Microseconds (Jaeger API) |
| Loki | Nanoseconds |
| Tempo | Unix seconds (int) |
| OpenSearch | ISO 8601 string |
| Elasticsearch | epoch_millis string |
| Datadog v1 | Unix seconds (int) |
| Datadog v2 | ISO 8601 |
| SLS | Unix seconds (int) |

---

## 3. Metric Commands

### 3.1 `obz metric query` — Metric Query

**Flags**:

| Flag | Short | Type | Required | Default | Description |
|------|-------|------|----------|---------|-------------|
| `--query` | `-q` | string | Yes | — | Query expression (PromQL / MetricsQL / DQL) |
| `--from` | — | string | No | `now-1h` | Time window start |
| `--to` | — | string | No | `now` | Time window end |
| `--step` | — | duration | No | auto | Range query step interval (effective for VM/SLS; not supported by DD) |
| `--limit` | `-n` | int | No | 1000 | Maximum number of series returned |

**Provider-specific flags**:

| Flag | Provider | Description | Status |
|------|----------|-------------|--------|
| `--project` | SLS | SLS Project name | Implemented |
| `--metricstore` | SLS | SLS MetricStore name | Implemented |

**Behavior**:
- `--query` only, without `--from`/`--to`/`--step`: VM/SLS runs an instant query, DD queries the last 1h
- With `--from`/`--to` or `--step`: VM/SLS runs a range query, DD runs a timeseries query
- `--step` controls the range query step interval

**Output result_type**: `scalar` (instant scalar) / `vector` (instant vector) / `matrix` (range query)

**Examples**:

```bash
# VM: instant query
obz metric query -q 'avg(rate(node_cpu_seconds_total{mode!="idle"}[5m])) by (instance)'

# VM: range query
obz metric query -q 'up{job="prometheus"}' --from now-1h --to now --step 15s

# Datadog: query
obz metric query -p datadog -q 'avg:system.cpu.user{env:prod} by {host}' --from now-1h

# SLS: query MetricStore (PromQL)
obz metric query -p sls -q 'sum(rate(http_requests_total[5m])) by (service)' \
  --from now-1h --step 1m --project my-proj --metricstore prom-store
```

**Backend API mapping**:

| Behavior | VM / Prometheus / Mimir | Datadog | SLS |
|----------|------------------------|---------|-----|
| Instant query | `GET /api/v1/query` | `GET /api/v1/query` | `GET /api/v1/query` (Prom-compatible) |
| Range query | `GET /api/v1/query_range` | `GET /api/v1/query` | `GET /api/v1/query_range` (Prom-compatible) |

> Prometheus and Mimir share the same PromQL HTTP API implementation as VM. Mimir multi-tenancy is configured via the `headers:` block in `config.yaml` by setting the `X-Scope-OrgID` header.

---

### 3.2 `obz metric list` — List Metric Names

**Flags**:

| Flag | Short | Type | Required | Default | Description |
|------|-------|------|----------|---------|-------------|
| `--match` | `-m` | string | No | — | Filter (VM/SLS: series selector; DD: prefix match) |
| `--from` | — | string | No | — | Active metric time range start (omitted = backend decides) |
| `--to` | — | string | No | `now` | Active metric time range end |
| `--limit` | `-n` | int | No | 100 | Maximum results |

**Output result_type**: `metric_list`

**Examples**:

```bash
obz metric list                                    # VM: list all
obz metric list --match 'node_cpu*'                # VM: filter by prefix
obz metric list -p datadog --from now-24h          # DD: list active metrics
obz metric list -p sls --project my-proj --metricstore prom-store  # SLS
```

**Backend API mapping**:

| VM / Prometheus / Mimir | Datadog | SLS |
|------------------------|---------|-----|
| `GET /api/v1/label/__name__/values` | `GET /api/v1/search?q=metrics:{prefix}` | `GET /api/v1/label/__name__/values` (Prom-compatible) |

---

### 3.3 `obz metric info` — Get Metric Metadata

**Flags**:

| Flag | Type | Required | Description |
|------|------|----------|-------------|
| `<metric_name>` | positional | Yes | Metric name |

**Output result_type**: `metric_info`

**Examples**:

```bash
obz metric info node_cpu_seconds_total             # VM
obz metric info -p datadog system.cpu.user         # Datadog
```

**Backend API mapping**:

| VM / Prometheus / Mimir | Datadog | SLS |
|------------------------|---------|-----|
| `GET /api/v1/metadata?metric=<name>` | `GET /api/v1/metrics/{name}` | Not supported (returns unsupported error) |

> Note: SLS MetricStore does not store Prometheus metadata (type/help/unit).

---

### 3.4 `obz metric labels` — List Label Names

**Flags**:

| Flag | Short | Type | Required | Default | Description |
|------|-------|------|----------|---------|-------------|
| `--match` | `-m` | string | No | — | Series selector filter |
| `--from` | — | string | No | — | Time range start (omitted = backend decides) |
| `--to` | — | string | No | `now` | Time range end |

**Output result_type**: `label_list`

**Examples**:

```bash
obz metric labels                                  # VM: list all labels
obz metric labels --match 'http_requests_total'    # VM: filtered
obz metric labels -p sls --project my-proj --metricstore prom-store  # SLS
obz metric labels -p datadog                       # DD: no global support, returns error
```

**Backend API mapping**:

| VM / Prometheus / Mimir | Datadog | SLS |
|------------------------|---------|-----|
| `GET /api/v1/labels` | Not supported (returns unsupported error) | `GET /api/v1/labels` (Prom-compatible) |

---

### 3.5 `obz metric label-values` — List Label Values

**Flags**:

| Flag | Short | Type | Required | Default | Description |
|------|-------|------|----------|---------|-------------|
| `<label_name>` | — | positional | Yes | — | Label name |
| `--match` | `-m` | string | No | — | Series selector filter |
| `--from` | — | string | No | — | Time range start (omitted = backend decides) |
| `--to` | — | string | No | `now` | Time range end |
| `--limit` | `-n` | int | No | 100 | Maximum results |

**Output result_type**: `label_values`

**Examples**:

```bash
obz metric label-values job                        # VM
obz metric label-values instance --match 'up{job="prometheus"}'  # VM: with filter
obz metric label-values job -p sls --project my-proj --metricstore prom-store  # SLS
```

**Backend API mapping**:

| VM / Prometheus / Mimir | Datadog | SLS |
|------------------------|---------|-----|
| `GET /api/v1/label/<name>/values` | Not supported (returns unsupported error) | `GET /api/v1/label/<name>/values` (Prom-compatible) |

---

### 3.6 `obz metric series` — Find Matching Time Series

**Flags**:

| Flag | Short | Type | Required | Default | Description |
|------|-------|------|----------|---------|-------------|
| `--match` | `-m` | string | Yes | — | Series selector, repeatable |
| `--from` | — | string | No | `now-1h` | Time range start |
| `--to` | — | string | No | `now` | Time range end |
| `--limit` | `-n` | int | No | 1000 | Maximum number of series returned |

**Output result_type**: `series`

**Examples**:

```bash
obz metric series --match 'up'                     # VM
obz metric series --match 'up' --match 'node_cpu_seconds_total'  # VM: multiple matchers
obz metric series -p sls --project my-proj --metricstore prom-store --match 'up'  # SLS
```

**Backend API mapping**:

| VM / Prometheus / Mimir | Datadog | SLS |
|------------------------|---------|-----|
| `GET /api/v1/series` | Not supported (returns unsupported error) | `GET /api/v1/series` (Prom-compatible) |

---

## 4. Log Commands

### 4.1 `obz log search` — Log Search

**Flags**:

| Flag | Short | Type | Required | Default | Description |
|------|-------|------|----------|---------|-------------|
| `--query` | `-q` | string | No | `*` | Query expression |
| `--from` | — | string | No | `now-1h` | Time window start |
| `--to` | — | string | No | `now` | Time window end |
| `--limit` | `-n` | int | No | 100 | Maximum number of entries |
| `--field-mapping` | — | string | No | — | Field mapping override: `message=log_body,severity=priority` (Not Implemented) |

**SLS-specific flags**:

| Flag | Description | Status |
|------|-------------|--------|
| `--project` | SLS Project name | Implemented |
| `--logstore` | SLS Logstore name | Implemented |
| `--topic` | Log Topic | Not Implemented |
| `--reverse` | Reverse sort order | Not Implemented |
| `--power-sql` | Enable enhanced SQL | Not Implemented |

**VictoriaLogs-specific flags**:

| Flag | Description | Status |
|------|-------------|--------|
| `--account-id` | Multi-tenant AccountID | Implemented |
| `--project-id` | Multi-tenant ProjectID | Implemented |

**Datadog-specific flags**:

| Flag | Description | Status |
|------|-------------|--------|
| `--indexes` | Log indexes (comma-separated) | Not Implemented |
| `--storage-tier` | Storage tier: `indexes`/`online-archives`/`flex` | Not Implemented |

**OpenSearch-specific flags**:

| Flag | Description | Status |
|------|-------------|--------|
| `--index` | Index name, pattern, or data stream (e.g. `otel-logs-*`) | Implemented |

**Elasticsearch-specific flags**:

| Flag | Description | Status |
|------|-------------|--------|
| `--index` | Index name or data stream (e.g. `logs-generic.otel-default`) | Implemented |

**Special behavior**:
- SLS `x-log-progress: Incomplete` is automatically retried by the CLI (up to 10 times); no user action needed
- When an SLS query contains SQL analytics, `--limit` has no effect; use SQL `LIMIT/OFFSET` instead

**Output result_type**: `log_entries`

**Examples**:

```bash
# SLS: keyword search
obz log search -p sls --project my-project --logstore nginx -q 'error AND status:500' --from now-1h

# SLS: SQL analytics
obz log search -p sls --project my-project --logstore nginx \
  -q '* | SELECT status, count(*) as cnt GROUP BY status ORDER BY cnt DESC LIMIT 10'

# Datadog
obz log search -p datadog -q 'service:web status:error' --from now-1h

# VictoriaLogs
obz log search -p victorialogs -q 'error AND _stream:{host="web01"}' --from now-1h
```

**Backend API mapping**:

| Behavior | SLS | Datadog | VictoriaLogs | Loki | OpenSearch | Elasticsearch |
|----------|-----|---------|--------------|------|------------|---------------|
| Log search | `GET /logstores/{logstore}` | `POST /api/v2/logs/events/search` | `POST /select/logsql/query` | `GET /loki/api/v1/query_range` | `POST /{index}/_search` | `POST /{index}/_search` |
| Query language | SLS keyword + SQL pipeline | Datadog Log Query Syntax | LogsQL | LogQL | OpenSearch DSL | ES Query DSL |
| Pagination | offset/line + progress retry | cursor-based (`page.after`) | limit/offset | limit | size/from | size/from |

---

## 5. Trace Commands

> VictoriaTraces, Jaeger, Grafana Tempo, OpenSearch, Elasticsearch, Datadog, and SLS (7 providers) support trace commands.
> VM, Prometheus, Mimir, VL, and Loki return an `unsupported` error.
> See [trace-model.md](trace-model.md) for the data model.

### 5.1 `obz trace search` — Span Search

**Flags**:

| Flag | Short | Type | Required | Default | Description |
|------|-------|------|----------|---------|-------------|
| `--query` | `-q` | string | Yes | — | Service name or query expression (syntax depends on the provider) |
| `--from` | — | string | No | `now-1h` | Time window start |
| `--to` | — | string | No | `now` | Time window end |
| `--limit` | `-n` | int | No | 20 | Maximum number of spans |

**SLS-specific flags**:

| Flag | Description | Status |
|------|-------------|--------|
| `--project` | SLS Project name | Implemented |
| `--trace-logstore` | SLS trace logstore name | Implemented |

**OpenSearch-specific flags**:

| Flag | Description | Status |
|------|-------------|--------|
| `--index` | Index name or pattern (e.g. `otel-traces-*`) | Implemented |

**Elasticsearch-specific flags**:

| Flag | Description | Status |
|------|-------------|--------|
| `--index` | Index name or data stream (e.g. `traces-generic.otel-default`) | Implemented |

**Output result_type**: `spans`

**Examples**:

```bash
# VictoriaTraces: search by service name
obz trace search -p vt --endpoint http://localhost:10428 -q 'frontend'

# Jaeger: search by service name (-q is the service name, required)
obz trace search -p jg --endpoint http://localhost:16686 -q 'frontend'

# Grafana Tempo: TraceQL query
obz trace search -p tempo --endpoint http://localhost:3200 \
  -q '{ resource.service.name = "frontend" }' --from now-1h

# OpenSearch: search traces
obz trace search -p os --endpoint http://localhost:9200 \
  --index 'otel-traces-*' -q 'service.name:frontend' --from now-1h

# Elasticsearch: search traces
obz trace search -p es --endpoint http://localhost:9200 \
  --index 'traces-generic.otel-default' -q 'service.name:frontend' --from now-1h

# Datadog: search error spans (credentials from config.yaml)
obz trace search -p dd \
  -q 'service:api-gateway @http.status_code:500' --from now-1h

# SLS: search traces (credentials from config.yaml)
obz trace search -p sls \
  --project my-project --trace-logstore trace-store -q 'service:api-gateway'
```

**Backend API mapping**:

| Provider | API |
|----------|-----|
| VT | `GET /select/jaeger/api/traces` |
| Jaeger | `GET /api/traces` |
| Tempo | `GET /api/search` |
| OpenSearch | `POST /{index}/_search` |
| Elasticsearch | `POST /{index}/_search` |
| DD | `POST /api/v2/spans/events/search` |
| SLS | `GET /logstores/{trace-logstore}` |

---

### 5.2 `obz trace get` — Get Full Trace by traceID

**Flags**:

| Flag | Type | Required | Default | Description |
|------|------|----------|---------|-------------|
| `<trace_id>` | positional | Yes | — | Trace ID (hex format) |
| `--from` | string | No | `now-1h` | Time window start (limits search scope) |
| `--to` | string | No | `now` | Time window end |

**Output result_type**: `trace_detail`

**Examples**:

```bash
# VictoriaTraces
obz trace get -p vt --endpoint http://localhost:10428 2e62a34ece72499fa08897393365be2f

# Datadog
obz trace get -p dd abc123def456789012345678 --from now-24h

# SLS (credentials from config.yaml)
obz trace get -p sls \
  --project my-project --trace-logstore trace-store abc123def456789012345678
```

**Backend API mapping**:

| Provider | API | Notes |
|----------|-----|-------|
| VT | `GET /select/jaeger/api/traces/{id}` | Direct ID lookup |
| Jaeger | `GET /api/traces/{id}` | Direct ID lookup |
| Tempo | `GET /api/traces/{id}` | Returns OTLP protobuf-JSON format |
| OpenSearch | `POST /{index}/_search` | `term` query on `traceId` field, max 1000 spans |
| Elasticsearch | `POST /{index}/_search` | `term` query on `trace_id` field, max 1000 spans |
| DD | `POST /api/v2/spans/events/search` | `trace_id:<id>` filter, max 1000 spans |
| SLS | `GET /logstores/{trace-logstore}` | `traceID:{id}` filter, max 1000 spans |

> DD, OpenSearch, Elasticsearch, and SLS don't have a dedicated "get trace by traceID" API. obz implements this via span search + trace_id filtering, then computes the trace summary (span_count, service_count, duration_us, services).

---

## 6. Unified Response Envelope

### 6.1 Success Response

> **Note**: The `--view` flag is not yet implemented. The current version outputs all fields (equivalent to Full View). The Agent View / Full View differences described below are planned behavior.

**Agent View (default)**:

```json
{
  "status": "success",
  "metadata": {
    "provider": "dev-vm",
    "total_count": 15
  },
  "data": {
    "result_type": "matrix",
    "series": [...]
  }
}
```

**Full View (`--view full`)**:

```json
{
  "status": "success",
  "metadata": {
    "provider": "dev-vm",
    "provider_type": "victoriametrics",
    "query_language": "MetricsQL",
    "query": "cpu_usage{env=\"prod\"}",
    "time_range": {"start": 1711231200, "end": 1711234800},
    "total_count": 15,
    "is_complete": true
  },
  "data": {
    "result_type": "matrix",
    "series": [...]
  }
}
```

**Agent View vs Full View differences**: Agent View omits `metadata.provider_type`, `metadata.query_language`, `metadata.query`, `metadata.time_range`, `metadata.is_complete`, and `extensions` within series.

**`metadata.is_complete` field semantics**:

Type is `Option<bool>`, indicating whether the result set contains all matching data:

- `true`: the backend confirms the results are complete.
- `false`: the backend confirms the results are incomplete (truncated).
- Omitted (absent from JSON): the backend doesn't provide a reliable truncation signal (e.g. Loki, VictoriaLogs). The agent can infer from `total_count == limit`.

See [log-model.md §5.6](log-model.md#56-truncation-completeness-signal-metadatais_complete) and [trace-model.md §5.6](trace-model.md#56-truncation-completeness-signal-metadatais_complete) for per-backend behavior.

**`warnings` field**: The `Response` struct contains `warnings: Option<Vec<String>>`. When the backend returns warning information (e.g. partial data unavailable), a `warnings` array appears at the top level of the response.

**`metadata.truncated_values`**: When `--truncate` causes string values in the output to be truncated, `metadata` includes `truncated_values: N` showing the number of truncated values.

### 6.2 Error Response

```json
{
  "status": "error",
  "error": {
    "category": "provider",
    "code": "query_syntax",
    "provider": "dev-vm",
    "message": "invalid expression type \"foo\" for range query",
    "raw_error": "{\"status\":\"error\",\"errorType\":\"bad_data\",\"error\":\"invalid expression type \\\"foo\\\" for range query, must be Scalar or instant Vector\"}",
    "recoverable": false,
    "suggestion": "Check your PromQL/MetricsQL expression syntax",
    "doc_url": "https://docs.victoriametrics.com/metricsql/"
  }
}
```

**error fields**:

| Field | Type | Description |
|-------|------|-------------|
| `category` | string | `auth` / `flag` / `provider` / `network` / `unsupported` |
| `code` | string | Machine-readable: `query_syntax` / `auth_expired` / `rate_limited` / `not_supported`, etc. |
| `provider` | string? | Provider that triggered the error (provider/auth categories only) |
| `message` | string | Error description (human-readable message formatted by obz) |
| `raw_error` | string? | Raw error message from the backend API (full string, provider/network categories only) |
| `recoverable` | bool | `true` = retryable or fixable by changing args, `false` = give up and report to user |
| `suggestion` | string? | Fix suggestion |
| `doc_url` | string? | Documentation link |
| `source_chain` | string[]? | Error source chain (underlying library error chain, e.g. TLS/DNS/IO errors) |

### 6.3 Error Categories

| category | Description |
|----------|-------------|
| `auth` | Authentication error (not logged in, token expired) |
| `flag` | Argument error (invalid time format, missing required field) |
| `provider` | Backend error (4xx/5xx), `raw_error` carries the original response |
| `network` | Network error (DNS, timeout, TLS) |
| `unsupported` | Current provider does not support this command |

### 6.4 Exit Codes

| Exit Code | Category | Description |
|-----------|----------|-------------|
| 0 | — | Success |
| 1 | `auth` | Authentication error |
| 2 | `flag` | Invalid arguments or flags |
| 3 | `provider` | Backend provider error (4xx/5xx) |
| 4 | `network` | Network error (DNS, timeout, TLS) |
| 5 | `unsupported` | Operation not supported by provider |

### 6.5 Error Codes

| Code | Category | Description |
|------|----------|-------------|
| `auth_missing` | auth | Credentials not configured |
| `auth_expired` | auth | Token/credentials expired |
| `access_denied` | auth | Insufficient permissions |
| `query_syntax` | flag | Invalid query expression |
| `invalid_time_range` | flag | Invalid time range or format |
| `missing_required` | flag | Missing required flag |
| `invalid_flag` | flag | Invalid flag value |
| `config_error` | flag | Config file read/parse error |
| `backend_error` | provider | Backend returned error response |
| `rate_limited` | provider | Rate limited by backend |
| `not_found` | provider | Requested resource not found |
| `timeout` | provider/network | Request timed out |
| `dns_error` | network | DNS resolution failed |
| `tls_error` | network | TLS/SSL handshake failed |
| `connection_error` | network | Connection refused or reset |
| `not_supported` | unsupported | Operation not supported |

### 6.6 result_type Enumeration

| result_type | Command | Data field in `data` |
|-------------|---------|---------------------|
| `scalar` | `metric query` | `data.scalar: [ts, val]` |
| `vector` | `metric query` | `data.series: [...]` |
| `matrix` | `metric query` | `data.series: [...]` |
| `metric_list` | `metric list` | `data.items: [string, ...]` |
| `metric_info` | `metric info` | `data.info: {...}` |
| `label_list` | `metric labels` | `data.items: [string, ...]` |
| `label_values` | `metric label-values` | `data.items: [string, ...]` |
| `series` | `metric series` | `data.series: [{labels...}, ...]` |
| `log_entries` | `log search` | `data.entries: [...]` |
| `spans` | `trace search` | `data.spans: [Span, ...]` |
| `trace_detail` | `trace get` | `data.trace_id`, `data.spans: [...]`, `data.span_count`, `data.service_count`, `data.duration_us`, `data.services` |
| `services` | `trace services` | `data.data: [string, ...]` |
| `operations` | `trace operations` | `data.data: [string, ...]` |
| `tags` | `trace tags` | `data.data: [string, ...]` |
| `tag-values` | `trace tag-values` | `data.data: [string, ...]` |

### 6.7 stdout / stderr Routing

| Content | All output formats |
|---------|-------------------|
| Success response | stdout |
| Error response (structured JSON) | **stdout** (unified parsing for agents) |
| Error response (human-readable summary) | **stderr** |
| `-v` logs / progress info | stderr |

> **Note**: The structured JSON error envelope is produced only for query command execution errors (`metric`, `log`, `trace`, and extension commands). Pre-dispatch errors (e.g. config parse failure, `provider list/check` errors) are reported to stderr only with exit code 1, without a JSON envelope on stdout.
