# obz Trace Data Model Specification

> Final conclusions and interface specifications only.
> No decision rationale, no Rust structs.
> Currently supports 7 backends: VictoriaTraces, Jaeger, Grafana Tempo, OpenSearch, Elasticsearch, Datadog, and SLS.
>
> **Implementation status**: The `--view` flag is not yet implemented. The current version outputs all fields (equivalent to Full View).
> The Agent View / Full View differences described below are planned behavior.
> `--fields` and `--truncate` are implemented: field projection on `spans[]` and truncation of retained string values are supported; `trace_detail` top-level summary fields are not affected by `--fields`. See [`output-projection.md`](./output-projection.md) for details.

---

## 1. Output Examples

### 1.1 Agent View — trace search (span search)

```json
{
  "status": "success",
  "metadata": {
    "provider": "prod-dd",
    "total_count": 3
  },
  "data": {
    "result_type": "spans",
    "spans": [
      {
        "trace_id": "abc123def456789012345678",
        "span_id": "def4567890123456",
        "parent_span_id": "aabb112233445566",
        "name": "HTTP GET /api/users",
        "service": "api-gateway",
        "kind": "server",
        "status": "ok",
        "start_time": 1711234800,
        "duration_us": 45230,
        "attributes": {
          "http.method": "GET",
          "http.status_code": "200",
          "http.url": "https://api.example.com/users"
        }
      },
      {
        "trace_id": "abc123def456789012345678",
        "span_id": "1234abcd5678ef90",
        "parent_span_id": "def4567890123456",
        "name": "SELECT * FROM users",
        "service": "user-service",
        "kind": "client",
        "status": "ok",
        "start_time": 1711234800,
        "duration_us": 12300,
        "attributes": {
          "db.system": "postgresql",
          "db.statement": "SELECT * FROM users WHERE id = ?"
        }
      },
      {
        "trace_id": "ff00112233445566aabbccdd",
        "span_id": "9988776655443322",
        "name": "POST /api/orders",
        "service": "order-service",
        "kind": "server",
        "status": "error",
        "start_time": 1711234795,
        "duration_us": 1523700,
        "attributes": {
          "http.method": "POST",
          "http.status_code": "500",
          "error.message": "Connection refused"
        }
      }
    ]
  }
}
```

### 1.2 Agent View — trace get (fetch full trace by traceID)

```json
{
  "status": "success",
  "metadata": {
    "provider": "prod-dd",
    "total_count": 4
  },
  "data": {
    "result_type": "trace_detail",
    "trace_id": "abc123def456789012345678",
    "span_count": 4,
    "service_count": 3,
    "duration_us": 245500,
    "services": ["api-gateway", "user-service", "postgres"],
    "spans": [
      {
        "trace_id": "abc123def456789012345678",
        "span_id": "root001aabbccddee",
        "name": "HTTP GET /api/users",
        "service": "api-gateway",
        "kind": "server",
        "status": "ok",
        "start_time": 1711234800,
        "duration_us": 245500,
        "attributes": {
          "http.method": "GET",
          "http.status_code": "200"
        }
      },
      {
        "trace_id": "abc123def456789012345678",
        "span_id": "child002ffeeddcc",
        "parent_span_id": "root001aabbccddee",
        "name": "user.getById",
        "service": "user-service",
        "kind": "internal",
        "status": "ok",
        "start_time": 1711234800,
        "duration_us": 50200,
        "attributes": {}
      },
      {
        "trace_id": "abc123def456789012345678",
        "span_id": "child003aabb1122",
        "parent_span_id": "child002ffeeddcc",
        "name": "SELECT * FROM users WHERE id = ?",
        "service": "user-service",
        "kind": "client",
        "status": "ok",
        "start_time": 1711234800,
        "duration_us": 12300,
        "attributes": {
          "db.system": "postgresql"
        }
      },
      {
        "trace_id": "abc123def456789012345678",
        "span_id": "child004ccdd3344",
        "parent_span_id": "root001aabbccddee",
        "name": "redis.GET user:123",
        "service": "api-gateway",
        "kind": "client",
        "status": "ok",
        "start_time": 1711234800,
        "duration_us": 2100,
        "attributes": {
          "db.system": "redis"
        }
      }
    ]
  }
}
```

