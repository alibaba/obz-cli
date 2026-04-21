# obz Log Data Model Specification

> Final conclusions and interface specifications only.
> No decision rationale, no Rust structs.
>
> **Implementation Status**:
> - `--view` flag is **Not Implemented**. The current version outputs all fields (equivalent to Full View). The Agent View / Full View differences described below are planned behavior.
> - `--field-mapping` flag is **Not Implemented**. Field mapping is currently handled by hardcoded heuristics within each provider.
> - `--fields` and `--truncate` are implemented: they apply field projection and string truncation to `entries[]` at the output layer. See [`output-projection.md`](./output-projection.md).

---

## 1. Output Examples

### 1.1 Agent View — SLS Log Search

> v0.1.0 does not include the `summary` field (skipped, not null). Planned for v0.2.0.

```json
{
  "status": "success",
  "metadata": {
    "provider": "prod-sls",
    "total_count": 2
  },
  "data": {
    "result_type": "log_entries",
    "entries": [
      {
        "timestamp": 1711266323,
        "source": "172.16.1.100",
        "severity": "ERROR",
        "message": "Connection refused to upstream server 10.0.1.5:8080 after 3 retries",
        "service": "api-gateway",
        "attributes": {
          "status": "500",
          "request_uri": "/api/v1/users",
          "request_method": "GET",
          "upstream": "10.0.1.5:8080"
        },
        "trace_id": "abc123def456"
      },
      {
        "timestamp": 1711266321,
        "source": "172.16.1.101",
        "severity": "WARN",
        "message": "Slow query detected: SELECT * FROM orders WHERE created_at > '2026-03-23' took 2345ms",
        "attributes": {
          "status": "200",
          "request_uri": "/api/v1/orders",
          "duration_ms": "2345"
        }
      }
    ]
  }
}
```

### 1.2 Agent View — VictoriaLogs Log Search

```json
{
  "status": "success",
  "metadata": {
    "provider": "dev-vl",
    "total_count": 1
  },
  "data": {
    "result_type": "log_entries",
    "entries": [
      {
        "timestamp": 1672577533,
        "source": "web01",
        "severity": "ERROR",
        "message": "error: disconnect from 19.54.37.22: Auth fail [preauth]",
        "attributes": {
          "app": "sshd"
        }
      }
    ]
  }
}
```

### 1.3 Agent View — Datadog Log Search

```json
{
  "status": "success",
  "metadata": {
    "provider": "prod-dd",
    "total_count": 1
  },
  "data": {
    "result_type": "log_entries",
    "entries": [
      {
        "timestamp": 1569256789,
        "source": "i-0123",
        "severity": "ERROR",
        "message": "Connection refused to upstream server 10.0.1.5:8080",
        "service": "web-app",
        "id": "AAAAAWgN8Xwgr1vKDQAAAABBV2dOOFh3ZzZobm1mWXJFYTR0OA",
        "attributes": {
          "customAttribute": "123",
          "duration": "2345"
        }
      }
    ]
  }
}
```

### 1.4 Full View (`--view full`)

```json
{
  "status": "success",
  "metadata": {
    "provider": "prod-sls",
    "provider_type": "sls",
    "query_language": "SLS SQL",
    "query": "error AND status:500",
    "time_range": {"start": 1711262400, "end": 1711266000},
    "total_count": 3,
    "is_complete": true,
    "cursor": null
  },
  "data": {
    "result_type": "log_entries",
    "entries": [
      {
        "timestamp": 1711266323,
        "source": "172.16.1.100",
        "severity": "ERROR",
        "message": "Connection refused to upstream server 10.0.1.5:8080 after 3 retries",
        "service": "api-gateway",
        "attributes": {
          "status": "500",
          "request_uri": "/api/v1/users"
        },
        "resource": {},
        "trace_id": "abc123def456",
        "extensions": {
          "sls.topic": "",
          "sls.meta": {
            "progress": "Complete",
            "processedRows": 10000,
            "elapsedMillisecond": 45
          }
        }
      }
    ]
  }
}
```

> Full View includes additional fields compared to Agent View: `metadata.provider_type`/`query_language`/`query`/`time_range`/`is_complete`/`cursor`, plus `resource` and `extensions` on each entry.

### 1.5 Table View (`-o table`)

```
TIMESTAMP            SOURCE          SEVERITY  SERVICE       MESSAGE
2026-03-24 10:05:23  172.16.1.100    ERROR     api-gateway   Connection refused to upstream server…
2026-03-24 10:05:21  172.16.1.101    WARN                    Slow query detected: 2345ms
2026-03-24 10:05:19  172.16.1.100    ERROR     api-gateway   HTTP 500: Internal Server Error
```

> In table mode, message is truncated and attributes are not displayed.

---

## 2. Core Structure

