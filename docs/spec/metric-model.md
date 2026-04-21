# obz Metric Data Model Specification

> Final conclusions and interface specifications only.
> No decision rationale, no Rust structs.
>
> **Implementation status**: The `--view` flag is not yet implemented. The current version outputs all fields (equivalent to Full View).
> The Agent View / Full View differences described below are planned behavior.
> The Full View NaN→`"NaN"` string projection is also not yet implemented (NaN is currently unified as `null`).
> NaN values are excluded from stats computation (same as ±Inf); only finite values participate in min/max/avg calculation.
>
> **Output projection (implemented)**: All metric query commands support `--fields` and `--truncate` for field projection and string truncation at the output layer, applied to series / info / items results. See [`output-projection.md`](./output-projection.md) for the full rules.

---

## 1. Output Examples

### 1.1 Agent View — matrix (range query, default)

```json
{
  "status": "success",
  "metadata": {
    "provider": "dev-vm",
    "total_count": 2
  },
  "data": {
    "result_type": "matrix",
    "series": [
      {
        "name": "cpu_usage",
        "labels": {"env": "prod", "host": "web01"},
        "points": [[1711234800, 75.3], [1711234860, 82.1]],
        "stats": {"min": 75.3, "max": 82.1, "avg": 78.7, "count": 2}
      },
      {
        "name": "cpu_usage",
        "labels": {"env": "prod", "host": "web02"},
        "points": [[1711234800, 45.2], [1711234860, 51.8]],
        "stats": {"min": 45.2, "max": 51.8, "avg": 48.5, "count": 2}
      }
    ]
  }
}
```

### 1.2 Agent View — vector (instant query)

```json
{
  "status": "success",
  "metadata": {
    "provider": "dev-vm",
    "total_count": 2
  },
  "data": {
    "result_type": "vector",
    "series": [
      {
        "name": "cpu_usage",
        "labels": {"env": "prod", "host": "web01"},
        "points": [[1711234800, 75.3]],
        "stats": {"min": 75.3, "max": 75.3, "avg": 75.3, "count": 1}
      }
    ]
  }
}
```

> vector and matrix share the same `data` structure (both use `series[]`); only `result_type` differs.

### 1.3 Agent View — scalar

```json
{
  "status": "success",
  "metadata": {
    "provider": "dev-vm",
    "total_count": 1
  },
  "data": {
    "result_type": "scalar",
    "scalar": [1711234800, 1]
  }
}
```

> `scalar` format is `[timestamp, value]`. No series concept, no stats.

### 1.4 Full View (`--view full`)

```json
{
  "status": "success",
  "metadata": {
    "provider": "dev-vm",
    "provider_type": "victoriametrics",
    "query_language": "MetricsQL",
    "query": "cpu_usage{env=\"prod\"}",
    "time_range": {"start": 1711231200, "end": 1711234800},
    "total_count": 2,
    "is_complete": true
  },
  "data": {
    "result_type": "matrix",
    "series": [
      {
        "name": "cpu_usage",
        "labels": {"env": "prod", "host": "web01"},
        "points": [[1711234800, 75.3], [1711234860, 82.1]],
        "stats": {"min": 75.3, "max": 82.1, "avg": 78.7, "count": 2},
        "extensions": {
          "vm.step": 60
        }
      }
    ]
  }
}
```

> Full View adds the following over Agent View: `metadata.provider_type` / `query_language` / `query` / `time_range` / `is_complete`, and `extensions` within each series.

### 1.5 Table View (`-o table`)

**vector (instant query)**:

```
NAME        LABELS                  VALUE   TIMESTAMP
cpu_usage   env=prod,host=web01    75.3    2024-03-24 10:00:00
cpu_usage   env=prod,host=web02    45.2    2024-03-24 10:00:00
```

**matrix (range query)**:

```
NAME        LABELS                  POINTS  MIN    MAX    AVG
cpu_usage   env=prod,host=web01    20      68.9   82.1   75.4
cpu_usage   env=prod,host=web02    20      45.2   51.8   48.4

(2 series, 40 total points. Use -o json or -o csv for full data.)
```