> `trace_detail` has additional top-level summary fields compared to `spans`: `trace_id`, `span_count`, `service_count`, `duration_us`, `services`. These fields let agents get a trace overview without iterating through all spans.

### 1.3 Full View (`--view full`)

```json
{
  "status": "success",
  "metadata": {
    "provider": "prod-dd",
    "provider_type": "datadog",
    "query_language": "Span Search Syntax",
    "query": "service:api-gateway @http.status_code:500",
    "time_range": {"start": 1711231200, "end": 1711234800},
    "total_count": 3,
    "is_complete": true,
    "cursor": "eyJhZnRlciI6IkFRQUFBR..."
  },
  "data": {
    "result_type": "spans",
    "spans": [
      {
        "trace_id": "abc123def456789012345678",
        "span_id": "def4567890123456",
        "parent_span_id": "aabb112233445566",
        "name": "HTTP GET /api/users",
        "service": "api-gateway",
        "kind": "server",
        "status": "ok",
        "start_time": 1711234800,
        "duration_us": 45230,
        "attributes": {
          "http.method": "GET",
          "http.status_code": "200",
          "http.url": "https://api.example.com/users"
        },
        "events": [],
        "resource": {
          "hostname": "web01",
          "env": "prod",
          "version": "1.2.3"
        },
        "extensions": {
          "datadog.type": "web",
          "datadog.meta": {
            "runtime-id": "abc-123"
          }
        }
      }
    ]
  }
}
```

> Full View adds the following compared to Agent View: `metadata.provider_type`/`query_language`/`query`/`time_range`/`is_complete`/`cursor`, and per-span `events`/`resource`/`extensions`.

### 1.4 Table View (`-o table`)

**trace search**:

```
TRACE_ID                          SERVICE         NAME                          STATUS  DURATION_MS  START_TIME
abc123def456789012345678          api-gateway     HTTP GET /api/users           ok      45.23        2024-03-24 10:00:00
abc123def456789012345678          user-service    SELECT * FROM users           ok      12.30        2024-03-24 10:00:00
ff00112233445566aabbccdd          order-service   POST /api/orders              error   1523.70      2024-03-24 09:59:55
```

**trace get**:

```
Trace: abc123def456789012345678  |  4 spans  |  3 services  |  245.5 ms

SPAN_ID           PARENT            SERVICE         NAME                          STATUS  DURATION_MS
root001aabbcc…    —                 api-gateway     HTTP GET /api/users           ok      245.50
child002ffeedd…   root001aabbcc…    user-service    user.getById                  ok      50.20
child003aabb11…   child002ffeedd…   user-service    SELECT * FROM users WHERE…    ok      12.30
child004ccdd33…   root001aabbcc…    api-gateway     redis.GET user:123            ok      2.10
```

> In table mode, trace_id and span_id are truncated, and attributes are not shown.

---

## 2. Core Structures

### 2.1 Span — a single span

| Field | Type | Agent View | Full View | Description |
|-------|------|:----------:|:---------:|-------------|
| `trace_id` | string | required | required | Hex format |
| `span_id` | string | required | required | Hex format |
| `parent_span_id` | string? | present when set | present when set | Omitted for root spans |
| `name` | string | required | required | Operation name (Jaeger `operationName`, DD `operation_name`) |
| `service` | string | required | required | Service name |
| `kind` | string? | present when set | present when set | Normalized SpanKind (see §3.1) |
| `status` | string | required | required | `ok` / `error` / `unset` |
| `start_time` | int64 | required | required | Unix seconds |
| `duration_us` | i64 | required | required | Duration in microseconds |
| `attributes` | `{key: string}` | present when non-empty | always present | Flattened to strings (consistent with log) |
| `events` | `[SpanEvent]?` | **omitted** | present when set | Span Events (see §2.2) |
| `resource` | `{key: string}?` | **omitted** | always present | Resource attributes (hostname, env, etc.) |
| `extensions` | `{key: any}?` | **omitted** | always present | Provider-specific information |

### 2.2 SpanEvent — event within a span