### 2.1 LogEntry — Single Log Record

| Field | Type | Agent View | Full View | Description |
|-------|------|:----------:|:---------:|-------------|
| `timestamp` | int | required | required | Unix seconds |
| `message` | string | required | required | Log body |
| `severity` | string? | emitted when present | emitted when present | Normalized level (see §3.1) |
| `source` | string? | emitted when present | emitted when present | Source identifier (hostname/IP) |
| `service` | string? | emitted when present | emitted when present | Service name |
| `id` | string? | emitted when present | emitted when present | Unique log entry ID (provider-specific; not all backends emit this) |
| `attributes` | `{key: val}` | emitted when non-empty | always emitted | Structured attributes, BTreeMap, sorted by key |
| `resource` | `{key: val}` | **always omitted** | always emitted | Resource-level attributes (DD tags, VL _stream) |
| `trace_id` | string? | emitted when present | emitted when present | Associated Trace ID |
| `span_id` | string? | emitted when present | emitted when present | Associated Span ID |
| `extensions` | `{key: any}?` | omitted | always emitted | Provider-specific information |

---

## 3. Field Mapping and Normalization

### 3.1 Severity Normalization

Unified to 6 levels (uppercase). Values that can't be normalized are preserved as-is:

| Normalized Output | Input Variants |
|-------------------|----------------|
| `TRACE` | `trace`, `TRACE` |
| `DEBUG` | `debug`, `DEBUG`, syslog `7` |
| `INFO` | `info`, `INFO`, `informational`, `notice`, `NOTICE`, syslog `5`, `6` |
| `WARN` | `warn`, `WARN`, `warning`, `WARNING`, syslog `4` |
| `ERROR` | `error`, `ERROR`, `err`, `ERR`, syslog `3` |
| `FATAL` | `fatal`, `FATAL`, `critical`, `CRITICAL`, `crit`, `alert`, `ALERT`, `emergency`, `emerg`, `EMERG`, syslog `0`, `1`, `2` |

> Values that don't match any of the above variants are emitted as-is (e.g., `Other("custom_level")`), without normalization.

**Severity field lookup priority** (shares the same priority chain as message):

1. `--field-mapping severity=<field>` CLI flag (**Not Implemented**)
2. `logstore_overrides.<logstore>.field_mapping.severity` config
3. `field_mapping.severity` context-level config
4. Heuristic: `severity` → `level` → `log.level` → `loglevel` → `log_level` → `status` (DD only)

> **Per-provider implementation**: The heuristic chain above applies primarily to VictoriaLogs. Other providers use fixed field paths:
> - **Datadog**: `status` field directly
> - **SLS**: `severityText` field directly
> - **Loki**: stream labels `severity_text` → `detected_level` → `level`
> - **OpenSearch**: `severity.text` (nested)
> - **Elasticsearch**: `severity_text` (top-level)

### 3.2 Message Extraction Priority

Four priority levels, from highest to lowest:

| Priority | Source | Description |
|----------|--------|-------------|
| 1 | `--field-mapping message=<field>` | Per-query override (**Not Implemented**) |
| 2 | `logstore_overrides.<logstore>.field_mapping.message` | Per-logstore config (SLS-specific, **Not Implemented**) |
| 3 | `field_mapping.message` | Context-level default (**Not Implemented**) |
| 4 | Heuristic | Tries the following field names in order |

**Heuristic candidate fields**: `message` → `content` → `_msg` → `msg` → `body` → `log` → `__raw__`

If none exist, the first non-built-in field with the longest value is used. Final fallback: serialize the entire JSON record as message.

> **Per-provider implementation**: The heuristic chain above is a fallback for providers without fixed message fields. Most providers use a single fixed field:
> - **VictoriaLogs**: `_msg`
> - **Datadog**: `attributes.message`
> - **SLS**: `content`
> - **Loki**: stream value (log line content)
> - **OpenSearch**: `body`
> - **Elasticsearch**: `body.text`

> Each field_mapping layer is a **wholesale override**, not a field-level merge. If logstore_overrides defines message but not severity, severity falls through directly to the heuristic, not back to the context-level config.

### 3.3 Service Extraction

Heuristic: `service` → `app` → `app_name`

> **Per-provider implementation**: The chain `service → app → app_name` applies to VictoriaLogs (which also checks `service.name` first). Other providers use fixed paths:
> - **Datadog**: `attributes.service`
> - **SLS**: `service`
> - **Loki**: stream labels `service_name` → `otelServiceName`
> - **OpenSearch**: `resource.service.name`
> - **Elasticsearch**: `resource.attributes.service.name`

### 3.4 Attributes Flattening

The core `attributes` is a `BTreeMap<String, String>`:
- Primitive types (string/number/bool) → converted to string
- Nested objects/arrays → serialized as JSON string
- Full nested structures are preserved in `extensions` (Full View)

