//! Elasticsearch API response → obz model conversion.
//!
//! Converts Elasticsearch `_search` responses into obz unified data models
//! for both log and trace data streams.
//!
//! # Key differences from `OpenSearch` conversion
//!
//! - **Timestamps**: `epoch_millis` strings (not RFC 3339).
//! - **Log body**: extracted from `body.text` (nested), not a plain string.
//! - **Severity**: top-level `severity_text` (not nested `severity.text`).
//! - **Resource**: nested `resource.attributes` (not flat map).
//! - **Span duration**: explicit `duration` field in nanoseconds (not computed from start/end).
//! - **Span status Unset**: `status.code` is `None` (from `{}`), not `"Unset"`.

use std::collections::BTreeMap;

use obz_core::model::log::{parse_severity, severity_from_otel_number, LogEntry};
use obz_core::model::trace::{Span, SpanKind, SpanStatus, TraceDetail};
use obz_core::provider::results::{LogSearchResult, TraceSearchResult};

use super::response::{EsHit, EsLogDocument, EsSearchResponse, EsSpanDocument};

// ---------------------------------------------------------------------------
// Log conversion
// ---------------------------------------------------------------------------

/// Convert an Elasticsearch search response (from a log data stream) into a `LogSearchResult`.
pub(crate) fn convert_log_search_result(resp: EsSearchResponse) -> LogSearchResult {
    let total_count = resp.hits.total.value as usize;

    // Determine completeness from total count and relation *before*
    // consuming hits via into_iter(). We compare the raw hit count
    // (not entries.len()) because filter_map may drop malformed hits,
    // which would falsely report incomplete results.
    let is_complete = if resp.hits.total.relation == "gte" {
        Some(false)
    } else {
        Some(resp.hits.hits.len() >= total_count)
    };

    let entries: Vec<LogEntry> = resp
        .hits
        .hits
        .into_iter()
        .filter_map(convert_log_hit)
        .collect();

    LogSearchResult {
        entries,
        total_count,
        is_complete,
        cursor: None,
    }
}

