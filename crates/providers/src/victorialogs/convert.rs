//! `VictoriaLogs` → obz `LogEntry` conversion.
//!
//! Converts raw NDJSON entries from `VictoriaLogs` into normalized
//! obz `LogEntry` models. Key transformations:
//! - `_time` (RFC3339 nanosecond) → Unix seconds
//! - `_msg` → message
//! - `_stream` → resource (parsed from `LogsQL` format)
//! - `severity` → normalized severity level
//! - Remaining fields → attributes

use std::collections::BTreeMap;

use jiff::Timestamp;

use obz_core::model::log::{parse_severity, LogEntry};
use obz_core::provider::LogSearchResult;

use super::response::{parse_stream, VlEntry};

/// System fields that are extracted into dedicated `LogEntry` fields
/// and should NOT appear in `attributes`.
///
/// This includes all field names checked in the heuristic lookup chains
/// for severity, source, service, and trace/span IDs.
const SYSTEM_FIELDS: &[&str] = &[
    "_msg",
    "_time",
    "_stream",
    "_stream_id",
    // Severity candidates (spec §3.1 heuristic)
    "severity",
    "level",
    "log.level",
    "loglevel",
    "log_level",
    // Service candidates (spec §3.3 heuristic)
    "service.name",
    "service",
    "app",
    "app_name",
    // Source
    "host.name",
    "source",
    // Trace/span IDs (VL native + OTel aliases)
    "trace_id",
    "traceID",
    "span_id",
    "spanID",
];

/// Convert a list of `VictoriaLogs` entries to obz `LogSearchResult`.
pub(crate) fn convert_entries(entries: Vec<VlEntry>) -> LogSearchResult {
    let total_count = entries.len();
    let log_entries: Vec<LogEntry> = entries.into_iter().map(convert_entry).collect();

    // VictoriaLogs does not provide a reliable completeness signal,
    // so we set `is_complete` to `None` (omitted from output).
    LogSearchResult {
        entries: log_entries,
        total_count,
        is_complete: None,
        cursor: None,
    }
}