---

## 4. Cross-Platform Conversion Rules

### 4.1 Datadog → obz LogEntry

| Datadog Field | obz Field | Conversion |
|---------------|-----------|------------|
| `data[i].id` | `id` | Direct mapping |
| `data[i].attributes.timestamp` | `timestamp` | ISO8601 → Unix seconds |
| `data[i].attributes.message` | `message` | Direct mapping |
| `data[i].attributes.status` | `severity` | Normalized (§3.1) |
| `data[i].attributes.host` | `source` | Direct mapping |
| `data[i].attributes.service` | `service` | Direct mapping |
| `data[i].attributes.tags[]` | `resource` | `splitn(2, ':')` → BTreeMap; tags without colon → key=value, value=`""` |
| `data[i].attributes.attributes.*` | `attributes` | Flattened (§3.4) |
| `data[i].attributes.attributes.dd.trace_id` | `trace_id` | Direct mapping |
| `data[i].attributes.attributes.dd.span_id` | `span_id` | Direct mapping |
| `meta.page.after` | `metadata.cursor` | Pagination cursor |

**Datadog extensions**: In log entries, `extensions` is always `None` (nested attributes are flattened into dot-notation keys in `attributes`; see §5.2). For spans, extensions include `datadog.indexes`, `datadog.storage_tier`, and other Datadog-specific metadata.

### 4.2 SLS → obz LogEntry

| SLS Field | obz Field | Conversion |
|-----------|-----------|------------|
| `__time__` | `timestamp` | string → i64 |
| Heuristic message field | `message` | §3.2 |
| Heuristic severity field | `severity` | §3.1 |
| `__source__` | `source` | Direct mapping |
| Heuristic service field | `service` | §3.3 |
| All non-`__`-prefixed fields | `attributes` | Flat mapping (excluding message/severity/service fields) |
| `traceId` / `trace_id` | `trace_id` | Extracted from attributes |
| `spanId` / `span_id` | `span_id` | Extracted from attributes |
| `x-log-progress` response header | `metadata.is_complete` | `Complete` → true, `Incomplete` → automatic retry |

**SLS extensions**: `sls.topic` (`__topic__`), `sls.tags` (`__tag__:*` fields), `sls.pack_id`, `sls.meta` (processedRows, elapsedMillisecond, etc.)

### 4.3 VictoriaLogs → obz LogEntry

| VL Field | obz Field | Conversion |
|----------|-----------|------------|
| `_time` | `timestamp` | RFC3339 → Unix seconds |
| `_msg` | `message` | Direct mapping |
| `level` / `severity` / `log.level` | `severity` | Heuristic + normalization |
| `_stream`: `host`/`source` | `source` | Destructure stream, extract host/source |
| `service` / `app` | `service` | Heuristic |
| All `_stream` fields | `resource` | Destructured into BTreeMap |
| All non-system fields (not `_msg`/`_time`/`_stream`/`_stream_id`) | `attributes` | Flat mapping (excluding severity/service fields) |
| `trace_id` / `traceID` | `trace_id` | Extracted from attributes |
| `span_id` / `spanID` | `span_id` | Extracted from attributes |

**VL extensions**: `vl.stream_id` (`_stream_id`), `vl.stream` (original stream string)

### 4.4 Grafana Loki → obz LogEntry

| Loki Field | obz Field | Conversion |
|------------|-----------|------------|
| `stream` labels | `resource` | Stream label set mapped directly to BTreeMap |
| `values[][0]` | `timestamp` | Nanosecond string → Unix seconds (divided by 1_000_000_000) |
| `values[][1]` | `message` | Log line content |
| `stream`: `level`/`severity` | `severity` | Heuristic + normalization |
| `stream`: `host`/`source` | `source` | Heuristic extraction |
| `stream`: `service`/`app` | `service` | Heuristic extraction |
| Parsed structured fields | `attributes` | If the log line is JSON, parsed into attributes |

### 4.5 OpenSearch → obz LogEntry

OpenSearch uses the OTel data model with camelCase fields:

| OpenSearch Field | obz Field | Conversion |
|------------------|-----------|------------|
| `_source.@timestamp` | `timestamp` | ISO 8601 → Unix seconds |
| `_source.body` | `message` | String-typed log body |
| `_source.severity.text` | `severity` | Normalized |
| `_source.resource.*` | `resource` | Flat mapping |
| `_source.attributes.*` | `attributes` | Flat mapping |
| `_source.traceId` | `trace_id` | Direct mapping (camelCase) |
| `_source.spanId` | `span_id` | Direct mapping (camelCase) |

### 4.6 Elasticsearch → obz LogEntry

Elasticsearch also uses the OTel data model, but with snake_case fields and nesting differences:

