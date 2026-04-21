//! Tempo API response deserialization types.
//!
//! Tempo has three distinct response formats:
//!
//! 1. **Search** (`/api/search`) — Returns trace summaries with optional
//!    `spanSets` when a TraceQL query is used.
//! 2. **Trace detail** (`/api/traces/{id}`) — Returns OTLP protobuf-JSON
//!    format with `batches[].scopeSpans[].spans[]`.
//! 3. **Tags** (`/api/v2/search/tags`) — Returns tag names grouped by scope.
//! 4. **Tag values** (`/api/v2/search/tag/{name}/values`) — Returns tag values.
//!
//! Error responses from Tempo are `text/plain` (not JSON).

use serde::Deserialize;

// ---------------------------------------------------------------------------
// Search response (/api/search)
// ---------------------------------------------------------------------------

/// Top-level search response from `/api/search`.
#[derive(Debug, Deserialize)]
pub(crate) struct TempoSearchResponse {
    /// Matching traces (may be empty).
    #[serde(default)]
    pub traces: Vec<TempoTraceEntry>,
}

/// A single trace summary from a search result.
#[derive(Debug, Deserialize)]
pub(crate) struct TempoTraceEntry {
    /// Trace ID as a hex string (32 characters).
    #[serde(rename = "traceID")]
    pub trace_id: String,
    /// Service name of the root span.
    #[serde(rename = "rootServiceName")]
    pub root_service_name: Option<String>,
    /// Operation name of the root span.
    #[serde(rename = "rootTraceName")]
    pub root_trace_name: Option<String>,
    /// Start time as nanosecond Unix timestamp string.
    #[serde(rename = "startTimeUnixNano")]
    pub start_time_unix_nano: Option<String>,
    /// Duration in milliseconds.
    #[serde(rename = "durationMs")]
    pub duration_ms: Option<u64>,
    /// Matched span sets (present when a `TraceQL` query is used).
    #[allow(dead_code)]
    #[serde(default, rename = "spanSets")]
    pub span_sets: Vec<TempoSpanSet>,
}

/// A set of spans matching a `TraceQL` query condition.
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub(crate) struct TempoSpanSet {
    /// Matching spans within this set.
    #[serde(default)]
    pub spans: Vec<TempoSearchSpan>,
    /// Number of matched spans.
    #[serde(default)]
    pub matched: u64,
}

/// A span reference within a search result span set.
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub(crate) struct TempoSearchSpan {
    /// Span ID as hex string.
    #[serde(rename = "spanID")]
    pub span_id: String,
    /// Start time as nanosecond Unix timestamp string.
    #[serde(rename = "startTimeUnixNano")]
    pub start_time_unix_nano: Option<String>,
    /// Duration in nanoseconds.
    #[serde(rename = "durationNanos")]
    pub duration_nanos: Option<String>,
    /// Span attributes (OTLP key-value format).
    #[serde(default)]
    pub attributes: Vec<OtlpKeyValue>,
}

// ---------------------------------------------------------------------------
// Trace detail response (/api/traces/{id}) — OTLP protobuf-JSON
// ---------------------------------------------------------------------------

/// Top-level OTLP trace response.
#[derive(Debug, Deserialize)]
pub(crate) struct OtlpTraceResponse {
    /// Resource batches.
    #[serde(default)]
    pub batches: Vec<OtlpBatch>,
}

/// A resource batch containing scope spans.
#[derive(Debug, Deserialize)]
pub(crate) struct OtlpBatch {
    /// Resource metadata (service name, host, etc.).
    pub resource: Option<OtlpResource>,
    /// Scope spans within this batch.
    #[serde(default, rename = "scopeSpans")]
    pub scope_spans: Vec<OtlpScopeSpans>,
}

/// OTLP resource with key-value attributes.
#[derive(Debug, Deserialize)]
pub(crate) struct OtlpResource {
    /// Resource attributes.
    #[serde(default)]
    pub attributes: Vec<OtlpKeyValue>,
}

/// A scope (instrumentation library) and its spans.
#[derive(Debug, Deserialize)]
pub(crate) struct OtlpScopeSpans {
    /// Spans within this scope.
    #[serde(default)]
    pub spans: Vec<OtlpSpan>,
}