/// Convert a single `VictoriaLogs` NDJSON entry to an obz `LogEntry`.
fn convert_entry(mut entry: VlEntry) -> LogEntry {
    // Extract _time → Unix seconds.
    let timestamp = entry
        .get("_time")
        .and_then(|t| t.parse::<Timestamp>().ok())
        .map(Timestamp::as_second)
        .unwrap_or(0);

    // Extract _msg using remove() to avoid cloning.
    let message = entry.remove("_msg").unwrap_or_default();

    // Extract and normalize severity (spec §3.1 heuristic chain).
    let severity = ["severity", "level", "log.level", "loglevel", "log_level"]
        .iter()
        .find_map(|&k| entry.get(k))
        .map(|s| parse_severity(s));

    // Extract source: host.name → source (spec §4.3).
    let source = entry
        .get("host.name")
        .or_else(|| entry.get("source"))
        .cloned();

    // Extract service (spec §3.3 heuristic chain).
    let service = entry
        .get("service.name")
        .or_else(|| entry.get("service"))
        .or_else(|| entry.get("app"))
        .or_else(|| entry.get("app_name"))
        .cloned();

    // Extract trace/span IDs (VL native + OTel aliases, spec §4.3).
    let trace_id = entry
        .get("trace_id")
        .or_else(|| entry.get("traceID"))
        .cloned()
        .filter(|s| !s.is_empty());
    let span_id = entry
        .get("span_id")
        .or_else(|| entry.get("spanID"))
        .cloned()
        .filter(|s| !s.is_empty());

    // Parse _stream into resource labels.
    let resource: Option<BTreeMap<String, String>> = entry
        .get("_stream")
        .map(|s| parse_stream(s))
        .filter(|m| !m.is_empty());

    // Collect remaining fields as attributes (consume entry to avoid clone).
    let attributes: BTreeMap<String, String> = entry
        .into_iter()
        .filter(|(k, _)| !SYSTEM_FIELDS.contains(&k.as_str()))
        .collect();

    let attributes = if attributes.is_empty() {
        None
    } else {
        Some(attributes)
    };

    LogEntry {
        timestamp,
        message,
        severity,
        source,
        service,
        id: None, // VictoriaLogs has no per-entry ID.
        attributes,
        resource,
        trace_id,
        span_id,
        extensions: None, // Full View extensions to be added later.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_entry(fields: &[(&str, &str)]) -> VlEntry {
        fields
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn test_convert_basic_entry() {
        let entry = make_entry(&[
            ("_time", "2026-03-27T09:58:24.141420111Z"),
            ("_msg", "Error scraping metrics"),
            ("_stream", r#"{host.name="web01",service.name="otelcol"}"#),
            ("_stream_id", "abc123"),
            ("severity", "error"),
            ("host.name", "web01"),
            ("service.name", "otelcol"),
            ("code.line.number", "61"),
        ]);

        let log = convert_entry(entry);
        assert_eq!(log.timestamp, 1774605504);
        assert_eq!(log.message, "Error scraping metrics");
        assert_eq!(log.severity, Some(obz_core::model::log::Severity::Error));
        assert_eq!(log.source.as_deref(), Some("web01"));
        assert_eq!(log.service.as_deref(), Some("otelcol"));

        // _stream should be parsed into resource.
        let resource = log.resource.unwrap();
        assert_eq!(resource["host.name"], "web01");
        assert_eq!(resource["service.name"], "otelcol");

        // code.line.number should be in attributes (not a system field).
        let attrs = log.attributes.unwrap();
        assert_eq!(attrs["code.line.number"], "61");
        // System fields should NOT be in attributes.
        assert!(!attrs.contains_key("_msg"));
        assert!(!attrs.contains_key("_time"));
        assert!(!attrs.contains_key("severity"));
    }

    #[test]
    fn test_parse_rfc3339_timestamp_exact() {
        // Unix epoch → 0.
        let entry = make_entry(&[("_time", "1970-01-01T00:00:00Z"), ("_msg", "epoch")]);
        assert_eq!(convert_entry(entry).timestamp, 0);

        // Known fixed point: 2024-03-23T19:00:00Z = 1711220400
        let entry = make_entry(&[("_time", "2024-03-23T19:00:00Z"), ("_msg", "x")]);
        assert_eq!(convert_entry(entry).timestamp, 1_711_220_400);

        // Sub-second part must be truncated (not rounded).
        let entry = make_entry(&[("_time", "2024-03-23T19:00:00.999999999Z"), ("_msg", "x")]);
        assert_eq!(convert_entry(entry).timestamp, 1_711_220_400);

        // Pre-epoch with fractional seconds: jiff truncates toward zero.
        // -0.5s → as_second() = 0 (not -1).
        let entry = make_entry(&[("_time", "1969-12-31T23:59:59.500Z"), ("_msg", "x")]);
        assert_eq!(convert_entry(entry).timestamp, 0);
        // Whole pre-epoch second is unambiguous.
        let entry = make_entry(&[("_time", "1969-12-31T23:59:59Z"), ("_msg", "x")]);
        assert_eq!(convert_entry(entry).timestamp, -1);
    }

    #[test]
    fn test_parse_rfc3339_timestamp_invalid() {
        // Invalid _time → timestamp falls back to 0.
        let entry = make_entry(&[("_time", "not-a-time"), ("_msg", "x")]);
        assert_eq!(convert_entry(entry).timestamp, 0);

        // Missing _time → timestamp falls back to 0.
        let entry = make_entry(&[("_msg", "x")]);
        assert_eq!(convert_entry(entry).timestamp, 0);
    }

    #[test]
    fn test_parse_rfc3339_with_offset() {
        // +08:00 offset: 2024-03-23T19:00:00+08:00 = 2024-03-23T11:00:00Z = 1711191600
        let entry = make_entry(&[("_time", "2024-03-23T19:00:00+08:00"), ("_msg", "x")]);
        assert_eq!(convert_entry(entry).timestamp, 1_711_191_600);
    }

    #[test]
    fn test_convert_entry_with_trace_id() {
        let entry = make_entry(&[
            ("_time", "2026-03-27T09:58:24Z"),
            ("_msg", "request"),
            ("trace_id", "72910557f18df4ab27da472b7b067f49"),
            ("span_id", "cca34c6b34f963a6"),
        ]);

        let log = convert_entry(entry);
        assert_eq!(
            log.trace_id.as_deref(),
            Some("72910557f18df4ab27da472b7b067f49")
        );
        assert_eq!(log.span_id.as_deref(), Some("cca34c6b34f963a6"));
    }

    #[test]
    fn test_convert_entry_with_camel_case_trace_id() {
        // VL may use camelCase "traceID"/"spanID" (spec §4.3 fallback).
        let entry = make_entry(&[
            ("_time", "2026-03-27T09:58:24Z"),
            ("_msg", "request"),
            ("traceID", "abcd1234"),
            ("spanID", "ef567890"),
        ]);

        let log = convert_entry(entry);
        assert_eq!(log.trace_id.as_deref(), Some("abcd1234"));
        assert_eq!(log.span_id.as_deref(), Some("ef567890"));
    }

    #[test]
    fn test_convert_entry_minimal() {
        let entry = make_entry(&[("_time", "2026-03-27T09:58:24Z"), ("_msg", "hello")]);

        let log = convert_entry(entry);
        assert_eq!(log.message, "hello");
        assert!(log.severity.is_none());
        assert!(log.source.is_none());
        assert!(log.service.is_none());
        assert!(log.attributes.is_none());
        assert!(log.resource.is_none());
    }

    #[test]
    fn test_convert_entries_result() {
        let entries = vec![
            make_entry(&[("_time", "2026-03-27T09:58:24Z"), ("_msg", "one")]),
            make_entry(&[("_time", "2026-03-27T09:58:25Z"), ("_msg", "two")]),
        ];

        let result = convert_entries(entries);
        assert_eq!(result.total_count, 2);
        assert_eq!(result.entries.len(), 2);
        assert_eq!(result.is_complete, None);
        assert!(result.cursor.is_none());
    }

    // --- Fixture-based tests ---

    fn fixture_dir() -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/victoria/logs")
    }

    fn load_fixture(name: &str) -> serde_json::Value {
        let path = fixture_dir().join(name);
        let content = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("fixture {}: {e}", path.display()));
        serde_json::from_str(&content).expect("invalid fixture JSON")
    }

    fn get_ndjson_body(f: &serde_json::Value) -> String {
        f["response"]["body_raw"].as_str().unwrap_or("").to_string()
    }

    #[test]
    fn fixture_query_all() {
        let f = load_fixture("query-all.json");
        let entries = super::super::response::parse_ndjson(&get_ndjson_body(&f)).unwrap();
        assert!(!entries.is_empty());
        let r = convert_entries(entries);
        for e in &r.entries {
            assert!(e.timestamp > 0);
            assert!(!e.message.is_empty());
        }
    }

    #[test]
    fn fixture_query_empty() {
        let f = load_fixture("query-empty.json");
        let entries = super::super::response::parse_ndjson(&get_ndjson_body(&f)).unwrap();
        assert!(entries.is_empty());
    }

    #[test]
    fn fixture_query_severity_error() {
        let f = load_fixture("query-severity-error.json");
        let entries = super::super::response::parse_ndjson(&get_ndjson_body(&f)).unwrap();
        let r = convert_entries(entries);
        for e in &r.entries {
            assert_eq!(e.severity, Some(obz_core::model::log::Severity::Error));
        }
    }

    #[test]
    fn fixture_system_fields_excluded() {
        let f = load_fixture("query-all.json");
        let entries = super::super::response::parse_ndjson(&get_ndjson_body(&f)).unwrap();
        let r = convert_entries(entries);
        for e in &r.entries {
            if let Some(attrs) = &e.attributes {
                assert!(!attrs.contains_key("_msg"));
                assert!(!attrs.contains_key("_time"));
                assert!(!attrs.contains_key("_stream"));
                assert!(!attrs.contains_key("severity"));
            }
        }
    }

    #[test]
    fn fixture_resource_from_stream() {
        let f = load_fixture("query-all.json");
        let entries = super::super::response::parse_ndjson(&get_ndjson_body(&f)).unwrap();
        let r = convert_entries(entries);
        let has_resource = r.entries.iter().any(|e| e.resource.is_some());
        assert!(
            has_resource,
            "some entries should have resource from _stream"
        );
    }

    #[test]
    fn fixture_query_fulltext() {
        let f = load_fixture("query-fulltext.json");
        let entries = super::super::response::parse_ndjson(&get_ndjson_body(&f)).unwrap();
        let r = convert_entries(entries);
        assert!(
            !r.entries.is_empty(),
            "fulltext search should return entries"
        );
    }

    #[test]
    fn fixture_query_service_filter() {
        let f = load_fixture("query-service-filter.json");
        let entries = super::super::response::parse_ndjson(&get_ndjson_body(&f)).unwrap();
        let r = convert_entries(entries);
        for e in &r.entries {
            assert!(
                e.service.is_some(),
                "service-filtered entries should have a service"
            );
        }
    }

    #[test]
    fn fixture_query_stream_filter() {
        let f = load_fixture("query-stream-filter.json");
        let entries = super::super::response::parse_ndjson(&get_ndjson_body(&f)).unwrap();
        let r = convert_entries(entries);
        assert!(
            !r.entries.is_empty(),
            "stream-filtered query should return entries"
        );
    }

    #[test]
    fn fixture_query_pipe_fields() {
        let f = load_fixture("query-pipe-fields.json");
        let entries = super::super::response::parse_ndjson(&get_ndjson_body(&f)).unwrap();
        let r = convert_entries(entries);
        assert!(
            !r.entries.is_empty(),
            "pipe-fields query should return entries"
        );
    }
}
