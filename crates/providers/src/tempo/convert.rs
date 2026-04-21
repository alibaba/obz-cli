//! Tempo → obz trace model conversion.
//!
//! Converts Tempo API responses into normalized obz trace models.
//! Key transformations:
//!
//! - **Search results**: `TempoTraceEntry` → `Span` (one virtual root span per trace)
//! - **Trace detail** (OTLP JSON): `OtlpBatch` → `Span` with base64→hex ID decoding
//! - **Tags**: Flatten scoped tags into a string list
//! - **Tag values**: Extract value strings

use std::collections::BTreeMap;

use base64::prelude::*;

use obz_core::model::trace::{Span, SpanEvent, SpanKind, SpanStatus, TraceDetail};
use obz_core::provider::results::TraceSearchResult;

use crate::util::parse_nanos_to_seconds;

use super::response::{
    OtlpBatch, OtlpKeyValue, OtlpSpan, OtlpTraceResponse, OtlpValue, TempoSearchResponse,
    TempoTagValuesResponse, TempoTagsResponse,
};

// ---------------------------------------------------------------------------
// Search result conversion
// ---------------------------------------------------------------------------

/// Convert a Tempo search response into a `TraceSearchResult`.
///
/// Each `TempoTraceEntry` becomes a single virtual root span, since Tempo
/// search results provide trace-level summaries rather than individual spans.
pub(crate) fn convert_search_result(resp: TempoSearchResponse) -> TraceSearchResult {
    let spans: Vec<Span> = resp
        .traces
        .into_iter()
        .map(|entry| {
            let start_time = entry
                .start_time_unix_nano
                .as_deref()
                .map(parse_nanos_to_seconds)
                .unwrap_or(0);

            let duration_us = entry.duration_ms.map(|ms| (ms * 1000) as i64).unwrap_or(0);

            Span {
                // Tempo search API may omit leading zeros from trace IDs
                // (e.g. 30 or 31 hex chars instead of 32). Pad to the
                // standard 32-char hex format for consistency with the
                // trace detail path (which decodes base64 -> fixed 32 hex).
                trace_id: format!("{:0>32}", entry.trace_id),
                span_id: String::new(),
                parent_span_id: None,
                name: entry.root_trace_name.unwrap_or_default(),
                service: entry.root_service_name.unwrap_or_default(),
                kind: None,
                status: SpanStatus::Unset,
                start_time,
                duration_us,
                attributes: None,
                events: None,
                resource: None,
                extensions: None,
            }
        })
        .collect();

    let total = spans.len();
    TraceSearchResult {
        spans,
        total_count: total,
        is_complete: None,
        cursor: None,
    }
}

// ---------------------------------------------------------------------------
// Trace detail conversion (OTLP JSON)
// ---------------------------------------------------------------------------

/// Convert an OTLP trace response into a `TraceDetail`.
pub(crate) fn convert_trace_detail(resp: &OtlpTraceResponse) -> TraceDetail {
    let mut all_spans = Vec::new();
    let mut trace_id = String::new();

    for batch in &resp.batches {
        let service = extract_service_name(batch);
        let resource = extract_resource_attrs(batch);

        for scope_spans in &batch.scope_spans {
            for otlp_span in &scope_spans.spans {
                let span = convert_otlp_span(otlp_span, &service, &resource);
                if trace_id.is_empty() {
                    trace_id.clone_from(&span.trace_id);
                }
                all_spans.push(span);
            }
        }
    }

    TraceDetail::from_spans(trace_id, all_spans)
}

/// Convert a single OTLP span to an obz `Span`.
fn convert_otlp_span(otlp: &OtlpSpan, service: &str, resource: &BTreeMap<String, String>) -> Span {
    let trace_id = decode_base64_to_hex(&otlp.trace_id);
    let span_id = decode_base64_to_hex(&otlp.span_id);
    let parent_span_id = otlp
        .parent_span_id
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(decode_base64_to_hex);

    // Parse timestamps.
    let start_time = otlp
        .start_time_unix_nano
        .as_deref()
        .map(parse_nanos_to_seconds)
        .unwrap_or(0);

    let end_nanos: i128 = otlp
        .end_time_unix_nano
        .as_deref()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let start_nanos: i128 = otlp
        .start_time_unix_nano
        .as_deref()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let duration_us = if end_nanos > start_nanos {
        ((end_nanos - start_nanos) / 1_000) as i64
    } else {
        0
    };

    // Parse span kind.
    let kind = otlp.kind.as_deref().map(parse_otlp_span_kind);

    // Parse status.
    let status = otlp
        .status
        .as_ref()
        .and_then(|s| s.code.as_deref())
        .map(parse_otlp_status)
        .unwrap_or(SpanStatus::Unset);

    // Convert attributes.
    let attributes = convert_otlp_attributes(&otlp.attributes);

    // Convert events.
    let events: Vec<SpanEvent> = otlp.events.iter().map(convert_otlp_event).collect();

    Span {
        trace_id,
        span_id,
        parent_span_id,
        name: otlp.name.clone(),
        service: service.to_string(),
        kind,
        status,
        start_time,
        duration_us,
        attributes: if attributes.is_empty() {
            None
        } else {
            Some(attributes)
        },
        events: if events.is_empty() {
            None
        } else {
            Some(events)
        },
        resource: if resource.is_empty() {
            None
        } else {
            Some(resource.clone())
        },
        extensions: None,
    }
}

