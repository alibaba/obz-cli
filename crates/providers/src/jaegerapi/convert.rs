//! Jaeger API response → obz model conversion.
//!
//! Converts Jaeger HTTP API responses into obz unified data models.
//! Shared across providers with Jaeger-compatible endpoints
//! (VictoriaTraces, Jaeger, etc.).

use std::collections::BTreeMap;

use obz_core::model::trace::{Span, SpanEvent, SpanKind, SpanStatus, TraceDetail};
use obz_core::provider::results::TraceSearchResult;

use super::response::{JaegerSpan, JaegerTrace};

/// Convert a list of Jaeger traces (from `GET /api/traces`) into a `TraceSearchResult`.
pub(crate) fn convert_search_result(traces: Vec<JaegerTrace>) -> TraceSearchResult {
    let spans: Vec<Span> = traces
        .into_iter()
        .flat_map(|t| convert_trace_spans(&t))
        .collect();
    let total = spans.len();
    TraceSearchResult {
        spans,
        total_count: total,
        is_complete: None,
        cursor: None,
    }
}

/// Convert a single Jaeger trace (from `GET /api/traces/{id}`) into a `TraceDetail`.
pub(crate) fn convert_trace_detail(trace: &JaegerTrace) -> TraceDetail {
    let trace_id = trace.trace_id.clone();
    let spans = convert_trace_spans(trace);
    TraceDetail::from_spans(trace_id, spans)
}

/// Convert all spans within a Jaeger trace into obz `Span` structs.
fn convert_trace_spans(trace: &JaegerTrace) -> Vec<Span> {
    trace.spans.iter().map(|s| convert_span(s, trace)).collect()
}

/// Convert a single Jaeger span into an obz `Span`.
fn convert_span(span: &JaegerSpan, trace: &JaegerTrace) -> Span {
    let service = trace
        .processes
        .get(&span.process_id)
        .map(|p| p.service_name.clone())
        .unwrap_or_default();

    // Parent span ID from the first CHILD_OF reference.
    let parent_span_id = span
        .references
        .iter()
        .find(|r| r.ref_type == "CHILD_OF")
        .map(|r| r.span_id.clone());

    // Parse tags: extract span.kind and error, flatten the rest to attributes.
    let mut attributes: BTreeMap<String, String> = BTreeMap::new();
    let mut kind = SpanKind::Internal;
    let mut is_error = false;

    for tag in &span.tags {
        match tag.key.as_str() {
            "span.kind" => {
                kind = SpanKind::parse(tag.value.as_str().unwrap_or(""));
            }
            "error" => {
                if tag.value == serde_json::Value::Bool(true) || tag.value.as_str() == Some("true")
                {
                    is_error = true;
                }
            }
            _ => {
                attributes.insert(
                    tag.key.clone(),
                    crate::util::json_value_to_string(&tag.value),
                );
            }
        }
    }

    // Process tags → resource attributes.
    let resource: BTreeMap<String, String> = trace
        .processes
        .get(&span.process_id)
        .map(|p| {
            p.tags
                .iter()
                .map(|t| (t.key.clone(), crate::util::json_value_to_string(&t.value)))
                .collect()
        })
        .unwrap_or_default();

    // Span logs → SpanEvent.
    let events: Vec<SpanEvent> = span
        .logs
        .iter()
        .map(|log| {
            // The "event" field is the event name; everything else is attributes.
            let name = log
                .fields
                .iter()
                .find(|f| f.key == "event")
                .map(|f| crate::util::json_value_to_string(&f.value))
                .unwrap_or_else(|| "log".to_string());
            let event_attrs: BTreeMap<String, String> = log
                .fields
                .iter()
                .filter(|f| f.key != "event")
                .map(|f| (f.key.clone(), crate::util::json_value_to_string(&f.value)))
                .collect();
            SpanEvent {
                name,
                // Jaeger log timestamps are in microseconds → Unix seconds.
                timestamp: log.timestamp / 1_000_000,
                attributes: if event_attrs.is_empty() {
                    None
                } else {
                    Some(event_attrs)
                },
            }
        })
        .collect();

    Span {
        trace_id: span.trace_id.clone(),
        span_id: span.span_id.clone(),
        parent_span_id,
        name: span.operation_name.clone(),
        service,
        kind: Some(kind),
        status: if is_error {
            SpanStatus::Error
        } else {
            SpanStatus::Ok
        },
        // Jaeger startTime is in microseconds → Unix seconds.
        start_time: span.start_time / 1_000_000,
        // Jaeger duration is in microseconds.
        duration_us: span.duration,
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
            Some(resource)
        },
        extensions: None,
    }
}