| Elasticsearch Field | obz Field | Conversion |
|---------------------|-----------|------------|
| `_source.@timestamp` | `timestamp` | epoch_millis → Unix seconds (divided by 1000) |
| `_source.body.text` | `message` | Nested under `body.text` (not a top-level string) |
| `_source.severity_text` | `severity` | Top-level field (not `severity.text`) |
| `_source.resource.attributes.*` | `resource` | Nested under `resource.attributes` |
| `_source.attributes.*` | `attributes` | Flat mapping |
| `_source.trace_id` | `trace_id` | snake_case |
| `_source.span_id` | `span_id` | snake_case |

**Key differences between OpenSearch and Elasticsearch**:

| Difference | OpenSearch | Elasticsearch |
|------------|-----------|---------------|
| Timestamp format | ISO 8601 string | epoch_millis string |
| Field naming | camelCase (`traceId`) | snake_case (`trace_id`) |
| Log body | Top-level `body` (string) | Nested `body.text` |
| Severity | `severity.text` | `severity_text` |
| Resource | Top-level flat map | Nested `resource.attributes` |

---

## 5. Edge Cases

### 5.1 SLS Progress Retry

The SLS GetLogs API may return `x-log-progress: Incomplete`:

- Automatic retry, up to 10 times, interval 200ms → 400ms → 800ms → 1600ms → 3200ms (capped). Formula: `200 * 2^min(retry_count, 4)` ms.
- **Each retry replaces the previous result** (not cumulative). The server processes more data on retry and returns a more complete result set.
- `-v` mode prints progress
- `metadata.is_complete` reflects the final state

### 5.2 Datadog Nested Attributes

```json
// Datadog original
{"attributes": {"user": {"id": 123, "roles": ["admin"]}}}

// obz LogEntry
{
  "attributes": {"user.id": "123", "user.roles": "[\"admin\"]"}
}
```

In log entries, nested objects within `attributes` are recursively flattened into dot-notation string keys (e.g. `user.id`, `user.roles`). The original nested structure is **not** preserved in `extensions` for log entries. `extensions` is always `None` in the current implementation. For spans, Datadog-specific metadata is preserved in `extensions` (see trace-model.md).

### 5.3 VictoriaLogs NDJSON Parsing

VictoriaLogs returns NDJSON (one JSON object per line), and the adapter parses line by line.

### 5.4 Timestamp Unification

| Source | Raw Format | Conversion |
|--------|------------|------------|
| Datadog | ISO8601 `"2019-01-02T09:42:36.320Z"` | parse → Unix seconds (truncated to seconds) |
| SLS | string `"1649902984"` | parse i64 |
| VictoriaLogs | RFC3339 `"2023-01-01T13:32:13Z"` | parse → Unix seconds |
| Loki | Nanosecond string `"1687266733320000000"` | parse i64 → divided by 1_000_000_000 |
| OpenSearch | ISO 8601 `"2023-01-01T13:32:13Z"` | parse → Unix seconds |
| Elasticsearch | epoch_millis `"1687266733320"` | parse i64 → divided by 1000 |

### 5.5 Large Result Set Pagination

| Platform | Max Per Request | Pagination Method |
|----------|-----------------|-------------------|
| Datadog | `page.limit` max 1000 | Cursor-based, CLI auto-paginates |
| SLS | `line` max 100 | offset + line, CLI auto-paginates |
| VictoriaLogs | `limit` parameter (no hard cap) | limit + offset |
| Loki | `limit` parameter | Single request |
| OpenSearch | `size` parameter | size/from |
| Elasticsearch | `size` parameter | size/from |

The CLI's `--limit` controls the final count returned to the user; adapters handle multiple requests internally.

### 5.6 Truncation Completeness Signal (`metadata.is_complete`)

`is_complete` indicates whether the returned result set contains all matching data. Its type is `Option<bool>` with three-state semantics:

| Value | Meaning |
|-------|---------|
| `true` | The backend confirms results are complete; all matching data has been returned |
| `false` | The backend confirms results are incomplete; more matching data exists |
| omitted (`null`) | The backend doesn't provide a reliable signal; the agent can infer from `total_count == limit` |

Per-platform behavior:

| Platform | `is_complete` Source | Description |
|----------|----------------------|-------------|
| ES / OpenSearch | `hits.total.value` + `hits.total.relation` | When `relation == "eq"`, exact comparison `hits.len() >= total`; when `relation == "gte"`, total is a lower bound, reports `false` |
| SLS | `x-log-progress` response header | `Complete` → `true`, still `Incomplete` after retries → `false` |
| Datadog | `meta.page.after` cursor | No cursor → `true`, has cursor → `false` |
| Loki | omitted | Loki API doesn't provide a truncation indicator |
| VictoriaLogs | omitted | VictoriaLogs API doesn't provide a truncation indicator |