| Field | Type | Description |
|-------|------|-------------|
| `name` | string | Event name |
| `timestamp` | int64 | Unix seconds |
| `attributes` | `{key: string}?` | Event attributes |

> Datadog does not support span events. Jaeger/Tempo span events are supported.

### 2.3 TraceDetail — top-level summary for trace get responses

When `result_type: "trace_detail"`, `data` contains these summary fields in addition to `spans`:

| Field | Type | Description |
|-------|------|-------------|
| `trace_id` | string | The queried traceID |
| `span_count` | int | Total span count for this trace |
| `service_count` | int | Number of services involved |
| `duration_us` | i64 | End-to-end duration (root span, or earliest start to latest end) |
| `services` | `[string]` | List of services involved (deduplicated, alphabetical) |

These summary fields are computed by the obz CLI and don't depend on the backend API. Agents can use these fields directly for decision-making without iterating through the spans array.

### 2.4 ID Format

| Field | Length | Format | Description |
|-------|--------|--------|-------------|
| `trace_id` | 16 or 32 hex chars | lowercase hex | OTel 128-bit = 32 chars; Jaeger/Zipkin compatible 64-bit = 16 chars |
| `span_id` | 16 hex chars | lowercase hex | 64-bit |

Datadog: `trace_id` is hex from the API; `span_id`/`parent_id` are decimal strings, passed through as-is by design. No decimal-to-hex conversion is performed because parent-child relationships are resolved within the same provider using the original format, and obz does not perform cross-provider span joins.

---

## 3. Field Mapping and Normalization

### 3.1 kind Normalization

Unified to the 5 OTel-defined SpanKind values (lowercase). Values that can't be normalized are preserved as-is:

| Normalized Output | Description | Datadog Mapping |
|-------------------|-------------|-----------------|
| `client` | Initiator of a synchronous remote call | `custom.span.kind = "client"` |
| `server` | Receiver of a synchronous remote call | `custom.span.kind = "server"` |
| `producer` | Sender of an asynchronous message | `custom.span.kind = "producer"` |
| `consumer` | Receiver of an asynchronous message | `custom.span.kind = "consumer"` |
| `internal` | In-process operation | `custom.span.kind = "internal"` |

> Datadog: kind is extracted from `custom.span.kind` in the span's custom attributes (OTel-instrumented spans). The Datadog `type` field (`web`/`http`/`db`/`cache`/`custom`) is preserved in `extensions["datadog.type"]` but not used for kind normalization. Spans without `custom.span.kind` have `kind: null`.
> VictoriaTraces (Jaeger API) kinds are already OTel standard values and map directly.
> SLS trace extracts kind from log fields.

### 3.2 status Normalization

Unified to 3 statuses (lowercase), aligned with OTel `StatusCode`:

| Normalized Output | Description | Datadog Mapping |
|-------------------|-------------|-----------------|
| `ok` | Normal | `attrs.status == "ok"` |
| `error` | Error | `attrs.status == "error"` |
| `unset` | Not set | anything else or missing |

> Jaeger/Zipkin backends determine status via the `error` tag; Tempo/OTel has `status.code` directly.

### 3.3 attributes Flattening

Consistent with log-model.md §3.4:
- Primitive types (string/number/bool) → converted to string
- Nested objects/arrays → serialized as JSON string
- Full nested structures are preserved in `extensions` (Full View)

### 3.4 service Extraction

| Backend | Source |
|---------|--------|
| Datadog | `attributes.service` |
| VT / Jaeger | `processes.{processID}.serviceName` |
| Tempo | OTel `resource.service.name` |
| OpenSearch | `resource.service.name` (camelCase structure) |
| Elasticsearch | `resource.attributes.service.name` (nested structure) |
| SLS | Heuristically extracted from attributes |

### 3.5 name Extraction

| Backend | Source |
|---------|--------|
| Datadog | `attributes.operation_name` |
| VT / Jaeger | `operationName` |
| Tempo | OTel `name` |
| OpenSearch | `name` |
| Elasticsearch | `name` |
| SLS | Heuristically extracted from attributes |

---

## 4. Provider → obz Conversion Rules

### 4.0 VictoriaTraces / Jaeger (Jaeger API) → obz Conversion

