//! SLS → obz model conversion functions.
//!
//! Converts SLS `GetLogs` API responses into obz unified data models.
//!
//! # Log conversion
//!
//! - `content` → `message`
//! - `timeUnixNano` (nanosecond string) → Unix seconds
//! - `severityText` → `parse_severity()`
//! - `host` → `source`
//! - `service` → `service`
//! - `attribute` (JSON string) → parsed into `attributes`
//! - `resource` (JSON string) → parsed into `resource`
//! - `traceID`/`spanID` → `trace_id`/`span_id` (empty strings filtered)
//!
//! # Trace conversion
//!
//! - `start`/`end` (microsecond strings) → timestamps
//! - `duration` (microsecond string) → `duration_us`
//! - `statusCode` → `SpanStatus`
//! - `logs` (JSON string) → `Vec<SpanEvent>`

use std::collections::BTreeMap;

use obz_core::model::log::{parse_severity, severity_from_otel_number, LogEntry};
use obz_core::model::trace::{Span, SpanEvent, SpanKind, SpanStatus, TraceDetail};
use obz_core::provider::results::{LogSearchResult, TraceSearchResult};

use super::response::{field_as_str, parse_json_string_field, parse_span_logs, SlsEntry};

// ---------------------------------------------------------------------------
// Log conversion
// ---------------------------------------------------------------------------

/// System fields extracted into dedicated `LogEntry` fields.
/// These should NOT appear in `attributes`.
const LOG_SYSTEM_FIELDS: &[&str] = &[
    "content",
    "timeUnixNano",
    "severityNumber",
    "severityText",
    "host",
    "service",
    "resource",
    "attribute",
    "traceID",
    "spanID",
    "flags",
    "otlp.name",
    "otlp.version",
    "__topic__",
    "__source__",
    "__time__",
];

/// Convert SLS log entries to obz `LogSearchResult`.
pub(crate) fn convert_log_entries(entries: &[SlsEntry], is_complete: bool) -> LogSearchResult {
    let total_count = entries.len();
    let log_entries: Vec<LogEntry> = entries.iter().map(convert_log_entry).collect();
    LogSearchResult {
        entries: log_entries,
        total_count,
        is_complete: Some(is_complete),
        cursor: None,
    }
}

/// Convert a single SLS log entry to an obz `LogEntry`.
fn convert_log_entry(entry: &SlsEntry) -> LogEntry {
    // Extract timestamp: timeUnixNano is a nanosecond string.
    let timestamp = entry
        .get("timeUnixNano")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<u64>().ok())
        .map(|ns| (ns / 1_000_000_000) as i64)
        .unwrap_or(0);

    // Extract message from `content`.
    let message = entry.get("content").map(field_as_str).unwrap_or_default();

    // Extract severity from `severityText`.
    let severity = entry
        .get("severityText")
        .and_then(|v| v.as_str())
        .map(parse_severity)
        .or_else(|| {
            entry.get("severityNumber").and_then(|v| {
                v.as_u64()
                    .map(|n| n as u32)
                    .or_else(|| v.as_str().and_then(|s| s.parse::<u32>().ok()))
                    .and_then(severity_from_otel_number)
            })
        });

    // Extract source from `host`.
    let source = entry
        .get("host")
        .and_then(|v| v.as_str())
        .map(str::to_string);

    // Extract service.
    let service = entry
        .get("service")
        .and_then(|v| v.as_str())
        .map(str::to_string);

    // Extract trace/span IDs (filter empty strings).
    let trace_id = entry
        .get("traceID")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let span_id = entry
        .get("spanID")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    // Parse `attribute` JSON string → attributes.
    let mut attributes: BTreeMap<String, String> = entry
        .get("attribute")
        .map(parse_json_string_field)
        .unwrap_or_default();

    // Add non-system fields to attributes.
    for (k, v) in entry {
        if !LOG_SYSTEM_FIELDS.contains(&k.as_str()) {
            attributes.insert(k.clone(), field_as_str(v));
        }
    }

    let attributes = if attributes.is_empty() {
        None
    } else {
        Some(attributes)
    };

    // Parse `resource` JSON string → resource.
    let resource: Option<BTreeMap<String, String>> = entry
        .get("resource")
        .map(parse_json_string_field)
        .filter(|m| !m.is_empty());

    LogEntry {
        timestamp,
        message,
        severity,
        source,
        service,
        id: None,
        attributes,
        resource,
        trace_id,
        span_id,
        extensions: None,
    }
}