/// Convert a single Elasticsearch log hit into an obz `LogEntry`.
fn convert_log_hit(hit: EsHit) -> Option<LogEntry> {
    let doc: EsLogDocument = serde_json::from_value(hit.source).ok()?;

    let timestamp = doc
        .timestamp
        .as_deref()
        .or(doc.observed_timestamp.as_deref())
        .and_then(parse_epoch_millis_to_seconds)
        .unwrap_or(0);

    let severity = doc
        .severity_text
        .as_deref()
        .map(parse_severity)
        .or_else(|| doc.severity_number.and_then(severity_from_otel_number));

    // Extract service name from nested resource.attributes.
    let service = doc
        .resource
        .as_ref()
        .and_then(|r| r.attributes.get("service.name"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    // Extract log body from nested body.text.
    let message = doc
        .body
        .as_ref()
        .and_then(|b| b.text.as_deref())
        .unwrap_or("")
        .to_string();

    // Flatten attributes to string map.
    let attributes = flatten_json_map(&doc.attributes);

    // Flatten resource attributes to string map.
    let resource = doc
        .resource
        .as_ref()
        .map(|r| flatten_json_map(&r.attributes))
        .unwrap_or_default();

    Some(LogEntry {
        timestamp,
        message,
        severity,
        source: None,
        service: if service.is_empty() {
            None
        } else {
            Some(service)
        },
        id: Some(hit.id),
        attributes: if attributes.is_empty() {
            None
        } else {
            Some(attributes)
        },
        resource: if resource.is_empty() {
            None
        } else {
            Some(resource)
        },
        trace_id: doc.trace_id,
        span_id: doc.span_id,
        extensions: None,
    })
}

// ---------------------------------------------------------------------------
// Trace conversion
// ---------------------------------------------------------------------------

/// Convert an Elasticsearch search response (from a trace data stream) into a `TraceSearchResult`.
pub(crate) fn convert_trace_search_result(resp: EsSearchResponse) -> TraceSearchResult {
    let total_count = resp.hits.total.value as usize;
    let is_complete = if resp.hits.total.relation == "gte" {
        Some(false)
    } else {
        Some(resp.hits.hits.len() >= total_count)
    };
    let spans: Vec<Span> = resp
        .hits
        .hits
        .into_iter()
        .filter_map(convert_span_hit)
        .collect();

    TraceSearchResult {
        spans,
        total_count,
        is_complete,
        cursor: None,
    }
}

/// Convert an Elasticsearch search response into a `TraceDetail` for a single trace.
///
/// Assumes all hits belong to the same trace (filtered by `trace_id` in the query).
pub(crate) fn convert_trace_detail(resp: EsSearchResponse, trace_id: &str) -> Option<TraceDetail> {
    let spans: Vec<Span> = resp
        .hits
        .hits
        .into_iter()
        .filter_map(convert_span_hit)
        .collect();

    if spans.is_empty() {
        return None;
    }

    Some(TraceDetail::from_spans(trace_id.to_string(), spans))
}

/// Convert a single Elasticsearch span hit into an obz `Span`.
fn convert_span_hit(hit: EsHit) -> Option<Span> {
    let doc: EsSpanDocument = serde_json::from_value(hit.source).ok()?;

    // Parse @timestamp (epoch_millis) to seconds.
    let start_time = doc
        .timestamp
        .as_deref()
        .and_then(parse_epoch_millis_to_seconds)
        .unwrap_or(0);

    // Duration: nanoseconds → microseconds.
    // Falls back to 0 when `duration` is absent (should not happen per OTel spec,
    // but defensive handling avoids panics on malformed documents).
    let duration_us = doc.duration.map(|ns| ns / 1_000).unwrap_or(0);

    let kind = SpanKind::parse(&doc.kind);

    // Handle status: empty object `{}` deserializes with code=None → Unset.
    let status = doc
        .status
        .as_ref()
        .and_then(|s| s.code.as_deref())
        .map(|code| match code {
            "Error" => SpanStatus::Error,
            "Ok" => SpanStatus::Ok,
            _ => SpanStatus::Unset,
        })
        .unwrap_or(SpanStatus::Unset);

    // Extract service name from nested resource.attributes.
    let service = doc
        .resource
        .as_ref()
        .and_then(|r| r.attributes.get("service.name"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let mut attributes = flatten_json_map(&doc.attributes);

    let resource = doc
        .resource
        .as_ref()
        .map(|r| flatten_json_map(&r.attributes))
        .unwrap_or_default();

    // Add error message to attributes if present.
    if let Some(st) = &doc.status {
        if st.code.as_deref() == Some("Error") && !st.message.is_empty() {
            attributes
                .entry("error.message".to_string())
                .or_insert_with(|| st.message.clone());
        }
    }

    Some(Span {
        trace_id: doc.trace_id,
        span_id: doc.span_id,
        parent_span_id: doc.parent_span_id,
        name: doc.name,
        service,
        kind: Some(kind),
        status,
        start_time,
        duration_us,
        attributes: if attributes.is_empty() {
            None
        } else {
            Some(attributes)
        },
        events: None, // ES stores span events as separate log documents.
        resource: if resource.is_empty() {
            None
        } else {
            Some(resource)
        },
        extensions: None,
    })
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Parse an `epoch_millis` timestamp string to Unix epoch seconds.
///
/// Elasticsearch native `OTel` integration stores timestamps as epoch millisecond
/// strings with optional fractional part, e.g., `"1775552455282.107378"`.
///
/// We parse the integer part (milliseconds) and divide by 1000 to get seconds.
fn parse_epoch_millis_to_seconds(s: &str) -> Option<i64> {
    // The string may have a fractional part (e.g., "1775552455282.107378").
    // We only need the integer part of the milliseconds for second-level precision.
    let int_part = if let Some(dot_pos) = s.find('.') {
        &s[..dot_pos]
    } else {
        s
    };
    int_part.parse::<i64>().ok().map(|ms| ms / 1_000)
}

/// Flatten a JSON map to string key-value pairs.
///
/// Skips `data_stream` keys (internal Elasticsearch metadata).
fn flatten_json_map(map: &BTreeMap<String, serde_json::Value>) -> BTreeMap<String, String> {
    map.iter()
        .filter(|(k, _)| !k.starts_with("data_stream"))
        .map(|(k, v)| (k.clone(), crate::util::json_value_to_string(v)))
        .collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::elasticsearch::response::{EsHit, EsHits, EsSearchResponse, EsTotal};

    fn fixture_dir(signal: &str) -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join(format!("../../fixtures/elasticsearch/{signal}"))
    }

    fn load_fixture(signal: &str, name: &str) -> serde_json::Value {
        let path = fixture_dir(signal).join(name);
        let content = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("fixture {}: {e}", path.display()));
        serde_json::from_str(&content).expect("invalid fixture JSON")
    }

    fn get_body(f: &serde_json::Value) -> &serde_json::Value {
        &f["response"]["body"]
    }

    // --- Timestamp parsing tests ---

    #[test]
    fn parse_epoch_millis_integer() {
        assert_eq!(
            parse_epoch_millis_to_seconds("1775552455282"),
            Some(1_775_552_455)
        );
    }

    #[test]
    fn parse_epoch_millis_with_fractional() {
        assert_eq!(
            parse_epoch_millis_to_seconds("1775552455282.107378"),
            Some(1_775_552_455)
        );
    }

    #[test]
    fn parse_epoch_millis_zero() {
        assert_eq!(parse_epoch_millis_to_seconds("0"), Some(0));
    }

    #[test]
    fn parse_epoch_millis_invalid() {
        assert_eq!(parse_epoch_millis_to_seconds("invalid"), None);
        assert_eq!(parse_epoch_millis_to_seconds(""), None);
    }

    // --- Log tests ---

    #[test]
    fn fixture_logs_search_all() {
        let f = load_fixture("logs", "search-all.json");
        let resp: EsSearchResponse = serde_json::from_value(get_body(&f).clone()).unwrap();
        let result = convert_log_search_result(resp);
        assert!(!result.entries.is_empty(), "should have log entries");
        assert!(result.total_count > 0);
        for entry in &result.entries {
            assert!(entry.timestamp > 0, "timestamp should be positive");
            // Some ES log entries may lack body.text (e.g., exception events).
            // Just verify we can parse them without panicking.
        }
    }

    #[test]
    fn fixture_logs_search_empty() {
        let f = load_fixture("logs", "search-empty.json");
        let resp: EsSearchResponse = serde_json::from_value(get_body(&f).clone()).unwrap();
        let result = convert_log_search_result(resp);
        assert!(result.entries.is_empty());
        assert_eq!(result.total_count, 0);
    }

    #[test]
    fn fixture_logs_search_by_service() {
        let f = load_fixture("logs", "search-by-service.json");
        let resp: EsSearchResponse = serde_json::from_value(get_body(&f).clone()).unwrap();
        let result = convert_log_search_result(resp);
        assert!(!result.entries.is_empty());
        for entry in &result.entries {
            assert!(entry.service.is_some(), "should have service");
        }
    }

    #[test]
    fn fixture_logs_search_by_severity() {
        let f = load_fixture("logs", "search-by-severity.json");
        let resp: EsSearchResponse = serde_json::from_value(get_body(&f).clone()).unwrap();
        let result = convert_log_search_result(resp);
        assert!(!result.entries.is_empty());
    }

    #[test]
    fn fixture_logs_search_fulltext() {
        let f = load_fixture("logs", "search-fulltext.json");
        let resp: EsSearchResponse = serde_json::from_value(get_body(&f).clone()).unwrap();
        let result = convert_log_search_result(resp);
        assert!(!result.entries.is_empty());
    }

    #[test]
    fn fixture_logs_search_by_trace_id() {
        let f = load_fixture("logs", "search-by-trace-id.json");
        let resp: EsSearchResponse = serde_json::from_value(get_body(&f).clone()).unwrap();
        let result = convert_log_search_result(resp);
        assert!(!result.entries.is_empty());
        for entry in &result.entries {
            assert!(entry.trace_id.is_some(), "should have trace_id");
        }
    }

    #[test]
    fn fixture_logs_body_text_extraction() {
        let f = load_fixture("logs", "search-all.json");
        let resp: EsSearchResponse = serde_json::from_value(get_body(&f).clone()).unwrap();
        let result = convert_log_search_result(resp);
        // At least some entries should have non-empty messages (from body.text).
        let has_message = result.entries.iter().any(|e| !e.message.is_empty());
        assert!(has_message, "at least one entry should have body.text");
    }

    #[test]
    fn fixture_logs_severity_text_parsed() {
        let f = load_fixture("logs", "search-by-severity.json");
        let resp: EsSearchResponse = serde_json::from_value(get_body(&f).clone()).unwrap();
        let result = convert_log_search_result(resp);
        let has_severity = result.entries.iter().any(|e| e.severity.is_some());
        assert!(has_severity, "at least one entry should have severity");
    }

    // --- Trace tests ---

    #[test]
    fn fixture_traces_search_all() {
        let f = load_fixture("traces", "search-all.json");
        let resp: EsSearchResponse = serde_json::from_value(get_body(&f).clone()).unwrap();
        let result = convert_trace_search_result(resp);
        assert!(!result.spans.is_empty(), "should have spans");
        assert!(result.total_count > 0);
        for span in &result.spans {
            assert!(!span.trace_id.is_empty(), "trace_id should not be empty");
            assert!(!span.span_id.is_empty(), "span_id should not be empty");
            assert!(!span.name.is_empty(), "name should not be empty");
        }
    }

    #[test]
    fn fixture_traces_search_empty() {
        let f = load_fixture("traces", "search-empty.json");
        let resp: EsSearchResponse = serde_json::from_value(get_body(&f).clone()).unwrap();
        let result = convert_trace_search_result(resp);
        assert!(result.spans.is_empty());
        assert_eq!(result.total_count, 0);
    }

    #[test]
    fn fixture_traces_search_by_service() {
        let f = load_fixture("traces", "search-by-service.json");
        let resp: EsSearchResponse = serde_json::from_value(get_body(&f).clone()).unwrap();
        let result = convert_trace_search_result(resp);
        assert!(!result.spans.is_empty());
        for span in &result.spans {
            assert!(!span.service.is_empty(), "service should not be empty");
        }
    }

    #[test]
    fn fixture_traces_search_by_error() {
        let f = load_fixture("traces", "search-by-error.json");
        let resp: EsSearchResponse = serde_json::from_value(get_body(&f).clone()).unwrap();
        let result = convert_trace_search_result(resp);
        assert!(!result.spans.is_empty());
        let has_error = result.spans.iter().any(|s| s.status == SpanStatus::Error);
        assert!(has_error, "should have spans with error status");
    }

    #[test]
    fn fixture_traces_search_by_operation() {
        let f = load_fixture("traces", "search-by-operation.json");
        let resp: EsSearchResponse = serde_json::from_value(get_body(&f).clone()).unwrap();
        let result = convert_trace_search_result(resp);
        assert!(!result.spans.is_empty());
    }

    #[test]
    fn fixture_traces_search_by_kind() {
        let f = load_fixture("traces", "search-by-kind.json");
        let resp: EsSearchResponse = serde_json::from_value(get_body(&f).clone()).unwrap();
        let result = convert_trace_search_result(resp);
        assert!(!result.spans.is_empty());
    }

    #[test]
    fn fixture_traces_duration_nanoseconds() {
        let f = load_fixture("traces", "search-all.json");
        let resp: EsSearchResponse = serde_json::from_value(get_body(&f).clone()).unwrap();
        let result = convert_trace_search_result(resp);
        // At least one span should have a positive duration.
        let has_duration = result.spans.iter().any(|s| s.duration_us > 0);
        assert!(
            has_duration,
            "at least one span should have positive duration_us"
        );
    }

    #[test]
    fn fixture_traces_unset_status_from_empty_object() {
        let f = load_fixture("traces", "search-all.json");
        let resp: EsSearchResponse = serde_json::from_value(get_body(&f).clone()).unwrap();
        let result = convert_trace_search_result(resp);
        // The fixture contains spans with `"status": {}` which should map to Unset.
        let has_unset = result.spans.iter().any(|s| s.status == SpanStatus::Unset);
        assert!(
            has_unset,
            "should have spans with Unset status (from empty object)"
        );
    }

    // --- Trace detail tests ---

    #[test]
    fn fixture_trace_detail_by_id() {
        let f = load_fixture("traces", "trace-by-id.json");
        let resp: EsSearchResponse = serde_json::from_value(get_body(&f).clone()).unwrap();
        let detail = convert_trace_detail(resp, "2499b2eb00bb611f8635341cd1c49afa");
        assert!(detail.is_some(), "should produce a TraceDetail");
        let detail = detail.unwrap();
        assert_eq!(detail.trace_id, "2499b2eb00bb611f8635341cd1c49afa");
        assert!(detail.span_count > 0);
        assert!(detail.service_count > 0);
        assert!(!detail.services.is_empty());
    }

    #[test]
    fn fixture_trace_detail_not_found() {
        let f = load_fixture("traces", "trace-by-id-not-found.json");
        let resp: EsSearchResponse = serde_json::from_value(get_body(&f).clone()).unwrap();
        let detail = convert_trace_detail(resp, "00000000000000000000000000000000");
        assert!(detail.is_none(), "empty search should return None");
    }

    #[test]
    fn fixture_trace_detail_from_search() {
        let f = load_fixture("traces", "search-by-service.json");
        let resp: EsSearchResponse = serde_json::from_value(get_body(&f).clone()).unwrap();
        // Pick the trace_id from the first hit.
        let first_trace_id = {
            let raw: EsSearchResponse = serde_json::from_value(get_body(&f).clone()).unwrap();
            let hit = &raw.hits.hits[0];
            let doc: EsSpanDocument = serde_json::from_value(hit.source.clone()).unwrap();
            doc.trace_id
        };
        let detail = convert_trace_detail(resp, &first_trace_id);
        assert!(detail.is_some(), "should produce a TraceDetail");
        let detail = detail.unwrap();
        assert_eq!(detail.trace_id, first_trace_id);
        assert!(detail.span_count > 0);
        // Verify spans are sorted by start_time ascending.
        let times: Vec<i64> = detail.spans.iter().map(|s| s.start_time).collect();
        let mut sorted = times.clone();
        sorted.sort();
        assert_eq!(times, sorted, "spans should be sorted by start_time asc");
    }

    // --- Error response deserialization tests ---

    #[test]
    fn fixture_logs_error_syntax_deserializes() {
        let f = load_fixture("logs", "error-syntax.json");
        let body = get_body(&f);
        let err: crate::elasticsearch::response::EsErrorResponse =
            serde_json::from_value(body.clone()).unwrap();
        assert!(!err.error.error_type.is_empty());
        assert!(!err.error.reason.is_empty());
        assert_eq!(err.status, 400);
    }

    #[test]
    fn fixture_traces_error_syntax_deserializes() {
        let f = load_fixture("traces", "error-syntax.json");
        let body = get_body(&f);
        let err: crate::elasticsearch::response::EsErrorResponse =
            serde_json::from_value(body.clone()).unwrap();
        assert!(!err.error.error_type.is_empty());
        assert!(!err.error.reason.is_empty());
        assert_eq!(err.status, 400);
    }

    #[test]
    fn fixture_logs_error_index_not_found_deserializes() {
        let f = load_fixture("logs", "error-index-not-found.json");
        let body = get_body(&f);
        let err: crate::elasticsearch::response::EsErrorResponse =
            serde_json::from_value(body.clone()).unwrap();
        assert_eq!(err.error.error_type, "index_not_found_exception");
    }

    #[test]
    fn fixture_logs_error_field_type_deserializes() {
        let f = load_fixture("logs", "error-field-type.json");
        let body = get_body(&f);
        let err: crate::elasticsearch::response::EsErrorResponse =
            serde_json::from_value(body.clone()).unwrap();
        assert!(!err.error.error_type.is_empty());
        assert!(!err.error.reason.is_empty());
    }

    #[test]
    fn fixture_traces_error_sort_unmapped_deserializes() {
        let f = load_fixture("traces", "error-sort-unmapped.json");
        let body = get_body(&f);
        let err: crate::elasticsearch::response::EsErrorResponse =
            serde_json::from_value(body.clone()).unwrap();
        assert!(!err.error.error_type.is_empty());
        assert!(!err.error.reason.is_empty());
    }

    // --- Duration edge case tests ---

    #[test]
    fn fixture_traces_search_by_duration() {
        let f = load_fixture("traces", "search-by-duration.json");
        let resp: EsSearchResponse = serde_json::from_value(get_body(&f).clone()).unwrap();
        let result = convert_trace_search_result(resp);
        assert!(!result.spans.is_empty());
        // search-by-duration contains slow spans (duration > 1s = 1_000_000_000 ns).
        // Verify large durations are correctly converted to microseconds.
        let has_long_duration = result.spans.iter().any(|s| s.duration_us > 1_000_000);
        assert!(
            has_long_duration,
            "should have spans with duration > 1 second"
        );
    }

    // --- is_complete tests ---

    /// Helper to build a minimal `EsSearchResponse` for `is_complete` tests.
    fn make_search_response(
        total_value: u64,
        relation: &str,
        hit_count: usize,
    ) -> EsSearchResponse {
        let hits: Vec<EsHit> = (0..hit_count)
            .map(|i| EsHit {
                index: "test".to_string(),
                id: format!("hit-{i}"),
                source: serde_json::json!({
                    "body": { "text": "test message" },
                    "severity_text": "INFO",
                    "@timestamp": "1775552455000"
                }),
            })
            .collect();
        EsSearchResponse {
            hits: EsHits {
                total: EsTotal {
                    value: total_value,
                    relation: relation.to_string(),
                },
                hits,
            },
        }
    }

    #[test]
    fn is_complete_fixture_truncated() {
        // Fixture has total=7046 but only 10 hits (relation="eq"),
        // so is_complete should be Some(false) — result is truncated.
        let f = load_fixture("logs", "search-all.json");
        let resp: EsSearchResponse = serde_json::from_value(get_body(&f).clone()).unwrap();
        let result = convert_log_search_result(resp);
        assert_eq!(
            result.is_complete,
            Some(false),
            "fixture with fewer hits than total should be incomplete"
        );
    }

    #[test]
    fn is_complete_eq_relation_all_hits_returned() {
        // When relation is "eq" and hits.len() >= total, result is complete.
        let resp = make_search_response(3, "eq", 3);
        let result = convert_log_search_result(resp);
        assert_eq!(
            result.is_complete,
            Some(true),
            "all hits returned should be complete"
        );
    }

    #[test]
    fn is_complete_gte_relation_returns_false() {
        // When relation is "gte", total is a lower bound — always report
        // Some(false) regardless of hit count.
        let resp = make_search_response(5, "gte", 5);
        let result = convert_log_search_result(resp);
        assert_eq!(
            result.is_complete,
            Some(false),
            "relation=gte should always report incomplete"
        );
    }

    #[test]
    fn is_complete_eq_relation_fewer_hits_than_total() {
        // When relation is "eq" but hits < total, the result is incomplete.
        let resp = make_search_response(100, "eq", 10);
        let result = convert_log_search_result(resp);
        assert_eq!(
            result.is_complete,
            Some(false),
            "fewer hits than total should report incomplete"
        );
    }

    #[test]
    fn is_complete_trace_fixture_truncated() {
        // Trace fixture has total=5603 but only 10 hits (relation="eq").
        let f = load_fixture("traces", "search-all.json");
        let resp: EsSearchResponse = serde_json::from_value(get_body(&f).clone()).unwrap();
        let result = convert_trace_search_result(resp);
        assert_eq!(
            result.is_complete,
            Some(false),
            "trace fixture with fewer hits than total should be incomplete"
        );
    }
}