VictoriaTraces and Jaeger share the `jaegerapi` module; conversion rules are identical. The only differences are API path prefixes and some request parameters.

| Jaeger Field | obz Field | Conversion |
|-------------|-----------|------------|
| `traceID` | `trace_id` | Direct mapping (already hex) |
| `spanID` | `span_id` | Direct mapping |
| `references[0].spanID` (CHILD_OF) | `parent_span_id` | Direct mapping; no reference → omitted |
| `operationName` | `name` | Direct mapping |
| `processes.{pID}.serviceName` | `service` | Resolved via processID |
| `tags["span.kind"]` | `kind` | Direct mapping (already OTel standard values) |
| `tags["error"]=true` | `status` | `true`→`"error"`, otherwise→`"ok"` |
| `startTime` | `start_time` | Microseconds → Unix seconds (÷ 1,000,000) |
| `duration` | `duration_us` | Direct mapping (Jaeger native microseconds) |
| `tags` | `attributes` | Flat key=value mapping, excluding system tags |

VictoriaTraces and Jaeger both support two extension commands:
- `trace services`: VT uses `GET /select/jaeger/api/services`, Jaeger uses `GET /api/services`
- `trace operations --service <name>`: VT uses `GET /select/jaeger/api/services/{service}/operations`, Jaeger uses `GET /api/services/{service}/operations`

**VT vs Jaeger differences**:
- VT API path prefix is `/select/jaeger/api/`, Jaeger uses `/api/`
- VT `trace get` passes `start`/`end` parameters, Jaeger does not
- Jaeger `trace search` requires `-q` to be a service name (mandatory, cannot be empty)

### 4.1 Datadog → obz Conversion

### 4.2 Datadog Span Field Mapping

| Datadog Field | obz Field | Conversion |
|---------------|-----------|------------|
| `attributes.trace_id` | `trace_id` | Direct mapping (hex string from API) |
| `attributes.span_id` | `span_id` | Direct mapping (decimal string, passed through as-is) |
| `attributes.parent_id` | `parent_span_id` | Direct mapping; `"0"` → omitted |
| `attributes.operation_name` | `name` | Direct mapping |
| `attributes.service` | `service` | Direct mapping |
| `custom.span.kind` | `kind` | OTel SpanKind value from custom attributes (§3.1) |
| `attributes.start_timestamp` | `start_time` | ISO 8601 → Unix seconds |
| `custom.duration` or timestamps | `duration_us` | `custom.duration / 1000` (ns→us), or `(end_ms - start_ms) * 1000` (§5.3) |
| `attributes.status` | `status` | `"error"`→`error`, `"ok"`→`ok`, else→`unset` |
| `custom.*` | `attributes` | Flattened to `{key: string}` |
| — | `events` | Datadog has no events, Full View shows `[]` |

### 4.3 Datadog extensions

Provider-specific information preserved in Full View:

| Field | Description |
|-------|-------------|
| `datadog.type` | Datadog original span type (`web`/`http`/`db`/`cache`/`custom`) |
| `datadog.resource_name` | Datadog resource name (e.g. `"GET /api/users"`) |
| `datadog.env` | Datadog environment (e.g. `"prod"`, `"staging"`) |

> **Note**: In the current implementation, only `datadog.type`, `datadog.resource_name`, and `datadog.env` are populated. `datadog.meta` and `datadog.metrics` are reserved keys for future use.

### 4.4 trace get Datadog Implementation

Datadog has no dedicated "fetch full trace by traceID" API. The obz `trace get <trace_id>` command is implemented as:

```
POST /api/v2/spans/events/search
{
  "data": {
    "type": "search_request",
    "attributes": {
      "filter": {
        "query": "trace_id:<trace_id>",
        "from": "now-15m",
        "to": "now"
      },
      "sort": "timestamp",
      "page": {"limit": 1000}
    }
  }
}
```

The obz CLI then computes the `trace_detail` summary fields (`span_count`, `service_count`, `duration_us`, `services`).

> Note: `trace get` defaults to searching spans within the last 15 minutes. For a wider time range, use `--from`/`--to`.

### 4.5 Grafana Tempo → obz Conversion

Tempo returns OTLP protobuf-JSON format (`OtlpTraceResponse`), which obz converts to the unified Span model:

