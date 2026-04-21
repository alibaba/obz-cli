//! `OpenSearch` API response types.
//!
//! These types model the `OpenSearch` `_search` API response format for both
//! log and trace indices. The `OTel` data is stored in the `_source` field of
//! each hit document.

use std::collections::BTreeMap;

use serde::Deserialize;

// ---------------------------------------------------------------------------
// Common search response envelope
// ---------------------------------------------------------------------------

/// Top-level `OpenSearch` search response.
#[derive(Debug, Deserialize)]
pub(crate) struct OsSearchResponse {
    pub hits: OsHits,
}

/// The `hits` wrapper containing total count and hit documents.
#[derive(Debug, Deserialize)]
pub(crate) struct OsHits {
    pub total: OsTotal,
    pub hits: Vec<OsHit>,
}

/// Total hit count with relation (`eq` or `gte`).
#[derive(Debug, Deserialize)]
pub(crate) struct OsTotal {
    pub value: u64,
    /// `"eq"` for exact count, `"gte"` when the count is a lower bound.
    pub relation: String,
}

/// A single hit document.
#[derive(Debug, Deserialize)]
pub(crate) struct OsHit {
    #[allow(dead_code)]
    #[serde(rename = "_index")]
    pub index: String,
    #[serde(rename = "_id")]
    pub id: String,
    #[serde(rename = "_source")]
    pub source: serde_json::Value,
}

// ---------------------------------------------------------------------------
// Log document (`_source` for OTel log indices)
// ---------------------------------------------------------------------------

/// `OTel` log document stored in `OpenSearch`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OsLogDocument {
    /// Log message body.
    #[serde(default)]
    pub body: String,

    /// Severity information.
    pub severity: Option<OsSeverity>,

    /// Resource attributes (flattened, e.g. `service.name`).
    #[serde(default)]
    pub resource: BTreeMap<String, serde_json::Value>,

    /// Span/log attributes.
    #[serde(default)]
    pub attributes: BTreeMap<String, serde_json::Value>,

    /// Primary timestamp.
    #[serde(rename = "@timestamp")]
    pub timestamp: Option<String>,

    /// Observed timestamp (when the log was collected).
    pub observed_timestamp: Option<String>,

    /// Trace correlation.
    #[serde(rename = "traceId")]
    pub trace_id: Option<String>,
    #[serde(rename = "spanId")]
    pub span_id: Option<String>,
}

/// Severity with text and numeric level.
#[derive(Debug, Deserialize)]
pub(crate) struct OsSeverity {
    pub text: Option<String>,
    pub number: Option<u32>,
}

// ---------------------------------------------------------------------------
// Trace/span document (`_source` for OTel trace indices)
// ---------------------------------------------------------------------------

/// `OTel` span document stored in `OpenSearch`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OsSpanDocument {
    /// Trace ID (32-char hex).
    #[serde(rename = "traceId")]
    pub trace_id: String,

    /// Span ID (16-char hex).
    #[serde(rename = "spanId")]
    pub span_id: String,

    /// Parent span ID (empty string when root span).
    #[serde(rename = "parentSpanId", default)]
    pub parent_span_id: String,

    /// Operation name.
    #[serde(default)]
    pub name: String,

    /// Span kind: `"Client"`, `"Server"`, `"Internal"`, `"Producer"`, `"Consumer"`.
    #[serde(default)]
    pub kind: String,

    /// Span status.
    pub status: Option<OsSpanStatus>,

    /// Start time (RFC 3339 with nanosecond precision).
    pub start_time: Option<String>,

    /// End time (RFC 3339 with nanosecond precision).
    pub end_time: Option<String>,

    /// Resource attributes (flattened).
    #[serde(default)]
    pub resource: BTreeMap<String, serde_json::Value>,

    /// Span attributes.
    #[serde(default)]
    pub attributes: BTreeMap<String, serde_json::Value>,

    /// Span events (may be absent entirely).
    #[serde(default)]
    pub events: Vec<OsSpanEvent>,
}

/// Span status with string code and optional message.
#[derive(Debug, Deserialize)]
pub(crate) struct OsSpanStatus {
    /// `"Ok"`, `"Error"`, or `"Unset"`.
    pub code: String,
    /// Error message (empty when status is not error).
    #[serde(default)]
    pub message: String,
}

/// A span event stored in `OpenSearch`.
#[derive(Debug, Deserialize)]
pub(crate) struct OsSpanEvent {
    /// Event name (e.g. `"exception"`).
    #[serde(default)]
    pub name: String,

    /// Event timestamp (RFC 3339).
    #[serde(rename = "@timestamp")]
    pub timestamp: Option<String>,

    /// Event attributes.
    #[serde(default)]
    pub attributes: BTreeMap<String, serde_json::Value>,
}

// ---------------------------------------------------------------------------
// Error response
// ---------------------------------------------------------------------------

/// `OpenSearch` error response body (HTTP 4xx/5xx).
#[derive(Debug, Deserialize)]
pub(crate) struct OsErrorResponse {
    pub error: OsErrorDetail,
    #[allow(dead_code)]
    pub status: u32,
}

/// Error detail with type and reason.
#[derive(Debug, Deserialize)]
pub(crate) struct OsErrorDetail {
    #[serde(rename = "type")]
    pub error_type: String,
    pub reason: String,
}