/// An OTLP span in protobuf-JSON format.
#[derive(Debug, Deserialize)]
pub(crate) struct OtlpSpan {
    /// Trace ID — **base64 encoded** in OTLP JSON format.
    #[serde(default, rename = "traceId")]
    pub trace_id: String,
    /// Span ID — **base64 encoded**.
    #[serde(default, rename = "spanId")]
    pub span_id: String,
    /// Parent span ID — **base64 encoded** (empty for root spans).
    #[serde(default, rename = "parentSpanId")]
    pub parent_span_id: Option<String>,
    /// Span name (operation name).
    #[serde(default)]
    pub name: String,
    /// Span kind (e.g., `"SPAN_KIND_CLIENT"`, `"SPAN_KIND_SERVER"`).
    #[serde(default)]
    pub kind: Option<String>,
    /// Start time as nanosecond Unix timestamp string.
    #[serde(default, rename = "startTimeUnixNano")]
    pub start_time_unix_nano: Option<String>,
    /// End time as nanosecond Unix timestamp string.
    #[serde(default, rename = "endTimeUnixNano")]
    pub end_time_unix_nano: Option<String>,
    /// Span attributes.
    #[serde(default)]
    pub attributes: Vec<OtlpKeyValue>,
    /// Span events (logs, exceptions).
    #[serde(default)]
    pub events: Vec<OtlpEvent>,
    /// Span status.
    #[serde(default)]
    pub status: Option<OtlpStatus>,
}

/// OTLP key-value pair (used in attributes, resource, events).
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct OtlpKeyValue {
    /// Attribute key.
    pub key: String,
    /// Attribute value (wrapped in a typed container).
    pub value: Option<OtlpValue>,
}

/// OTLP value container — supports string, int, bool, double values.
///
/// In practice, most Tempo attributes are string values.
///
/// Note: `arrayValue` and `kvlistValue` are not yet supported. These
/// complex value types are rare in span attributes but may appear in
/// resource attributes or event payloads.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct OtlpValue {
    /// String value.
    #[serde(default, rename = "stringValue")]
    pub string_value: Option<String>,
    /// Integer value (as string in JSON).
    #[serde(default, rename = "intValue")]
    pub int_value: Option<String>,
    /// Boolean value.
    #[serde(default, rename = "boolValue")]
    pub bool_value: Option<bool>,
    /// Double value.
    #[serde(default, rename = "doubleValue")]
    pub double_value: Option<f64>,
}

/// OTLP span event.
#[derive(Debug, Deserialize)]
pub(crate) struct OtlpEvent {
    /// Event timestamp as nanosecond Unix timestamp string.
    #[serde(default, rename = "timeUnixNano")]
    pub time_unix_nano: Option<String>,
    /// Event name (e.g., `"exception"`).
    #[serde(default)]
    pub name: String,
    /// Event attributes.
    #[serde(default)]
    pub attributes: Vec<OtlpKeyValue>,
}

/// OTLP span status.
#[derive(Debug, Deserialize)]
pub(crate) struct OtlpStatus {
    /// Status message (human-readable).
    #[allow(dead_code)]
    #[serde(default)]
    pub message: Option<String>,
    /// Status code (e.g., `"STATUS_CODE_ERROR"`, `"STATUS_CODE_OK"`).
    #[serde(default)]
    pub code: Option<String>,
}

// ---------------------------------------------------------------------------
// Tags response (/api/v2/search/tags)
// ---------------------------------------------------------------------------

/// Response from `/api/v2/search/tags`.
#[derive(Debug, Deserialize)]
pub(crate) struct TempoTagsResponse {
    /// Tag scopes (intrinsic, resource, span, event).
    #[serde(default)]
    pub scopes: Vec<TempoTagScope>,
}

/// A tag scope containing tag names.
#[derive(Debug, Deserialize)]
pub(crate) struct TempoTagScope {
    /// Scope name (e.g., `"resource"`, `"span"`).
    pub name: String,
    /// Tag names within this scope.
    #[serde(default)]
    pub tags: Vec<String>,
}

// ---------------------------------------------------------------------------
// Tag values response (/api/v2/search/tag/{name}/values)
// ---------------------------------------------------------------------------

/// Response from `/api/v2/search/tag/{name}/values`.
#[derive(Debug, Deserialize)]
pub(crate) struct TempoTagValuesResponse {
    /// Tag values.
    #[serde(default, rename = "tagValues")]
    pub tag_values: Vec<TempoTagValue>,
}