| Tempo (OTLP) Field | obz Field | Conversion |
|---------------------|-----------|------------|
| `resourceSpans[].resource.attributes[].key/value` | `resource` | Flattened key-value array |
| `resourceSpans[].resource.attributes["service.name"]` | `service` | Extracted from resource |
| `scopeSpans[].spans[].traceId` | `trace_id` | base64 or hex → standard hex |
| `scopeSpans[].spans[].spanId` | `span_id` | base64 or hex → standard hex |
| `scopeSpans[].spans[].parentSpanId` | `parent_span_id` | Same as above; empty or all-zeros → omitted |
| `scopeSpans[].spans[].name` | `name` | Direct mapping |
| `scopeSpans[].spans[].kind` | `kind` | OTel SpanKind enum → standard value |
| `scopeSpans[].spans[].status.code` | `status` | `STATUS_CODE_OK`→`ok`, `STATUS_CODE_ERROR`→`error`, others→`unset` |
| `scopeSpans[].spans[].startTimeUnixNano` | `start_time` | Nanoseconds → Unix seconds |
| `scopeSpans[].spans[].endTimeUnixNano` - `startTimeUnixNano` | `duration_us` | Nanosecond difference → microseconds |
| `scopeSpans[].spans[].attributes[]` | `attributes` | Flattened key-value array |
| `scopeSpans[].spans[].events[]` | `events` | Mapped to SpanEvent |

Tempo also supports two extension commands:
- `trace tags`: `GET /api/v2/search/tags`, returns tag names grouped by scope
- `trace tag-values --tag <name>`: `GET /api/v2/search/tag/{tag}/values`

### 4.6 OpenSearch → obz Span Conversion

OpenSearch uses the OTel data model (camelCase field names):

| OpenSearch Field | obz Field | Conversion |
|-----------------|-----------|------------|
| `_source.traceId` | `trace_id` | Direct mapping (camelCase) |
| `_source.spanId` | `span_id` | Direct mapping |
| `_source.parentSpanId` | `parent_span_id` | Direct mapping |
| `_source.name` | `name` | Direct mapping |
| `_source.resource.service.name` | `service` | Extracted from resource |
| `_source.kind` | `kind` | Normalized |
| `_source.status.code` | `status` | Normalized |
| `_source.startTime` | `start_time` | ISO 8601 → Unix seconds |
| `_source.endTime` - `_source.startTime` | `duration_us` | Time difference → microseconds |
| `_source.attributes.*` | `attributes` | Flat mapping |
| `_source.resource.*` | `resource` | Flat mapping |

### 4.7 Elasticsearch → obz Span Conversion

Elasticsearch also uses the OTel data model, but with snake_case fields:

| Elasticsearch Field | obz Field | Conversion |
|--------------------|-----------|------------|
| `_source.trace_id` | `trace_id` | snake_case |
| `_source.span_id` | `span_id` | snake_case |
| `_source.parent_span_id` | `parent_span_id` | snake_case |
| `_source.name` | `name` | Direct mapping |
| `_source.resource.attributes.service.name` | `service` | Nested under `resource.attributes` |
| `_source.kind` | `kind` | Normalized |
| `_source.status.code` | `status` | `Unset` represented as `{}` (empty object) |
| `_source.@timestamp` | `start_time` | epoch_millis → Unix seconds |
| `_source.duration` | `duration_us` | Nanoseconds → microseconds (÷ 1000); explicit field, not computed |
| `_source.attributes.*` | `attributes` | Flat mapping |
| `_source.resource.attributes.*` | `resource` | Nested under `resource.attributes` |

### 4.8 SLS → obz Span Conversion

SLS uses the GetLogs API; span data is stored in a logstore:

| SLS Field | obz Field | Conversion |
|-----------|-----------|------------|
| `traceID` / `trace_id` | `trace_id` | Extracted from attributes |
| `spanID` / `span_id` | `span_id` | Extracted from attributes |
| `parentSpanID` / `parent_span_id` | `parent_span_id` | Extracted from attributes |
| Heuristic name field | `name` | Heuristically extracted |
| Heuristic service field | `service` | Heuristically extracted |
| `__time__` | `start_time` | string → i64 |
| `duration` | `duration_us` | Extracted from attributes and converted |
| All non-system fields | `attributes` | Flat mapping |

