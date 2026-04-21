//! Datadog API response deserialization types.
//!
//! These types map directly to the Datadog REST API JSON responses.
//! They are intentionally kept separate from obz-core models — conversion
//! happens in [`super::convert`].

use serde::Deserialize;

// ---------------------------------------------------------------------------
// Common
// ---------------------------------------------------------------------------

/// Error response returned by Datadog for 401/403/etc.
///
/// ```json
/// { "errors": ["Unauthorized"] }
/// ```
#[derive(Debug, Deserialize)]
pub(crate) struct DdErrorResponse {
    pub errors: Vec<String>,
}

// ---------------------------------------------------------------------------
// Metric Query  — GET /api/v1/query
// ---------------------------------------------------------------------------

/// Top-level response from `GET /api/v1/query`.
///
/// Note: Datadog may return HTTP 200 with `status: "error"` for invalid
/// queries.  The `error` field contains the error message in that case.
#[derive(Debug, Deserialize)]
pub(crate) struct DdMetricQueryResponse {
    pub status: String,
    #[serde(default)]
    pub series: Vec<DdSeries>,
    /// Present when `status == "error"`.
    pub error: Option<String>,
}

/// A single time series in a metric query response.
#[derive(Debug, Deserialize)]
pub(crate) struct DdSeries {
    /// Metric name (e.g. `system.cpu.idle`).
    pub metric: String,
    /// Data points as `[[timestamp_ms, value], ...]`.
    #[serde(default)]
    pub pointlist: Vec<DdPoint>,
    /// Aggregation interval in seconds.
    pub interval: Option<i64>,
    /// Scope string (e.g. `"host:foo,env:test"`).
    pub scope: Option<String>,
    /// Tag set as individual `"key:value"` strings.
    #[serde(default)]
    pub tag_set: Vec<String>,
    /// Unit information (can be `null` or `[{...}, null]`).
    /// Kept for future use (e.g. unit display in output formatting).
    #[allow(dead_code)]
    pub unit: Option<serde_json::Value>,
}

/// A single data point in a Datadog metric series.
///
/// Datadog returns `[timestamp_ms, value]` where timestamp is in
/// **milliseconds** since epoch.  We deserialize into a tuple and
/// convert to seconds in the conversion layer.
#[derive(Debug, Deserialize)]
pub(crate) struct DdPoint(pub f64, pub Option<f64>);

// ---------------------------------------------------------------------------
// Metric Search  — GET /api/v1/search
// ---------------------------------------------------------------------------

/// Response from `GET /api/v1/search?q=metrics:{prefix}`.
#[derive(Debug, Deserialize)]
pub(crate) struct DdSearchResponse {
    pub results: DdSearchResults,
}

/// Inner results of a metric search response.
#[derive(Debug, Deserialize)]
pub(crate) struct DdSearchResults {
    #[serde(default)]
    pub metrics: Vec<String>,
}

// ---------------------------------------------------------------------------
// Metric Metadata  — GET /api/v1/metrics/{name}
// ---------------------------------------------------------------------------

/// Response from `GET /api/v1/metrics/{metric_name}`.
#[derive(Debug, Deserialize)]
pub(crate) struct DdMetricMetadata {
    pub description: Option<String>,
    #[serde(rename = "type")]
    pub metric_type: Option<String>,
    pub unit: Option<String>,
    pub per_unit: Option<String>,
}

// ---------------------------------------------------------------------------
// Logs Search  — POST /api/v2/logs/events/search
// ---------------------------------------------------------------------------

/// Top-level response from `POST /api/v2/logs/events/search`.
#[derive(Debug, Deserialize)]
pub(crate) struct DdLogsResponse {
    #[serde(default)]
    pub data: Vec<DdLogEvent>,
    pub meta: Option<DdMeta>,
}

/// A single log event.
#[derive(Debug, Deserialize)]
pub(crate) struct DdLogEvent {
    /// Unique log ID.
    pub id: Option<String>,
    /// Log attributes.
    pub attributes: DdLogAttributes,
}

/// Attributes of a log event.
#[derive(Debug, Deserialize)]
pub(crate) struct DdLogAttributes {
    /// Log message body.
    pub message: Option<String>,
    /// Status string (e.g. `"error"`, `"info"`, `"warn"`).
    pub status: Option<String>,
    /// ISO 8601 timestamp string.
    pub timestamp: Option<String>,
    /// Service name.
    pub service: Option<String>,
    /// Hostname.
    pub host: Option<String>,
    /// Tags as `["key:value", ...]`.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Nested structured attributes from the log payload.
    pub attributes: Option<serde_json::Value>,
}

// ---------------------------------------------------------------------------
// Spans/Traces Search  — POST /api/v2/spans/events/search
// ---------------------------------------------------------------------------

/// Top-level response from `POST /api/v2/spans/events/search`.
#[derive(Debug, Deserialize)]
pub(crate) struct DdSpansResponse {
    #[serde(default)]
    pub data: Vec<DdSpanEvent>,
    pub meta: Option<DdMeta>,
}

/// A single span event.
#[derive(Debug, Deserialize)]
pub(crate) struct DdSpanEvent {
    /// Attributes of the span.
    pub attributes: DdSpanAttributes,
}

/// Attributes of a span event.
#[derive(Debug, Deserialize)]
pub(crate) struct DdSpanAttributes {
    /// Operation name (e.g. `"http.client.request"`).
    pub operation_name: Option<String>,
    /// Resource name (e.g. `"GET"`).
    pub resource_name: Option<String>,
    /// Service name.
    pub service: Option<String>,
    /// Trace ID (hex string).
    pub trace_id: Option<String>,
    /// Span ID (decimal string).
    pub span_id: Option<String>,
    /// Parent span ID (decimal string, `"0"` for root spans).
    pub parent_id: Option<String>,
    /// Span status: `"ok"` or `"error"`.
    pub status: Option<String>,
    /// Start timestamp as ISO 8601 string.
    pub start_timestamp: Option<String>,
    /// End timestamp as ISO 8601 string.
    pub end_timestamp: Option<String>,
    /// Span type (e.g. `"http"`, `"custom"`).
    #[serde(rename = "type")]
    pub span_type: Option<String>,
    /// Environment (e.g. `"prod"`, `"none"`).
    pub env: Option<String>,
    /// Hostname (kept for future resource mapping).
    #[allow(dead_code)]
    pub host: Option<String>,
    /// Tags as `["key:value", ...]` (kept for future resource mapping).
    #[allow(dead_code)]
    #[serde(default)]
    pub tags: Vec<String>,
    /// Custom/nested attributes containing duration, error details, HTTP
    /// info, `OTel` metadata, span kind, etc.
    pub custom: Option<serde_json::Value>,
}

// ---------------------------------------------------------------------------
// Shared pagination metadata
// ---------------------------------------------------------------------------

/// Pagination and request metadata shared across Datadog v2 APIs.
#[derive(Debug, Deserialize)]
pub(crate) struct DdMeta {
    pub page: Option<DdPage>,
    /// Request status (kept for future error handling).
    #[allow(dead_code)]
    pub status: Option<String>,
}

/// Pagination cursor.
#[derive(Debug, Deserialize)]
pub(crate) struct DdPage {
    pub after: Option<String>,
}
