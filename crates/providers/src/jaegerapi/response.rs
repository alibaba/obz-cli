//! Jaeger HTTP API response types.
//!
//! These types model the Jaeger Query API response format, shared by any
//! backend that exposes a Jaeger-compatible endpoint (VictoriaTraces, Jaeger).
//! All responses follow the `JaegerResponse<T>` envelope with a `data` field.

use std::collections::BTreeMap;

use serde::Deserialize;

/// Top-level Jaeger API response envelope.
///
/// The `data` field is `Option<T>` because Jaeger returns `null` instead of
/// an empty array when no results are found (e.g. operations for a
/// nonexistent service).
#[derive(Debug, Deserialize)]
pub(crate) struct JaegerResponse<T> {
    pub data: Option<T>,
}

/// A single trace returned by `/api/traces` or `/api/traces/{traceID}`.
///
/// Contains a list of spans and a process map (processID → process info).
#[derive(Debug, Deserialize)]
pub(crate) struct JaegerTrace {
    #[serde(rename = "traceID")]
    pub trace_id: String,
    pub spans: Vec<JaegerSpan>,
    pub processes: BTreeMap<String, JaegerProcess>,
}

/// A single span in the Jaeger format.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct JaegerSpan {
    #[serde(rename = "traceID")]
    pub trace_id: String,
    #[serde(rename = "spanID")]
    pub span_id: String,
    pub operation_name: String,
    pub references: Vec<JaegerReference>,
    /// Start time in microseconds since Unix epoch.
    pub start_time: i64,
    /// Duration in microseconds.
    pub duration: i64,
    pub tags: Vec<JaegerKeyValue>,
    pub logs: Vec<JaegerLog>,
    #[serde(rename = "processID")]
    pub process_id: String,
}

/// A reference to a parent or follows-from span.
#[derive(Debug, Deserialize)]
pub(crate) struct JaegerReference {
    #[serde(rename = "refType")]
    pub ref_type: String,
    /// The trace ID of the referenced span. Not used in conversion.
    #[allow(dead_code)]
    #[serde(rename = "traceID")]
    pub trace_id: String,
    #[serde(rename = "spanID")]
    pub span_id: String,
}

/// A key-value tag on a span or process.
#[derive(Debug, Deserialize)]
pub(crate) struct JaegerKeyValue {
    pub key: String,
    /// The value type string (e.g. "string", "bool", "int64"). Not used in conversion.
    #[allow(dead_code)]
    #[serde(rename = "type")]
    pub value_type: String,
    pub value: serde_json::Value,
}

/// A span log (span event in `OTel` terminology).
#[derive(Debug, Deserialize)]
pub(crate) struct JaegerLog {
    pub timestamp: i64,
    pub fields: Vec<JaegerKeyValue>,
}

/// Process (service) info mapped by processID.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct JaegerProcess {
    pub service_name: String,
    pub tags: Vec<JaegerKeyValue>,
}