`trace get` is implemented by querying `traceID:{trace_id}`, returning up to 1000 spans.
SLS progress retry follows the same mechanism as log search (up to 10 retries with exponential backoff).

---

## 5. Edge Cases

### 5.1 Datadog ID Handling

Datadog `trace_id` is a hex string from the API. `span_id` and `parent_id` are decimal strings, passed through as-is by design. Converting to hex would break parent-child span matching (both sides must use the same format), and obz does not perform cross-provider span joins where format normalization would matter.

### 5.2 Datadog parent_id = "0"

Datadog root spans have `parent_id` set to `"0"`. In obz, the `parent_span_id` field is **omitted** (no `"parent_span_id": "0000000000000000"` in the output).

### 5.3 Datadog Duration Calculation

Datadog duration is computed with two fallback paths:

1. If `custom.duration` (nanoseconds) is present in the span's custom attributes:
   ```
   duration_us = custom.duration / 1_000
   ```

2. Otherwise, from ISO 8601 timestamps:
   ```
   duration_us = (end_timestamp_ms - start_timestamp_ms) * 1_000
   ```

The first path is preferred because `custom.duration` provides nanosecond precision from OTel-instrumented spans.

### 5.4 Timestamp Unification

| Source | Original Format | Conversion |
|--------|----------------|------------|
| Datadog | ISO 8601 string | parse → Unix seconds |
| VT / Jaeger | Microsecond int64 | ÷ 1,000,000 → Unix seconds |
| Tempo / OTel | Nanosecond int64 | ÷ 1,000,000,000 → Unix seconds |
| OpenSearch | ISO 8601 string | parse → Unix seconds |
| Elasticsearch | epoch_millis | ÷ 1,000 → Unix seconds |
| SLS | Second string | parse i64 |

> Consistent with metric/log, `start_time` is always Unix seconds. Sub-second precision is preserved in `duration_us` as an integer in microseconds.

### 5.5 Large Result Set Pagination

| Platform | Max Per Request | Pagination Method |
|----------|-----------------|-------------------|
| Datadog | `page.limit` max 1000 | cursor-based (`meta.page.after`), CLI auto-paginates |
| VT / Jaeger | `limit` parameter (default 20) | Single request returns full trace |
| Tempo | `limit` parameter | Single request returns |
| OpenSearch | `size` parameter (`trace get` max 1000) | size/from |
| Elasticsearch | `size` parameter (`trace get` max 1000) | size/from |
| SLS | `line` parameter (`trace get` max 1000) | offset + line, progress retry |

The CLI's `--limit` controls the final number of spans returned to the user; the adapter handles multiple internal requests automatically.

### 5.6 Truncation Completeness Signal (`metadata.is_complete`)

The `is_complete` semantics for `trace search` are the same as for `log search` (see [log-model.md §5.6](log-model.md#56-truncation-completeness-signal-metadatais_complete)). Type is `Option<bool>` with three-state semantics: `true` (complete), `false` (incomplete), omitted (backend provides no signal).

Per-platform behavior:

| Platform | `is_complete` Source | Description |
|----------|---------------------|-------------|
| ES / OpenSearch | `hits.total.value` + `hits.total.relation` | Same as log search |
| SLS | `x-log-progress` response header | Progress signal is no longer discarded |
| Datadog | `meta.page.after` cursor | No cursor → `true`, has cursor → `false` |
| VT / Jaeger / Tempo | omitted | Backend does not provide a truncation indicator |

### 5.7 Span Ordering in trace get

Spans in `trace_detail` are sorted by `start_time` ascending (ties broken by `duration_us` descending), ensuring the call chain reads top-to-bottom.

---

## 6. result_type Summary

| result_type | Command | Data fields in `data` |
|-------------|---------|----------------------|
| `spans` | `trace search` | `data.spans: [Span, ...]` |
| `trace_detail` | `trace get` | `data.trace_id`, `data.span_count`, `data.service_count`, `data.duration_us`, `data.services`, `data.spans: [Span, ...]` |