// ---------------------------------------------------------------------------
// Trace conversion
// ---------------------------------------------------------------------------

/// System fields in SLS trace entries that should NOT appear in `attributes`.
const TRACE_SYSTEM_FIELDS: &[&str] = &[
    "traceID",
    "spanID",
    "parentSpanID",
    "name",
    "service",
    "kind",
    "start",
    "end",
    "duration",
    "attribute",
    "resource",
    "statusCode",
    "statusMessage",
    "logs",
    "links",
    "host",
    "otlp.name",
    "otlp.version",
    "traceState",
    "__topic__",
];

/// Convert SLS trace entries to obz `TraceSearchResult`.
pub(crate) fn convert_trace_entries(entries: &[SlsEntry], is_complete: bool) -> TraceSearchResult {
    let spans: Vec<Span> = entries.iter().map(convert_trace_entry).collect();
    let total = spans.len();
    TraceSearchResult {
        spans,
        total_count: total,
        is_complete: Some(is_complete),
        cursor: None,
    }
}

/// Convert SLS trace entries for a specific trace into a `TraceDetail`.
pub(crate) fn convert_trace_detail(entries: &[SlsEntry], trace_id: &str) -> TraceDetail {
    let spans: Vec<Span> = entries.iter().map(convert_trace_entry).collect();
    TraceDetail::from_spans(trace_id.to_string(), spans)
}

/// Convert a single SLS trace entry to an obz `Span`.
fn convert_trace_entry(entry: &SlsEntry) -> Span {
    let trace_id = entry.get("traceID").map(field_as_str).unwrap_or_default();
    let span_id = entry.get("spanID").map(field_as_str).unwrap_or_default();
    let parent_span_id = entry
        .get("parentSpanID")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let name = entry.get("name").map(field_as_str).unwrap_or_default();
    let service = entry.get("service").map(field_as_str).unwrap_or_default();

    // Parse span kind.
    let kind = entry
        .get("kind")
        .and_then(|v| v.as_str())
        .map(SpanKind::parse);

    // Parse start time: microsecond string → Unix seconds.
    let start_us = entry
        .get("start")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(0);
    let start_time = start_us / 1_000_000;

    // Parse duration: microsecond string → i64.
    let duration_us = entry
        .get("duration")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(0);

    // Parse status code.
    let status = entry
        .get("statusCode")
        .and_then(|v| v.as_str())
        .map(parse_span_status)
        .unwrap_or(SpanStatus::Unset);

    // Parse attributes from `attribute` JSON string.
    let mut attributes: BTreeMap<String, String> = entry
        .get("attribute")
        .map(parse_json_string_field)
        .unwrap_or_default();

    // Add statusMessage to attributes if present and non-empty.
    if let Some(msg) = entry.get("statusMessage").and_then(|v| v.as_str()) {
        if !msg.is_empty() {
            attributes.insert("status.message".to_string(), msg.to_string());
        }
    }

    // Add non-system fields to attributes.
    for (k, v) in entry {
        if !TRACE_SYSTEM_FIELDS.contains(&k.as_str()) {
            attributes.insert(k.clone(), field_as_str(v));
        }
    }

    let attributes = if attributes.is_empty() {
        None
    } else {
        Some(attributes)
    };

    // Parse resource from `resource` JSON string.
    let resource: Option<BTreeMap<String, String>> = entry
        .get("resource")
        .map(parse_json_string_field)
        .filter(|m| !m.is_empty());

    // Parse span events from `logs` JSON string.
    let raw_logs = entry
        .get("logs")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let span_logs = parse_span_logs(&raw_logs);
    let events: Vec<SpanEvent> = span_logs
        .into_iter()
        .map(|log| {
            let event_name = log
                .attribute
                .as_ref()
                .and_then(|a| a.get("event"))
                .and_then(|v| v.as_str())
                .unwrap_or("log")
                .to_string();
            let event_attrs: BTreeMap<String, String> = log
                .attribute
                .unwrap_or_default()
                .into_iter()
                .filter(|(k, _)| k != "event")
                .map(|(k, v)| (k, field_as_str(&v)))
                .collect();
            SpanEvent {
                name: event_name,
                timestamp: log.timestamp.map(|t| t / 1_000_000).unwrap_or(0),
                attributes: if event_attrs.is_empty() {
                    None
                } else {
                    Some(event_attrs)
                },
            }
        })
        .collect();

    let events = if events.is_empty() {
        None
    } else {
        Some(events)
    };

    Span {
        trace_id,
        span_id,
        parent_span_id,
        name,
        service,
        kind,
        status,
        start_time,
        duration_us,
        attributes,
        events,
        resource,
        extensions: None,
    }
}