### 1.6 Metadata Query Output

**metric list** (`result_type: metric_list`):

```json
{
  "status": "success",
  "metadata": {"provider": "dev-vm", "total_count": 3},
  "data": {
    "result_type": "metric_list",
    "items": ["cpu_usage", "http_requests_total", "node_memory_MemAvailable_bytes"]
  }
}
```

**metric info** (`result_type: metric_info`):

```json
{
  "status": "success",
  "metadata": {"provider": "dev-vm"},
  "data": {
    "result_type": "metric_info",
    "info": {
      "name": "node_cpu_seconds_total",
      "type": "counter",
      "description": "Seconds the CPUs spent in each mode.",
      "unit": "seconds"
    }
  }
}
```

> `metric info` output retains `type`/`unit` because these are the core return values for metadata queries. This differs from query results, where series omit `type`/`unit`.

**metric labels** (`result_type: label_list`):

```json
{
  "status": "success",
  "metadata": {"provider": "dev-vm"},
  "data": {
    "result_type": "label_list",
    "items": ["__name__", "env", "host", "instance", "job"]
  }
}
```

**metric label-values** (`result_type: label_values`):

```json
{
  "status": "success",
  "metadata": {"provider": "dev-vm"},
  "data": {
    "result_type": "label_values",
    "label": "job",
    "items": ["api-server", "node-exporter", "prometheus"]
  }
}
```

**metric series** (`result_type: series`):

```json
{
  "status": "success",
  "metadata": {"provider": "dev-vm", "total_count": 3},
  "data": {
    "result_type": "series",
    "series": [
      {"__name__": "up", "job": "prometheus", "instance": "localhost:9090"},
      {"__name__": "up", "job": "api-server", "instance": "10.0.0.1:8080"}
    ]
  }
}
```

---

## 2. Core Structures

### 2.1 MetricSeries — Single Time Series

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | string | Yes | Metric name |
| `labels` | `{key: value}` | Yes | Label key-value pairs, BTreeMap, always sorted alphabetically by key |
| `points` | `[[ts, val], ...]` | Yes | Time series data (see 2.3) |
| `stats` | SeriesStats? | No | Precomputed statistical summary (see 2.2). Computed for matrix/vector; absent for scalar |
| `extensions` | `{key: any}?` | Full View | Provider-specific information (omitted in Agent View) |

### 2.2 SeriesStats — Statistical Summary

| Field | Type | Description |
|-------|------|-------------|
| `min` | number? | Minimum value across points (null when all NaN) |
| `max` | number? | Maximum value across points |
| `avg` | number? | Arithmetic mean |
| `count` | int | Number of finite data points (excluding NaN and ±Inf) |

- NaN and ±Inf excluded from stats (code uses `is_finite()`)
- For vector (single point): min=max=avg=the point's value, count=1
- Scalar results have no stats

### 2.3 points Format

`points` uses the `[[timestamp, value], ...]` **two-dimensional array** format (not an array of objects), saving approximately 75% of tokens.

- timestamp is always **Unix seconds** (int)
- value is float64

| Value Type | JSON Representation | Description |
|------------|---------------------|-------------|
| Normal number | `number` | e.g. `75.3` |
| NaN | `null` | Mapped to null in Agent View |
| +Inf | `"+Inf"` | JSON doesn't support Infinity; uses string |
| -Inf | `"-Inf"` | Same as above |

### 2.4 MetricInfoDetail — Metric Metadata

Return value for the `metric info` command:

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | string | Yes | Metric name |
| `type` | string? | No | gauge / counter / histogram / unknown |
| `description` | string? | No | Description |
| `unit` | string? | No | Unit |

### 2.5 extensions

Provider-specific extra information, included only in Full View output:

