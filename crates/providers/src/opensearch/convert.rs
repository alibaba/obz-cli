//! `OpenSearch` API response → obz model conversion.
//!
//! Converts `OpenSearch` `_search` responses into obz unified data models
//! for both log and trace indices.

use std::collections::BTreeMap;
use std::str::FromStr;

use obz_core::model::log::{parse_severity, severity_from_otel_number, LogEntry};
use obz_core::model::trace::{Span, SpanEvent, SpanKind, SpanStatus, TraceDetail};
use obz_core::provider::results::{LogSearchResult, TraceSearchResult};

use super::response::{OsHit, OsLogDocument, OsSearchResponse, OsSpanDocument};

// ---------------------------------------------------------------------------
// Log conversion
// ---------------------------------------------------------------------------

/// Convert an `OpenSearch` search response (from a log index) into a `LogSearchResult`.
pub(crate) fn convert_log_search_result(resp: OsSearchResponse) -> LogSearchResult {
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

/// Convert a single `OpenSearch` log hit into an obz `LogEntry`.
fn convert_log_hit(hit: OsHit) -> Option<LogEntry> {
    let doc: OsLogDocument = serde_json::from_value(hit.source).ok()?;

    let timestamp = doc
        .timestamp
        .as_deref()
        .or(doc.observed_timestamp.as_deref())
        .and_then(parse_rfc3339_to_epoch)
        .unwrap_or(0);

    let severity = doc
        .severity
        .as_ref()
        .and_then(|s| s.text.as_deref())
        .map(parse_severity)
        .or_else(|| {
            doc.severity
                .as_ref()
                .and_then(|s| s.number)
                .and_then(severity_from_otel_number)
        });

    let service = doc
        .resource
        .get("service.name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    // Flatten attributes to string map, filtering out internal/data_stream keys.
    let attributes = flatten_json_map(&doc.attributes);
    let resource = flatten_json_map(&doc.resource);

    Some(LogEntry {
        timestamp,
        message: doc.body,
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

/// Convert an `OpenSearch` search response (from a trace index) into a `TraceSearchResult`.
pub(crate) fn convert_trace_search_result(resp: OsSearchResponse) -> TraceSearchResult {
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

/// Convert an `OpenSearch` search response into a `TraceDetail` for a single trace.
///
/// Assumes all hits belong to the same trace (filtered by `traceId` in the query).
pub(crate) fn convert_trace_detail(resp: OsSearchResponse, trace_id: &str) -> Option<TraceDetail> {
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

/// Convert a single `OpenSearch` span hit into an obz `Span`.
fn convert_span_hit(hit: OsHit) -> Option<Span> {
    let doc: OsSpanDocument = serde_json::from_value(hit.source).ok()?;

    // Use microsecond precision for duration calculation — second-level
    // precision would round most span durations to zero.
    let start_us = doc
        .start_time
        .as_deref()
        .and_then(parse_rfc3339_to_epoch_us)
        .unwrap_or(0);

    let end_us = doc
        .end_time
        .as_deref()
        .and_then(parse_rfc3339_to_epoch_us)
        .unwrap_or(start_us);

    let duration_us = (end_us - start_us).max(0);
    // Span.start_time is in seconds.
    let start_time = start_us / 1_000_000;

    let kind = SpanKind::parse(&doc.kind);

    let status = doc
        .status
        .as_ref()
        .map(|s| match s.code.as_str() {
            "Error" => SpanStatus::Error,
            "Ok" => SpanStatus::Ok,
            _ => SpanStatus::Unset,
        })
        .unwrap_or(SpanStatus::Unset);

    let service = doc
        .resource
        .get("service.name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let parent_span_id = if doc.parent_span_id.is_empty() {
        None
    } else {
        Some(doc.parent_span_id)
    };

    let mut attributes = flatten_json_map(&doc.attributes);
    let resource = flatten_json_map(&doc.resource);

    // Add error message to attributes if present.
    if let Some(st) = &doc.status {
        if st.code == "Error" && !st.message.is_empty() {
            attributes
                .entry("error.message".to_string())
                .or_insert_with(|| st.message.clone());
        }
    }

    // Convert span events.
    let events: Vec<SpanEvent> = doc
        .events
        .into_iter()
        .map(|e| {
            let event_ts = e
                .timestamp
                .as_deref()
                .and_then(parse_rfc3339_to_epoch)
                .unwrap_or(start_time);
            // SpanEvent.timestamp is in seconds (not microseconds).
            let event_attrs = flatten_json_map(&e.attributes);
            SpanEvent {
                name: if e.name.is_empty() {
                    "event".to_string()
                } else {
                    e.name
                },
                timestamp: event_ts,
                attributes: if event_attrs.is_empty() {
                    None
                } else {
                    Some(event_attrs)
                },
            }
        })
        .collect();

    Some(Span {
        trace_id: doc.trace_id,
        span_id: doc.span_id,
        parent_span_id,
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
    })
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Parse an RFC 3339 timestamp to Unix epoch seconds.
fn parse_rfc3339_to_epoch(s: &str) -> Option<i64> {
    jiff::Timestamp::from_str(s)
        .ok()
        .map(jiff::Timestamp::as_second)
}

/// Parse an RFC 3339 timestamp to Unix epoch **microseconds**.
///
/// Used for span duration calculation where sub-second precision is critical.
fn parse_rfc3339_to_epoch_us(s: &str) -> Option<i64> {
    jiff::Timestamp::from_str(s)
        .ok()
        .map(jiff::Timestamp::as_microsecond)
}

/// Flatten a JSON map to string key-value pairs.
///
/// Skips `data_stream` keys (internal `OpenSearch` metadata).
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
    use crate::opensearch::response::{OsHit, OsHits, OsSearchResponse, OsTotal};

    fn fixture_dir(signal: &str) -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join(format!("../../fixtures/opensearch/{signal}"))
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

    // --- Log tests ---

    #[test]
    fn fixture_logs_search_all() {
        let f = load_fixture("logs", "search-all.json");
        let resp: OsSearchResponse = serde_json::from_value(get_body(&f).clone()).unwrap();
        let result = convert_log_search_result(resp);
        assert!(!result.entries.is_empty(), "should have log entries");
        assert!(result.total_count > 0);
        for entry in &result.entries {
            assert!(entry.timestamp > 0, "timestamp should be positive");
            assert!(!entry.message.is_empty(), "message should not be empty");
        }
    }

    #[test]
    fn fixture_logs_search_empty() {
        let f = load_fixture("logs", "search-empty.json");
        let resp: OsSearchResponse = serde_json::from_value(get_body(&f).clone()).unwrap();
        let result = convert_log_search_result(resp);
        assert!(result.entries.is_empty());
        assert_eq!(result.total_count, 0);
    }

    #[test]
    fn fixture_logs_search_by_service() {
        let f = load_fixture("logs", "search-by-service.json");
        let resp: OsSearchResponse = serde_json::from_value(get_body(&f).clone()).unwrap();
        let result = convert_log_search_result(resp);
        assert!(!result.entries.is_empty());
        for entry in &result.entries {
            assert!(entry.service.is_some(), "should have service");
        }
    }

    #[test]
    fn fixture_logs_search_by_severity() {
        let f = load_fixture("logs", "search-by-severity.json");
        let resp: OsSearchResponse = serde_json::from_value(get_body(&f).clone()).unwrap();
        let result = convert_log_search_result(resp);
        assert!(!result.entries.is_empty());
    }

    #[test]
    fn fixture_logs_search_fulltext() {
        let f = load_fixture("logs", "search-fulltext.json");
        let resp: OsSearchResponse = serde_json::from_value(get_body(&f).clone()).unwrap();
        let result = convert_log_search_result(resp);
        assert!(!result.entries.is_empty());
    }

    // --- Trace tests ---

    #[test]
    fn fixture_traces_search_all() {
        let f = load_fixture("traces", "search-all.json");
        let resp: OsSearchResponse = serde_json::from_value(get_body(&f).clone()).unwrap();
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
        let resp: OsSearchResponse = serde_json::from_value(get_body(&f).clone()).unwrap();
        let result = convert_trace_search_result(resp);
        assert!(result.spans.is_empty());
        assert_eq!(result.total_count, 0);
    }

    #[test]
    fn fixture_traces_search_by_service() {
        let f = load_fixture("traces", "search-by-service.json");
        let resp: OsSearchResponse = serde_json::from_value(get_body(&f).clone()).unwrap();
        let result = convert_trace_search_result(resp);
        assert!(!result.spans.is_empty());
        for span in &result.spans {
            assert!(!span.service.is_empty(), "service should not be empty");
        }
    }

    #[test]
    fn fixture_traces_search_by_error() {
        let f = load_fixture("traces", "search-by-error.json");
        let resp: OsSearchResponse = serde_json::from_value(get_body(&f).clone()).unwrap();
        let result = convert_trace_search_result(resp);
        assert!(!result.spans.is_empty());
        let has_error = result.spans.iter().any(|s| s.status == SpanStatus::Error);
        assert!(has_error, "should have spans with error status");
    }

    #[test]
    fn fixture_traces_search_by_operation() {
        let f = load_fixture("traces", "search-by-operation.json");
        let resp: OsSearchResponse = serde_json::from_value(get_body(&f).clone()).unwrap();
        let result = convert_trace_search_result(resp);
        assert!(!result.spans.is_empty());
    }

    // --- Trace detail tests ---

    #[test]
    fn fixture_trace_detail_from_search() {
        let f = load_fixture("traces", "search-by-service.json");
        let resp: OsSearchResponse = serde_json::from_value(get_body(&f).clone()).unwrap();
        // Pick the trace_id from the first hit.
        let first_trace_id = {
            let raw: OsSearchResponse = serde_json::from_value(get_body(&f).clone()).unwrap();
            let hit = &raw.hits.hits[0];
            let doc: OsSpanDocument = serde_json::from_value(hit.source.clone()).unwrap();
            doc.trace_id
        };
        let detail = convert_trace_detail(resp, &first_trace_id);
        assert!(detail.is_some(), "should produce a TraceDetail");
        let detail = detail.unwrap();
        assert_eq!(detail.trace_id, first_trace_id);
        assert!(detail.span_count > 0);
        assert!(detail.service_count > 0);
        assert!(!detail.services.is_empty());
        // Verify spans are sorted by start_time ascending.
        let times: Vec<i64> = detail.spans.iter().map(|s| s.start_time).collect();
        let mut sorted = times.clone();
        sorted.sort();
        assert_eq!(times, sorted, "spans should be sorted by start_time asc");
    }

    #[test]
    fn trace_detail_empty_returns_none() {
        let f = load_fixture("traces", "search-empty.json");
        let resp: OsSearchResponse = serde_json::from_value(get_body(&f).clone()).unwrap();
        let detail = convert_trace_detail(resp, "nonexistent");
        assert!(detail.is_none(), "empty search should return None");
    }

    // --- Error response deserialization tests ---

    #[test]
    fn fixture_logs_error_syntax_deserializes() {
        let f = load_fixture("logs", "error-syntax.json");
        let body = get_body(&f);
        let err: crate::opensearch::response::OsErrorResponse =
            serde_json::from_value(body.clone()).unwrap();
        assert!(!err.error.error_type.is_empty());
        assert!(!err.error.reason.is_empty());
        assert_eq!(err.status, 400);
    }

    #[test]
    fn fixture_traces_error_syntax_deserializes() {
        let f = load_fixture("traces", "error-syntax.json");
        let body = get_body(&f);
        let err: crate::opensearch::response::OsErrorResponse =
            serde_json::from_value(body.clone()).unwrap();
        assert!(!err.error.error_type.is_empty());
        assert!(!err.error.reason.is_empty());
        assert_eq!(err.status, 400);
    }

    // --- is_complete tests ---

    /// Helper to build a minimal `OsSearchResponse` for `is_complete` tests.
    fn make_search_response(
        total_value: u64,
        relation: &str,
        hit_count: usize,
    ) -> OsSearchResponse {
        let hits: Vec<OsHit> = (0..hit_count)
            .map(|i| OsHit {
                index: "test".to_string(),
                id: format!("hit-{i}"),
                source: serde_json::json!({
                    "@timestamp": "2026-03-27T09:58:24.141420111Z",
                    "body": "test message",
                    "severity": { "text": "INFO" }
                }),
            })
            .collect();
        OsSearchResponse {
            hits: OsHits {
                total: OsTotal {
                    value: total_value,
                    relation: relation.to_string(),
                },
                hits,
            },
        }
    }

    #[test]
    fn is_complete_fixture_truncated() {
        // OS log fixture has total=10000, relation="gte", 10 hits.
        // relation="gte" → always Some(false).
        let f = load_fixture("logs", "search-all.json");
        let resp: OsSearchResponse = serde_json::from_value(get_body(&f).clone()).unwrap();
        let result = convert_log_search_result(resp);
        assert_eq!(
            result.is_complete,
            Some(false),
            "fixture with relation=gte should be incomplete"
        );
    }

    #[test]
    fn is_complete_eq_relation_all_hits_returned() {
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
        // OS trace fixture has total=6882, relation="eq", 10 hits.
        let f = load_fixture("traces", "search-all.json");
        let resp: OsSearchResponse = serde_json::from_value(get_body(&f).clone()).unwrap();
        let result = convert_trace_search_result(resp);
        assert_eq!(
            result.is_complete,
            Some(false),
            "trace fixture with fewer hits than total should be incomplete"
        );
    }
}
