//! Elasticsearch API response types.
//!
//! These types model the Elasticsearch `_search` API response format for both
//! log and trace indices using the native `OTel` integration.
//!
//! # Key differences from `OpenSearch`
//!
//! | Aspect              | OpenSearch                    | Elasticsearch                    |
//! |---------------------|-------------------------------|----------------------------------|
//! | Timestamp format    | RFC 3339                      | epoch_millis string              |
//! | Field naming        | camelCase                     | snake_case                       |
//! | Log body            | `body` (string)               | `body.text` (nested object)      |
//! | Severity            | `severity.text` (nested)      | `severity_text` (top-level)      |
//! | Resource            | flat map                      | nested `resource.attributes`     |
//! | Span duration       | computed from start/end time  | explicit `duration` (nanoseconds)|
//! | Span status Unset   | `code: "Unset"`               | `{}` (empty object)              |

use std::collections::BTreeMap;

use serde::Deserialize;

// ---------------------------------------------------------------------------
// Common search response envelope
// ---------------------------------------------------------------------------

/// Top-level Elasticsearch search response.
///
/// The envelope structure (`hits.total`, `hits.hits[]._source`) is identical
/// to `OpenSearch`, so we use the same shape.
#[derive(Debug, Deserialize)]
pub(crate) struct EsSearchResponse {
    pub hits: EsHits,
}

/// The `hits` wrapper containing total count and hit documents.
#[derive(Debug, Deserialize)]
pub(crate) struct EsHits {
    pub total: EsTotal,
    pub hits: Vec<EsHit>,
}

/// Total hit count with relation (`eq` or `gte`).
#[derive(Debug, Deserialize)]
pub(crate) struct EsTotal {
    pub value: u64,
    /// `"eq"` for exact count, `"gte"` when the count is a lower bound.
    pub relation: String,
}

/// A single hit document.
#[derive(Debug, Deserialize)]
pub(crate) struct EsHit {
    #[allow(dead_code)]
    #[serde(rename = "_index")]
    pub index: String,
    #[serde(rename = "_id")]
    pub id: String,
    #[serde(rename = "_source")]
    pub source: serde_json::Value,
}

// ---------------------------------------------------------------------------
// Log document (`_source` for OTel log data streams)
// ---------------------------------------------------------------------------

/// `OTel` log document stored in Elasticsearch via native `OTel` integration.
///
/// Example index/data stream: `logs-generic.otel-default`.
///
/// Key differences from `OpenSearch`:
/// - `body` is `{ "text": "..." }` (nested), not a plain string.
/// - `severity_text` / `severity_number` are top-level (not nested in `severity`).
/// - `resource` is `{ "attributes": { ... }, "schema_url": "..." }` (nested).
/// - `@timestamp` is `epoch_millis` string like `"1775552455282.107378"`.
/// - Field names use `snake_case` (`trace_id`, `span_id`).
#[derive(Debug, Deserialize)]
pub(crate) struct EsLogDocument {
    /// Primary timestamp — epoch milliseconds as a string (e.g., `"1775552455282.107378"`).
    #[serde(rename = "@timestamp")]
    pub timestamp: Option<String>,

    /// Observed timestamp (when the log was collected), same format.
    pub observed_timestamp: Option<String>,

    /// Log message body — nested object with `text` field.
    pub body: Option<EsLogBody>,

    /// Severity text (e.g., `"INFO"`, `"ERROR"`). Top-level field.
    pub severity_text: Option<String>,

    /// Severity number (e.g., `9` for INFO, `17` for ERROR). Top-level field.
    pub severity_number: Option<u32>,

    /// Event name (e.g., `"exception"`). Top-level field.
    #[allow(dead_code)]
    pub event_name: Option<String>,

    /// Trace correlation — trace ID (`snake_case`).
    pub trace_id: Option<String>,

    /// Trace correlation — span ID (`snake_case`).
    pub span_id: Option<String>,