| Provider | Field | Description |
|----------|-------|-------------|
| VM | `vm.step` | Range query step interval (seconds) |
| Datadog | `datadog.interval` | Flush interval |
| Datadog | `datadog.scope` | Scope string |
| SLS | `sls.topic` | SLS topic |

> **Note**: In the current implementation, only Datadog extensions (`datadog.interval`, `datadog.scope`) are actively populated by the provider conversion code. `vm.step` and `sls.topic` are reserved keys documented for future use but not yet written by provider code.

---

## 3. Error Response

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
    "suggestion": "Check the PromQL/MetricsQL expression syntax"
  }
}
```

| Field | Description |
|-------|-------------|
| `category` | `auth` / `flag` / `provider` / `network` / `unsupported` |
| `code` | Machine-readable code: `query_syntax` / `auth_expired` / `rate_limited` / `not_supported` etc. |
| `provider` | Name of the provider that triggered the error |
| `message` | Error description (human-readable, formatted by obz) |
| `raw_error` | Raw error from the backend API (full string, present only for provider/network categories) |
| `recoverable` | `true` = retryable, `false` = should give up |
| `suggestion` | Suggested fix |
| `doc_url` | Documentation link |
| `source_chain` | Error source chain (`string[]?`). The `.source()` chain from underlying library errors (e.g. TLS/DNS/IO). Present only for network category errors |

---

## 4. Cross-Platform Conversion Rules

### 4.1 VM / Prometheus / Mimir → obz

VictoriaMetrics, Prometheus, and Grafana Mimir share the `promql` module and use the standard Prometheus HTTP API format. Their conversion rules are identical:

| Prometheus API Field | obz Field | Conversion |
|----------------------|-----------|------------|
| `metric.__name__` | `series[].name` | Extracted |
| `metric.{others}` | `series[].labels` | Direct mapping after removing `__name__` |
| `values[][0]` | `points[][0]` | float → int truncation to seconds |
| `values[][1]` | `points[][1]` | string → parse f64 |
| step parameter | `extensions.vm.step` | Full View |

Provider differences:
- **VictoriaMetrics**: Query language identified as `MetricsQL`
- **Prometheus**: Query language identified as `PromQL`
- **Grafana Mimir**: Query language identified as `PromQL`; multi-tenancy configured via `X-Scope-OrgID` header in the `headers:` block of `config.yaml`

### 4.2 Datadog → obz

| Datadog Field | obz Field | Conversion |
|---------------|-----------|------------|
| `series[].metric` | `series[].name` | Direct mapping |
| `series[].tag_set` | `series[].labels` | `split_once(':')` splits into key-value |
| `series[].pointlist` | `series[].points` | Milliseconds → seconds (integer division) |
| `series[].interval` | `extensions.datadog.interval` | Full View |
| `series[].scope` | `extensions.datadog.scope` | Full View |

> `split_once(':')` ensures that `url:https://example.com` splits into `("url", "https://example.com")`.

### 4.3 SLS → obz

SLS MetricStore uses a Prometheus-compatible API and returns the same format as section 4.1. The conversion rules are identical.

SLS-specific details:
- Endpoint format: `https://{project}.{sls-endpoint}/prometheus/{project}/{metricstore}/api/v1/...`
- Authentication: Basic Auth (AccessKey ID / AccessKey Secret)
- `metric info` is not supported (SLS MetricStore does not store Prometheus metadata)

---

## 5. Edge Cases

### NaN and Infinity

| Value | Agent View | Full View |
|-------|------------|-----------|
| NaN | `null` | `"NaN"` |
| +Inf | `"+Inf"` | `"+Inf"` |
| -Inf | `"-Inf"` | `"-Inf"` |

### Datadog Tags Containing Colons

`url:https://example.com` → `split_once(':')` → key=`url`, value=`https://example.com`.

### Timestamp Precision

All timestamps from backends are converted to Unix seconds. Millisecond precision is truncated. Original precision can be preserved through extensions.

### labels Ordering

Always sorted alphabetically by key (BTreeMap), ensuring consistent output order for the same query across invocations.