/// Convert an OTLP event to a `SpanEvent`.
fn convert_otlp_event(event: &super::response::OtlpEvent) -> SpanEvent {
    let timestamp = event
        .time_unix_nano
        .as_deref()
        .map(parse_nanos_to_seconds)
        .unwrap_or(0);

    let attributes = convert_otlp_attributes(&event.attributes);

    SpanEvent {
        name: event.name.clone(),
        timestamp,
        attributes: if attributes.is_empty() {
            None
        } else {
            Some(attributes)
        },
    }
}

// ---------------------------------------------------------------------------
// Tags conversion
// ---------------------------------------------------------------------------

/// Convert a Tempo tags response to a flat list of tag names.
///
/// Tags are prefixed with their scope for disambiguation
/// (e.g., `"resource.service.name"`, `"span.http.method"`).
pub(crate) fn convert_tags_response(resp: TempoTagsResponse) -> Vec<String> {
    let mut tags = Vec::new();
    for scope in resp.scopes {
        for tag in scope.tags {
            if scope.name == "intrinsic" {
                tags.push(tag);
            } else {
                tags.push(format!("{}.{}", scope.name, tag));
            }
        }
    }
    tags
}

/// Convert a Tempo tag values response to a list of strings.
pub(crate) fn convert_tag_values_response(resp: TempoTagValuesResponse) -> Vec<String> {
    resp.tag_values.into_iter().map(|tv| tv.value).collect()
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Decode a base64-encoded ID to a lowercase hex string.
///
/// Tempo's OTLP JSON format encodes trace IDs and span IDs as
/// standard base64 (RFC 4648). We decode them to the hex format
/// expected by obz and other tools.
fn decode_base64_to_hex(input: &str) -> String {
    if input.is_empty() {
        return String::new();
    }
    match BASE64_STANDARD.decode(input) {
        Ok(bytes) => {
            let mut hex = String::with_capacity(bytes.len() * 2);
            for b in &bytes {
                hex.push_str(&format!("{b:02x}"));
            }
            hex
        }
        Err(_) => {
            // If base64 decoding fails, return the input as-is.
            // This handles cases where the ID might already be hex.
            input.to_string()
        }
    }
}

/// Extract the `service.name` from a batch's resource attributes.
fn extract_service_name(batch: &OtlpBatch) -> String {
    batch
        .resource
        .as_ref()
        .and_then(|r| {
            r.attributes
                .iter()
                .find(|kv| kv.key == "service.name")
                .and_then(|kv| otlp_value_to_string(kv.value.as_ref()))
        })
        .unwrap_or_default()
}

/// Extract all resource attributes as a `BTreeMap`.
fn extract_resource_attrs(batch: &OtlpBatch) -> BTreeMap<String, String> {
    batch
        .resource
        .as_ref()
        .map(|r| convert_otlp_attributes(&r.attributes))
        .unwrap_or_default()
}

/// Convert OTLP key-value attributes to a `BTreeMap<String, String>`.
fn convert_otlp_attributes(attrs: &[OtlpKeyValue]) -> BTreeMap<String, String> {
    attrs
        .iter()
        .filter_map(|kv| otlp_value_to_string(kv.value.as_ref()).map(|v| (kv.key.clone(), v)))
        .collect()
}

/// Extract a string representation from an OTLP value.
fn otlp_value_to_string(value: Option<&OtlpValue>) -> Option<String> {
    let v = value?;
    if let Some(s) = &v.string_value {
        return Some(s.clone());
    }
    if let Some(i) = &v.int_value {
        return Some(i.clone());
    }
    if let Some(b) = v.bool_value {
        return Some(b.to_string());
    }
    if let Some(d) = v.double_value {
        return Some(d.to_string());
    }
    None
}

/// Parse an OTLP span kind string to `SpanKind`.
fn parse_otlp_span_kind(s: &str) -> SpanKind {
    match s {
        "SPAN_KIND_CLIENT" => SpanKind::Client,
        "SPAN_KIND_SERVER" => SpanKind::Server,
        "SPAN_KIND_PRODUCER" => SpanKind::Producer,
        "SPAN_KIND_CONSUMER" => SpanKind::Consumer,
        "SPAN_KIND_INTERNAL" => SpanKind::Internal,
        // Numeric values from OTLP protobuf enum
        "1" => SpanKind::Internal,
        "2" => SpanKind::Server,
        "3" => SpanKind::Client,
        "4" => SpanKind::Producer,
        "5" => SpanKind::Consumer,
        _ => SpanKind::Internal,
    }
}

/// Parse an OTLP status code string to `SpanStatus`.
fn parse_otlp_status(s: &str) -> SpanStatus {
    match s {
        "STATUS_CODE_OK" => SpanStatus::Ok,
        "STATUS_CODE_ERROR" => SpanStatus::Error,
        "STATUS_CODE_UNSET" | "" => SpanStatus::Unset,
        _ => SpanStatus::Unset,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tempo::response::{OtlpTraceResponse, TempoSearchResponse};

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

    // -- Unit tests ---------------------------------------------------------

    #[test]
    fn test_decode_base64_to_hex() {
        // "HOI5k5F6A2XZqJl9nNCW5A==" decodes to the trace ID
        // 0x1ce23993917a0365d9a8997d9cd096e4
        let hex = decode_base64_to_hex("HOI5k5F6A2XZqJl9nNCW5A==");
        assert_eq!(hex, "1ce23993917a0365d9a8997d9cd096e4");

        // Empty input → empty output
        assert_eq!(decode_base64_to_hex(""), "");

        // Span ID: "EojhBf5hBh8=" (8 bytes)
        let hex = decode_base64_to_hex("EojhBf5hBh8=");
        assert_eq!(hex, "1288e105fe61061f");
    }

    #[test]
    fn test_parse_otlp_span_kind() {
        assert_eq!(parse_otlp_span_kind("SPAN_KIND_CLIENT"), SpanKind::Client);
        assert_eq!(parse_otlp_span_kind("SPAN_KIND_SERVER"), SpanKind::Server);
        assert_eq!(
            parse_otlp_span_kind("SPAN_KIND_INTERNAL"),
            SpanKind::Internal
        );
        assert_eq!(parse_otlp_span_kind("unknown"), SpanKind::Internal);
    }

    #[test]
    fn test_parse_otlp_status() {
        assert_eq!(parse_otlp_status("STATUS_CODE_OK"), SpanStatus::Ok);
        assert_eq!(parse_otlp_status("STATUS_CODE_ERROR"), SpanStatus::Error);
        assert_eq!(parse_otlp_status("STATUS_CODE_UNSET"), SpanStatus::Unset);
        assert_eq!(parse_otlp_status(""), SpanStatus::Unset);
    }

    #[test]
    fn test_otlp_value_to_string() {
        let v = OtlpValue {
            string_value: Some("hello".into()),
            int_value: None,
            bool_value: None,
            double_value: None,
        };
        assert_eq!(otlp_value_to_string(Some(&v)), Some("hello".into()));

        let v = OtlpValue {
            string_value: None,
            int_value: Some("42".into()),
            bool_value: None,
            double_value: None,
        };
        assert_eq!(otlp_value_to_string(Some(&v)), Some("42".into()));

        let v = OtlpValue {
            string_value: None,
            int_value: None,
            bool_value: Some(true),
            double_value: None,
        };
        assert_eq!(otlp_value_to_string(Some(&v)), Some("true".into()));

        assert_eq!(otlp_value_to_string(None), None);
    }

    // -- Fixture tests: Search ----------------------------------------------

    #[test]
    fn fixture_search_all() {
        let body = load_fixture("search-all");
        let resp: TempoSearchResponse = serde_json::from_value(body).unwrap();
        let result = convert_search_result(resp);

        assert_eq!(result.total_count, 5);
        assert_eq!(result.spans.len(), 5);

        for span in &result.spans {
            assert!(!span.trace_id.is_empty(), "trace_id should not be empty");
            assert!(span.start_time > 0, "start_time should be positive");
        }

        // First trace should have correct service and name
        let first = &result.spans[0];
        assert_eq!(first.service, "load-generator");
        assert_eq!(first.name, "user_get_ads");
    }

    #[test]
    fn fixture_search_empty() {
        let body = load_fixture("search-empty");
        let resp: TempoSearchResponse = serde_json::from_value(body).unwrap();
        let result = convert_search_result(resp);
        assert_eq!(result.total_count, 0);
        assert!(result.spans.is_empty());
    }

    #[test]
    fn fixture_search_by_service() {
        let body = load_fixture("search-by-service");
        let resp: TempoSearchResponse = serde_json::from_value(body).unwrap();
        let result = convert_search_result(resp);
        assert_eq!(result.total_count, 5);

        for span in &result.spans {
            assert_eq!(span.service, "cart");
            // Verify trace_id is padded to 32 hex chars (the fixture
            // contains 31-char IDs with omitted leading zeros).
            assert_eq!(
                span.trace_id.len(),
                32,
                "trace_id not normalized: {}",
                span.trace_id
            );
        }
    }

    #[test]
    fn fixture_search_by_operation() {
        let body = load_fixture("search-by-operation");
        let resp: TempoSearchResponse = serde_json::from_value(body).unwrap();
        let result = convert_search_result(resp);
        assert!(!result.spans.is_empty());
    }

    #[test]
    fn fixture_search_by_status() {
        let body = load_fixture("search-by-status");
        let resp: TempoSearchResponse = serde_json::from_value(body).unwrap();
        let result = convert_search_result(resp);
        assert!(!result.spans.is_empty());

        // Verify trace_id padding (the fixture contains 30-char IDs).
        for span in &result.spans {
            assert_eq!(
                span.trace_id.len(),
                32,
                "trace_id not normalized: {}",
                span.trace_id
            );
        }
    }

    // -- Fixture tests: Trace detail ----------------------------------------

    #[test]
    fn fixture_trace_by_id() {
        let body = load_fixture("trace-by-id");
        let resp: OtlpTraceResponse = serde_json::from_value(body).unwrap();
        let detail = convert_trace_detail(&resp);

        // The fixture has 2 spans across 2 batches
        assert_eq!(detail.span_count, 2);
        assert_eq!(detail.trace_id, "1ce23993917a0365d9a8997d9cd096e4");

        // Both spans belong to "load-generator"
        for span in &detail.spans {
            assert_eq!(span.service, "load-generator");
            assert_eq!(span.trace_id, "1ce23993917a0365d9a8997d9cd096e4");
        }

        // First span (GET) should have CLIENT kind and ERROR status
        let get_span = detail
            .spans
            .iter()
            .find(|s| s.name == "GET")
            .expect("should find GET span");
        assert_eq!(get_span.kind, Some(SpanKind::Client));
        assert_eq!(get_span.status, SpanStatus::Error);
        assert!(get_span.events.is_some(), "GET span should have events");

        // Second span (user_browse_product) should have INTERNAL kind
        let browse_span = detail
            .spans
            .iter()
            .find(|s| s.name == "user_browse_product")
            .expect("should find user_browse_product span");
        assert_eq!(browse_span.kind, Some(SpanKind::Internal));

        // The GET span should be a child of user_browse_product
        assert_eq!(
            get_span.parent_span_id.as_deref(),
            Some(&browse_span.span_id[..])
        );
    }

    #[test]
    fn fixture_trace_by_id_events() {
        let body = load_fixture("trace-by-id");
        let resp: OtlpTraceResponse = serde_json::from_value(body).unwrap();
        let detail = convert_trace_detail(&resp);

        let get_span = detail.spans.iter().find(|s| s.name == "GET").unwrap();

        let events = get_span.events.as_ref().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].name, "exception");

        let attrs = events[0].attributes.as_ref().unwrap();
        assert!(attrs.contains_key("exception.type"));
        assert!(attrs.contains_key("exception.message"));
    }

    #[test]
    fn fixture_trace_by_id_resource() {
        let body = load_fixture("trace-by-id");
        let resp: OtlpTraceResponse = serde_json::from_value(body).unwrap();
        let detail = convert_trace_detail(&resp);

        for span in &detail.spans {
            let resource = span.resource.as_ref().expect("should have resource");
            assert_eq!(
                resource.get("service.name").map(String::as_str),
                Some("load-generator")
            );
            assert!(resource.contains_key("telemetry.sdk.language"));
        }
    }

    // -- Fixture tests: Tags ------------------------------------------------

    #[test]
    fn fixture_tags() {
        let body = load_fixture("tags");
        let resp: TempoTagsResponse = serde_json::from_value(body).unwrap();
        let tags = convert_tags_response(resp);

        assert!(!tags.is_empty());
        // Intrinsic tags should not have a scope prefix
        assert!(tags.contains(&"duration".to_string()));
        // Resource tags should have "resource." prefix
        assert!(tags.contains(&"resource.service.name".to_string()));
        // Span tags should have "span." prefix
        assert!(tags.contains(&"span.http.method".to_string()));
    }

    #[test]
    fn fixture_tag_values() {
        let body = load_fixture("tag-values-service");
        let resp: TempoTagValuesResponse = serde_json::from_value(body).unwrap();
        let values = convert_tag_values_response(resp);

        assert!(!values.is_empty());
        assert!(values.contains(&"cart".to_string()));
        assert!(values.contains(&"load-generator".to_string()));
    }
}