/// Parse SLS `statusCode` field to obz `SpanStatus`.
fn parse_span_status(s: &str) -> SpanStatus {
    match s {
        "OK" => SpanStatus::Ok,
        "ERROR" => SpanStatus::Error,
        _ => SpanStatus::Unset,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_log_entry(fields: &[(&str, serde_json::Value)]) -> SlsEntry {
        fields
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
    }

    #[test]
    fn test_convert_log_entry_basic() {
        let entry = make_log_entry(&[
            ("timeUnixNano", serde_json::json!("1774601910631501941")),
            ("content", serde_json::json!("Test log message")),
            ("severityText", serde_json::json!("ERROR")),
            ("host", serde_json::json!("web01")),
            ("service", serde_json::json!("api-server")),
            ("traceID", serde_json::json!("abc123")),
            ("spanID", serde_json::json!("def456")),
            ("attribute", serde_json::json!("{\"key\":\"value\"}")),
            ("resource", serde_json::json!("{\"os.type\":\"linux\"}")),
        ]);

        let log = convert_log_entry(&entry);
        assert_eq!(log.timestamp, 1774601910);
        assert_eq!(log.message, "Test log message");
        assert_eq!(log.severity, Some(obz_core::model::log::Severity::Error));
        assert_eq!(log.source.as_deref(), Some("web01"));
        assert_eq!(log.service.as_deref(), Some("api-server"));
        assert_eq!(log.trace_id.as_deref(), Some("abc123"));
        assert_eq!(log.span_id.as_deref(), Some("def456"));

        let attrs = log.attributes.unwrap();
        assert_eq!(attrs["key"], "value");

        let resource = log.resource.unwrap();
        assert_eq!(resource["os.type"], "linux");
    }

    #[test]
    fn test_convert_log_entry_empty_trace_id() {
        let entry = make_log_entry(&[
            ("timeUnixNano", serde_json::json!("1774601910000000000")),
            ("content", serde_json::json!("msg")),
            ("traceID", serde_json::json!("")),
            ("spanID", serde_json::json!("")),
        ]);

        let log = convert_log_entry(&entry);
        assert!(log.trace_id.is_none());
        assert!(log.span_id.is_none());
    }

    #[test]
    fn test_convert_log_entry_minimal() {
        let entry = make_log_entry(&[
            ("timeUnixNano", serde_json::json!("1774601910000000000")),
            ("content", serde_json::json!("hello")),
        ]);

        let log = convert_log_entry(&entry);
        assert_eq!(log.message, "hello");
        assert!(log.severity.is_none());
        assert!(log.source.is_none());
        assert!(log.service.is_none());
    }

    #[test]
    fn test_convert_log_entries_result() {
        let entries = vec![
            make_log_entry(&[
                ("timeUnixNano", serde_json::json!("1774601910000000000")),
                ("content", serde_json::json!("one")),
            ]),
            make_log_entry(&[
                ("timeUnixNano", serde_json::json!("1774601911000000000")),
                ("content", serde_json::json!("two")),
            ]),
        ];

        let result = convert_log_entries(&entries, true);
        assert_eq!(result.total_count, 2);
        assert_eq!(result.entries.len(), 2);
        assert_eq!(result.is_complete, Some(true));
    }

    #[test]
    fn test_convert_trace_entry_basic() {
        let entry = make_log_entry(&[
            ("traceID", serde_json::json!("abc123")),
            ("spanID", serde_json::json!("def456")),
            ("parentSpanID", serde_json::json!("parent1")),
            ("name", serde_json::json!("GET /api/users")),
            ("service", serde_json::json!("api-server")),
            ("kind", serde_json::json!("server")),
            ("start", serde_json::json!("1774601911124489")),
            ("end", serde_json::json!("1774601911128975")),
            ("duration", serde_json::json!("4485")),
            ("statusCode", serde_json::json!("ERROR")),
            ("statusMessage", serde_json::json!("connection failed")),
            ("attribute", serde_json::json!("{\"http.method\":\"GET\"}")),
            ("resource", serde_json::json!("{\"os.type\":\"linux\"}")),
            ("logs", serde_json::json!("[]")),
        ]);

        let span = convert_trace_entry(&entry);
        assert_eq!(span.trace_id, "abc123");
        assert_eq!(span.span_id, "def456");
        assert_eq!(span.parent_span_id.as_deref(), Some("parent1"));
        assert_eq!(span.name, "GET /api/users");
        assert_eq!(span.service, "api-server");
        assert_eq!(span.kind, Some(SpanKind::Server));
        assert_eq!(span.start_time, 1774601911);
        assert_eq!(span.duration_us, 4485);
        assert_eq!(span.status, SpanStatus::Error);

        let attrs = span.attributes.unwrap();
        assert_eq!(attrs["http.method"], "GET");
        assert_eq!(attrs["status.message"], "connection failed");

        let resource = span.resource.unwrap();
        assert_eq!(resource["os.type"], "linux");
    }

    #[test]
    fn test_convert_trace_entry_empty_parent() {
        let entry = make_log_entry(&[
            ("traceID", serde_json::json!("abc")),
            ("spanID", serde_json::json!("def")),
            ("parentSpanID", serde_json::json!("")),
            ("name", serde_json::json!("root")),
            ("service", serde_json::json!("svc")),
            ("kind", serde_json::json!("internal")),
            ("start", serde_json::json!("1000000")),
            ("duration", serde_json::json!("500")),
            ("statusCode", serde_json::json!("UNSET")),
        ]);

        let span = convert_trace_entry(&entry);
        assert!(span.parent_span_id.is_none());
        assert_eq!(span.kind, Some(SpanKind::Internal));
        assert_eq!(span.status, SpanStatus::Unset);
    }

    #[test]
    fn test_parse_span_kind_values() {
        assert_eq!(SpanKind::parse("client"), SpanKind::Client);
        assert_eq!(SpanKind::parse("server"), SpanKind::Server);
        assert_eq!(SpanKind::parse("producer"), SpanKind::Producer);
        assert_eq!(SpanKind::parse("consumer"), SpanKind::Consumer);
        assert_eq!(SpanKind::parse("internal"), SpanKind::Internal);
        assert_eq!(SpanKind::parse("unknown"), SpanKind::Internal);
    }

    #[test]
    fn test_parse_span_status_values() {
        assert_eq!(parse_span_status("OK"), SpanStatus::Ok);
        assert_eq!(parse_span_status("ERROR"), SpanStatus::Error);
        assert_eq!(parse_span_status("UNSET"), SpanStatus::Unset);
        assert_eq!(parse_span_status("something"), SpanStatus::Unset);
    }

    // --- Fixture-based tests ---

    fn fixture_dir() -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/sls")
    }

    fn load_fixture(subdir: &str, name: &str) -> serde_json::Value {
        let path = fixture_dir().join(subdir).join(name);
        let content = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("fixture {}: {e}", path.display()));
        serde_json::from_str(&content).expect("invalid fixture JSON")
    }

    fn get_body_entries(f: &serde_json::Value) -> Vec<SlsEntry> {
        let body = &f["response"]["body"];
        serde_json::from_value(body.clone()).expect("invalid body array")
    }

    fn get_facts(f: &serde_json::Value) -> &serde_json::Value {
        &f["facts"]
    }

    #[test]
    fn fixture_log_query_all() {
        let f = load_fixture("logs", "log-query-all.json");
        let entries = get_body_entries(&f);
        let expected_count = get_facts(&f)["log_count"].as_u64().unwrap() as usize;

        assert_eq!(entries.len(), expected_count);
        let result = convert_log_entries(&entries, true);
        assert_eq!(result.total_count, expected_count);
        assert_eq!(result.is_complete, Some(true));

        for e in &result.entries {
            assert!(e.timestamp > 0, "timestamp should be positive");
            assert!(!e.message.is_empty(), "message should not be empty");
        }
    }

    #[test]
    fn fixture_log_severity_error() {
        let f = load_fixture("logs", "log-severity-error.json");
        let entries = get_body_entries(&f);
        let result = convert_log_entries(&entries, true);

        for e in &result.entries {
            assert_eq!(
                e.severity,
                Some(obz_core::model::log::Severity::Error),
                "all entries should have ERROR severity"
            );
        }
    }

    #[test]
    fn fixture_log_empty() {
        let f = load_fixture("logs", "log-empty.json");
        let entries = get_body_entries(&f);
        let result = convert_log_entries(&entries, true);
        assert_eq!(result.total_count, 0);
        assert!(result.entries.is_empty());
    }

    #[test]
    fn fixture_log_system_fields_excluded() {
        let f = load_fixture("logs", "log-query-all.json");
        let entries = get_body_entries(&f);
        let result = convert_log_entries(&entries, true);

        for e in &result.entries {
            if let Some(attrs) = &e.attributes {
                assert!(
                    !attrs.contains_key("content"),
                    "content should not be in attributes"
                );
                assert!(
                    !attrs.contains_key("timeUnixNano"),
                    "timeUnixNano should not be in attributes"
                );
                assert!(
                    !attrs.contains_key("severityText"),
                    "severityText should not be in attributes"
                );
            }
        }
    }

    #[test]
    fn fixture_log_resource_parsed() {
        let f = load_fixture("logs", "log-query-all.json");
        let entries = get_body_entries(&f);
        let result = convert_log_entries(&entries, true);
        let has_resource = result.entries.iter().any(|e| e.resource.is_some());
        assert!(has_resource, "some entries should have parsed resource");
    }

    #[test]
    fn fixture_trace_query_all() {
        let f = load_fixture("traces", "trace-query-all.json");
        let entries = get_body_entries(&f);
        let expected_count = get_facts(&f)["log_count"].as_u64().unwrap() as usize;

        assert_eq!(entries.len(), expected_count);
        let result = convert_trace_entries(&entries, true);
        assert_eq!(result.total_count, expected_count);
        // SLS progress signal should be propagated (not discarded).
        assert_eq!(result.is_complete, Some(true));

        for span in &result.spans {
            assert!(!span.trace_id.is_empty(), "trace_id should not be empty");
            assert!(!span.span_id.is_empty(), "span_id should not be empty");
            assert!(!span.service.is_empty(), "service should not be empty");
            assert!(span.duration_us >= 0, "duration should be non-negative");
        }
    }

    #[test]
    fn fixture_trace_error_spans() {
        let f = load_fixture("traces", "trace-error-spans.json");
        let entries = get_body_entries(&f);
        let result = convert_trace_entries(&entries, true);

        for span in &result.spans {
            assert_eq!(
                span.status,
                SpanStatus::Error,
                "all spans should have ERROR status"
            );
        }
    }

    #[test]
    fn fixture_trace_empty() {
        let f = load_fixture("traces", "trace-empty.json");
        let entries = get_body_entries(&f);
        let result = convert_trace_entries(&entries, true);
        assert_eq!(result.total_count, 0);
        assert!(result.spans.is_empty());
    }

    #[test]
    fn fixture_trace_by_service() {
        let f = load_fixture("traces", "trace-by-service.json");
        let entries = get_body_entries(&f);
        let result = convert_trace_entries(&entries, true);
        assert!(!result.spans.is_empty(), "should have spans");
    }

    #[test]
    fn test_convert_trace_detail_aggregation() {
        let entries = vec![
            make_log_entry(&[
                ("traceID", serde_json::json!("trace1")),
                ("spanID", serde_json::json!("span1")),
                ("parentSpanID", serde_json::json!("")),
                ("name", serde_json::json!("root")),
                ("service", serde_json::json!("svc-a")),
                ("kind", serde_json::json!("server")),
                ("start", serde_json::json!("2000000")),
                ("duration", serde_json::json!("10000")),
                ("statusCode", serde_json::json!("OK")),
            ]),
            make_log_entry(&[
                ("traceID", serde_json::json!("trace1")),
                ("spanID", serde_json::json!("span2")),
                ("parentSpanID", serde_json::json!("span1")),
                ("name", serde_json::json!("child")),
                ("service", serde_json::json!("svc-b")),
                ("kind", serde_json::json!("client")),
                ("start", serde_json::json!("1000000")),
                ("duration", serde_json::json!("5000")),
                ("statusCode", serde_json::json!("OK")),
            ]),
        ];

        let detail = convert_trace_detail(&entries, "trace1");
        assert_eq!(detail.trace_id, "trace1");
        assert_eq!(detail.span_count, 2);
        assert_eq!(detail.service_count, 2);
        assert!(detail.services.contains(&"svc-a".to_string()));
        assert!(detail.services.contains(&"svc-b".to_string()));
        // Spans should be sorted by start_time (span2 at 1s, span1 at 2s).
        assert_eq!(detail.spans[0].span_id, "span2");
        assert_eq!(detail.spans[1].span_id, "span1");
    }

    #[test]
    fn test_convert_trace_entry_with_span_events() {
        let logs_json = r#"[{"timestamp":1774601911124489,"attribute":{"event":"exception","exception.message":"connection failed","exception.type":"ConnectionError"}}]"#;
        let entry = make_log_entry(&[
            ("traceID", serde_json::json!("abc")),
            ("spanID", serde_json::json!("def")),
            ("parentSpanID", serde_json::json!("")),
            ("name", serde_json::json!("GET")),
            ("service", serde_json::json!("api")),
            ("kind", serde_json::json!("client")),
            ("start", serde_json::json!("1774601911124489")),
            ("duration", serde_json::json!("4485")),
            ("statusCode", serde_json::json!("ERROR")),
            ("logs", serde_json::json!(logs_json)),
        ]);

        let span = convert_trace_entry(&entry);
        let events = span.events.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].name, "exception");
        assert!(events[0].timestamp > 0);
        let event_attrs = events[0].attributes.as_ref().unwrap();
        assert_eq!(event_attrs["exception.message"], "connection failed");
        assert_eq!(event_attrs["exception.type"], "ConnectionError");
        // "event" key should be excluded from attributes (used as name).
        assert!(!event_attrs.contains_key("event"));
    }

    #[test]
    fn fixture_trace_span_events_parsed() {
        let f = load_fixture("traces", "trace-query-all.json");
        let entries = get_body_entries(&f);
        let result = convert_trace_entries(&entries, true);
        // Some spans in the fixture have non-empty `logs` with exception events.
        let has_events = result.spans.iter().any(|s| s.events.is_some());
        assert!(has_events, "some spans should have parsed span events");
    }

    #[test]
    fn fixture_log_fulltext() {
        let f = load_fixture("logs", "log-fulltext.json");
        let entries = get_body_entries(&f);
        let result = convert_log_entries(&entries, true);
        let expected_count = get_facts(&f)["log_count"].as_u64().unwrap() as usize;
        assert_eq!(result.total_count, expected_count);
    }

    #[test]
    fn fixture_log_by_service() {
        let f = load_fixture("logs", "log-by-service.json");
        let entries = get_body_entries(&f);
        let result = convert_log_entries(&entries, true);
        assert!(!result.entries.is_empty(), "should have log entries");
        // All entries should share the same service (filtered by service).
        let first_service = result.entries[0].service.as_deref();
        for e in &result.entries {
            assert_eq!(
                e.service.as_deref(),
                first_service,
                "all entries should belong to the same service"
            );
        }
    }

    #[test]
    fn fixture_log_system_fields_no_source_time_leak() {
        let f = load_fixture("logs", "log-query-all.json");
        let entries = get_body_entries(&f);
        let result = convert_log_entries(&entries, true);

        for e in &result.entries {
            if let Some(attrs) = &e.attributes {
                assert!(
                    !attrs.contains_key("__source__"),
                    "__source__ should not leak into attributes"
                );
                assert!(
                    !attrs.contains_key("__time__"),
                    "__time__ should not leak into attributes"
                );
                assert!(
                    !attrs.contains_key("flags"),
                    "flags should not leak into attributes"
                );
            }
        }
    }

    #[test]
    fn fixture_trace_by_operation() {
        let f = load_fixture("traces", "trace-by-operation.json");
        let entries = get_body_entries(&f);
        let result = convert_trace_entries(&entries, true);
        assert!(!result.spans.is_empty(), "should have spans");
    }

    #[test]
    fn fixture_trace_by_id() {
        let f = load_fixture("traces", "trace-by-id.json");
        let entries = get_body_entries(&f);
        assert!(!entries.is_empty(), "should have trace entries");
        // Extract the trace ID from the first entry for the detail call.
        let trace_id = entries[0]
            .get("traceID")
            .and_then(|v| v.as_str())
            .expect("first entry should have traceID");
        let detail = convert_trace_detail(&entries, trace_id);
        assert_eq!(detail.trace_id, trace_id);
        assert!(detail.span_count > 0, "should have spans");
        assert!(detail.service_count > 0, "should have services");
    }

    #[test]
    fn fixture_trace_by_id_not_found() {
        let f = load_fixture("traces", "trace-by-id-not-found.json");
        let entries = get_body_entries(&f);
        assert!(entries.is_empty(), "not-found should return empty entries");
    }
}