/// A single tag value.
#[derive(Debug, Deserialize)]
pub(crate) struct TempoTagValue {
    /// Value as a string.
    pub value: String,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn load_fixture(name: &str) -> serde_json::Value {
        let path = format!(
            "{}/fixtures/grafana/traces/{name}.json",
            env!("CARGO_MANIFEST_DIR").replace("/crates/providers", "")
        );
        let content = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("failed to read fixture {path}: {e}"));
        let fixture: serde_json::Value = serde_json::from_str(&content)
            .unwrap_or_else(|e| panic!("failed to parse fixture {path}: {e}"));
        fixture["response"]["body"].clone()
    }

    #[test]
    fn deserialize_search_all() {
        let body = load_fixture("search-all");
        let resp: TempoSearchResponse = serde_json::from_value(body).unwrap();
        assert_eq!(resp.traces.len(), 5);

        let first = &resp.traces[0];
        assert!(!first.trace_id.is_empty());
        assert!(first.root_service_name.is_some());
        assert!(first.start_time_unix_nano.is_some());
    }

    #[test]
    fn deserialize_search_by_service() {
        let body = load_fixture("search-by-service");
        let resp: TempoSearchResponse = serde_json::from_value(body).unwrap();
        assert_eq!(resp.traces.len(), 5);

        // Service-filtered results should have spanSets
        let first = &resp.traces[0];
        assert!(!first.span_sets.is_empty());
        assert!(!first.span_sets[0].spans.is_empty());
    }

    #[test]
    fn deserialize_search_empty() {
        let body = load_fixture("search-empty");
        let resp: TempoSearchResponse = serde_json::from_value(body).unwrap();
        assert!(resp.traces.is_empty());
    }

    #[test]
    fn deserialize_trace_by_id() {
        let body = load_fixture("trace-by-id");
        let resp: OtlpTraceResponse = serde_json::from_value(body).unwrap();
        assert_eq!(resp.batches.len(), 2);

        // First batch should have resource attributes
        let batch = &resp.batches[0];
        let resource = batch.resource.as_ref().unwrap();
        assert!(!resource.attributes.is_empty());

        // Should have scope spans with actual spans
        assert!(!batch.scope_spans.is_empty());
        let spans = &batch.scope_spans[0].spans;
        assert!(!spans.is_empty());

        // Check span structure
        let span = &spans[0];
        assert!(!span.trace_id.is_empty());
        assert!(!span.span_id.is_empty());
        assert!(span.kind.is_some());
        assert!(span.start_time_unix_nano.is_some());
        assert!(span.end_time_unix_nano.is_some());
    }

    #[test]
    fn deserialize_trace_with_events() {
        let body = load_fixture("trace-by-id");
        let resp: OtlpTraceResponse = serde_json::from_value(body).unwrap();

        // The first span has an exception event
        let span = &resp.batches[0].scope_spans[0].spans[0];
        assert!(!span.events.is_empty());

        let event = &span.events[0];
        assert_eq!(event.name, "exception");
        assert!(!event.attributes.is_empty());
    }

    #[test]
    fn deserialize_trace_with_error_status() {
        let body = load_fixture("trace-by-id");
        let resp: OtlpTraceResponse = serde_json::from_value(body).unwrap();

        let span = &resp.batches[0].scope_spans[0].spans[0];
        let status = span.status.as_ref().unwrap();
        assert_eq!(status.code.as_deref(), Some("STATUS_CODE_ERROR"));
        assert!(status.message.is_some());
    }

    #[test]
    fn deserialize_tags() {
        let body = load_fixture("tags");
        let resp: TempoTagsResponse = serde_json::from_value(body).unwrap();
        assert_eq!(resp.scopes.len(), 4);

        // Check scope names
        let scope_names: Vec<&str> = resp.scopes.iter().map(|s| s.name.as_str()).collect();
        assert!(scope_names.contains(&"intrinsic"));
        assert!(scope_names.contains(&"resource"));
        assert!(scope_names.contains(&"span"));
        assert!(scope_names.contains(&"event"));
    }

    #[test]
    fn deserialize_tag_values() {
        let body = load_fixture("tag-values-service");
        let resp: TempoTagValuesResponse = serde_json::from_value(body).unwrap();
        assert!(!resp.tag_values.is_empty());
        assert!(resp.tag_values.iter().any(|tv| tv.value == "cart"));
    }
}