    /// Resource metadata with nested attributes.
    pub resource: Option<EsResource>,

    /// Span/log attributes (flat map).
    #[serde(default)]
    pub attributes: BTreeMap<String, serde_json::Value>,
}

/// Log body wrapper — Elasticsearch stores the body as `{ "text": "..." }`.
#[derive(Debug, Deserialize)]
pub(crate) struct EsLogBody {
    /// The actual log message text.
    pub text: Option<String>,
}

// ---------------------------------------------------------------------------
// Trace/span document (`_source` for OTel trace data streams)
// ---------------------------------------------------------------------------

/// `OTel` span document stored in Elasticsearch via native `OTel` integration.
///
/// Example index/data stream: `traces-generic.otel-default`.
///
/// Key differences from `OpenSearch`:
/// - No `startTime`/`endTime` — uses `@timestamp` (`epoch_millis`) + `duration` (nanoseconds).
/// - Field names are `snake_case` (`trace_id`, `span_id`, `parent_span_id`).
/// - `status` can be `{}` (empty object) for Unset, not `{ "code": "Unset" }`.
/// - `resource` is nested with `attributes` sub-object.
#[derive(Debug, Deserialize)]
pub(crate) struct EsSpanDocument {
    /// Span start time — epoch milliseconds as a string.
    #[serde(rename = "@timestamp")]
    pub timestamp: Option<String>,

    /// Trace ID (32-char hex, `snake_case`).
    pub trace_id: String,

    /// Span ID (16-char hex, `snake_case`).
    pub span_id: String,

    /// Parent span ID (`snake_case`). Absent for root spans.
    pub parent_span_id: Option<String>,

    /// Operation name.
    #[serde(default)]
    pub name: String,

    /// Span kind: `"Client"`, `"Server"`, `"Internal"`, `"Producer"`, `"Consumer"`.
    #[serde(default)]
    pub kind: String,

    /// Span duration in **nanoseconds**.
    pub duration: Option<i64>,

    /// Span status. Can be `{}` (empty) for Unset.
    pub status: Option<EsSpanStatus>,

    /// Resource metadata with nested attributes.
    pub resource: Option<EsResource>,

    /// Span attributes (flat map).
    #[serde(default)]
    pub attributes: BTreeMap<String, serde_json::Value>,
}

/// Span status with optional code and message.
///
/// Elasticsearch may return `{}` for Unset status, so both fields are optional.
#[derive(Debug, Deserialize)]
pub(crate) struct EsSpanStatus {
    /// `"Ok"`, `"Error"`, or absent (Unset).
    pub code: Option<String>,
    /// Error message (present when status is Error).
    #[serde(default)]
    pub message: String,
}

// ---------------------------------------------------------------------------
// Shared nested types
// ---------------------------------------------------------------------------

/// Resource metadata — Elasticsearch wraps resource attributes in a nested object.
///
/// ```json
/// {
///   "schema_url": "https://opentelemetry.io/schemas/1.39.0",
///   "attributes": {
///     "service.name": "load-generator",
///     "host.name": "demo-host-01"
///   }
/// }
/// ```
#[derive(Debug, Deserialize)]
pub(crate) struct EsResource {
    /// Resource-level attributes (e.g., `service.name`, `host.name`).
    #[serde(default)]
    pub attributes: BTreeMap<String, serde_json::Value>,
}

// ---------------------------------------------------------------------------
// Error response
// ---------------------------------------------------------------------------

/// Elasticsearch error response body (HTTP 4xx/5xx).
///
/// The error format is identical to `OpenSearch`.
#[derive(Debug, Deserialize)]
pub(crate) struct EsErrorResponse {
    pub error: EsErrorDetail,
    #[allow(dead_code)]
    pub status: u32,
}

/// Error detail with type and reason.
#[derive(Debug, Deserialize)]
pub(crate) struct EsErrorDetail {
    #[serde(rename = "type")]
    pub error_type: String,
    pub reason: String,
}
