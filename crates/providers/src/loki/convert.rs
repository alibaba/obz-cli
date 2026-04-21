//! Loki → obz `LogEntry` conversion.
//!
//! Converts Loki stream responses into normalized obz `LogEntry` models.
//! Key transformations:
//! - Nanosecond Unix timestamp string → Unix seconds (`i64`)
//! - `stream.severity_text` / `stream.detected_level` → `Severity`
//! - `stream.service_name` → `service`
//! - `stream.host_name` → `source`
//! - `stream.trace_id` / `stream.span_id` → trace correlation fields
//! - `values[n][1]` → `message`
//! - Remaining stream labels → `attributes`

use std::collections::BTreeMap;

use obz_core::model::error::{ErrorCode, ObzError};
use obz_core::model::log::{parse_severity, severity_from_otel_number, LogEntry};
use obz_core::provider::results::{LogSearchResult, ProviderResult};

use super::response::{LokiQueryData, LokiResponse};
use crate::util::parse_nanos_to_seconds;

/// Stream label keys that map to dedicated `LogEntry` fields and should NOT
/// appear in `attributes`.
const SYSTEM_LABELS: &[&str] = &[
    // Severity candidates
    "severity_text",
    "detected_level",
    "severity_number",
    "level",
    // Service candidates
    "service_name",
    "otelServiceName",
    // Source candidates
    "host_name",
    // Trace/span IDs
    "trace_id",
    "span_id",
    "otelTraceID",
    "otelSpanID",
    "otelTraceSampled",
];

/// Check the Loki response envelope for errors.
///
/// Loki wraps both successful and error responses in the same JSON
/// envelope. If the `status` field is `"error"`, this function extracts
/// the error message and returns an appropriate `ObzError`.
pub(crate) fn check_loki_error<T>(resp: &LokiResponse<T>) -> ProviderResult<()> {
    if resp.status == "error" {
        let message = resp.error.as_deref().unwrap_or("unknown error from Loki");
        let error_type = resp.error_type.as_deref().unwrap_or("");

        return Err(ObzError::Provider {
            code: ErrorCode::BackendError,
            message: format!("Loki error ({error_type}): {message}"),
            raw_error: resp.error.clone(),
            recoverable: false,
            suggestion: None,
            doc_url: None,
        });
    }
    Ok(())
}

/// Convert a Loki query response to `LogSearchResult`.
pub(crate) fn convert_query_response(
    resp: LokiResponse<LokiQueryData>,
) -> ProviderResult<LogSearchResult> {
    check_loki_error(&resp)?;

    let data = resp.data.ok_or_else(|| ObzError::Provider {
        code: ErrorCode::BackendError,
        message: "Loki response has no data field".to_string(),
        raw_error: None,
        recoverable: false,
        suggestion: None,
        doc_url: None,
    })?;

    let mut entries = Vec::new();
    for stream in data.result {
        let labels = &stream.stream;
        for (ts_str, message) in &stream.values {
            entries.push(convert_stream_entry(labels, ts_str, message));
        }
    }

    let total_count = entries.len();
    // Loki's query_range API does not provide a reliable truncation
    // indicator, so we set `is_complete` to `None` (omitted from output).
    // Agents can infer potential truncation from `total_count == limit`.
    Ok(LogSearchResult {
        entries,
        total_count,
        is_complete: None,
        cursor: None,
    })
}

/// Convert a single log entry from a Loki stream into an obz `LogEntry`.
fn convert_stream_entry(
    labels: &BTreeMap<String, String>,
    ts_str: &str,
    message: &str,
) -> LogEntry {
    // Parse nanosecond Unix timestamp → seconds.
    let timestamp = parse_nanos_to_seconds(ts_str);

    // Extract severity from stream labels.
    let severity = labels
        .get("severity_text")
        .or_else(|| labels.get("detected_level"))
        .or_else(|| labels.get("level"))
        .map(|s| parse_severity(s))
        .or_else(|| {
            labels
                .get("severity_number")
                .and_then(|s| s.parse::<u32>().ok())
                .and_then(severity_from_otel_number)
        });

    // Extract source (host).
    let source = labels.get("host_name").cloned();

    // Extract service name.
    let service = labels
        .get("service_name")
        .or_else(|| labels.get("otelServiceName"))
        .cloned();

    // Extract trace/span IDs.
    let trace_id = labels
        .get("trace_id")
        .or_else(|| labels.get("otelTraceID"))
        .cloned()
        .filter(|s| !s.is_empty());
    let span_id = labels
        .get("span_id")
        .or_else(|| labels.get("otelSpanID"))
        .cloned()
        .filter(|s| !s.is_empty());

    // Remaining labels become attributes.
    let attributes: BTreeMap<String, String> = labels
        .iter()
        .filter(|(k, _)| !SYSTEM_LABELS.contains(&k.as_str()))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();

    let attributes = if attributes.is_empty() {
        None
    } else {
        Some(attributes)
    };

    LogEntry {
        timestamp,
        message: message.to_string(),
        severity,
        source,
        service,
        id: None,
        attributes,
        resource: None,
        trace_id,
        span_id,
        extensions: None,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loki::response::LokiQueryData;
    use obz_core::model::log::Severity;

    fn load_fixture(name: &str) -> serde_json::Value {
        let path = format!(
            "{}/fixtures/grafana/logs/{name}.json",
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
    fn test_convert_stream_entry_basic() {
        let mut labels = BTreeMap::new();
        labels.insert("service_name".into(), "my-service".into());
        labels.insert("severity_text".into(), "error".into());
        labels.insert("host_name".into(), "host-01".into());
        labels.insert("custom_label".into(), "value".into());

        let entry = convert_stream_entry(&labels, "1775538346468275580", "test message");

        assert_eq!(entry.timestamp, 1_775_538_346);
        assert_eq!(entry.message, "test message");
        assert_eq!(entry.severity, Some(Severity::Error));
        assert_eq!(entry.service.as_deref(), Some("my-service"));
        assert_eq!(entry.source.as_deref(), Some("host-01"));

        // custom_label should be in attributes
        let attrs = entry.attributes.unwrap();
        assert!(attrs.contains_key("custom_label"));
        // system labels should NOT be in attributes
        assert!(!attrs.contains_key("service_name"));
        assert!(!attrs.contains_key("severity_text"));
        assert!(!attrs.contains_key("host_name"));
    }

    #[test]
    fn test_convert_stream_entry_with_trace_ids() {
        let mut labels = BTreeMap::new();
        labels.insert("trace_id".into(), "abc123".into());
        labels.insert("span_id".into(), "def456".into());

        let entry = convert_stream_entry(&labels, "1000000000", "msg");
        assert_eq!(entry.trace_id.as_deref(), Some("abc123"));
        assert_eq!(entry.span_id.as_deref(), Some("def456"));
    }

    #[test]
    fn test_convert_stream_entry_otel_trace_ids() {
        let mut labels = BTreeMap::new();
        labels.insert("otelTraceID".into(), "trace123".into());
        labels.insert("otelSpanID".into(), "span456".into());

        let entry = convert_stream_entry(&labels, "1000000000", "msg");
        assert_eq!(entry.trace_id.as_deref(), Some("trace123"));
        assert_eq!(entry.span_id.as_deref(), Some("span456"));
    }

    #[test]
    fn test_convert_stream_entry_detected_level_fallback() {
        let mut labels = BTreeMap::new();
        labels.insert("detected_level".into(), "info".into());

        let entry = convert_stream_entry(&labels, "1000000000", "msg");
        assert_eq!(entry.severity, Some(Severity::Info));
    }

    #[test]
    fn test_severity_number_fallback_and_not_in_attributes() {
        let mut labels = BTreeMap::new();
        labels.insert("severity_number".into(), "17".into());
        labels.insert("host_name".into(), "web01".into());

        let entry = convert_stream_entry(&labels, "1672577533000000000", "test message");

        assert_eq!(entry.severity, Some(Severity::Error));
        assert!(
            entry.attributes.is_none()
                || !entry
                    .attributes
                    .as_ref()
                    .is_some_and(|attrs| attrs.contains_key("severity_number"))
        );
    }

    // -- Fixture tests ------------------------------------------------------

    #[test]
    fn fixture_query_all() {
        let body = load_fixture("query-all");
        let resp: LokiResponse<LokiQueryData> = serde_json::from_value(body).unwrap();
        let result = convert_query_response(resp).unwrap();

        assert_eq!(result.total_count, 20);
        assert_eq!(result.entries.len(), 20);
        // Loki provides no reliable truncation signal.
        assert_eq!(result.is_complete, None);

        for entry in &result.entries {
            assert!(entry.timestamp > 0, "timestamp should be positive");
            assert!(!entry.message.is_empty(), "message should not be empty");
        }

        // Check that service_name is extracted for entries that have it
        let with_service = result
            .entries
            .iter()
            .filter(|e| e.service.is_some())
            .count();
        assert!(with_service > 0, "some entries should have service");
    }

    #[test]
    fn fixture_query_empty() {
        let body = load_fixture("query-empty");
        let resp: LokiResponse<LokiQueryData> = serde_json::from_value(body).unwrap();
        let result = convert_query_response(resp).unwrap();
        assert_eq!(result.total_count, 0);
        assert!(result.entries.is_empty());
    }

    #[test]
    fn fixture_query_by_service() {
        let body = load_fixture("query-by-service");
        let resp: LokiResponse<LokiQueryData> = serde_json::from_value(body).unwrap();
        let result = convert_query_response(resp).unwrap();
        assert!(!result.entries.is_empty());
    }

    #[test]
    fn fixture_query_fulltext() {
        let body = load_fixture("query-fulltext");
        let resp: LokiResponse<LokiQueryData> = serde_json::from_value(body).unwrap();
        let result = convert_query_response(resp).unwrap();
        assert!(!result.entries.is_empty());
        for entry in &result.entries {
            assert!(entry.timestamp > 0, "timestamp should be positive");
        }
    }

    #[test]
    fn fixture_query_severity_error() {
        let body = load_fixture("query-severity-error");
        let resp: LokiResponse<LokiQueryData> = serde_json::from_value(body).unwrap();
        let result = convert_query_response(resp).unwrap();
        assert!(!result.entries.is_empty());
        // All entries from the severity fixture should have severity extracted.
        // The fixture data contains `warn` level entries (recorded from a live
        // Loki instance), so we verify severity is present and is Warn.
        for entry in &result.entries {
            assert_eq!(
                entry.severity,
                Some(Severity::Warn),
                "severity-filtered entries should have Warn severity"
            );
        }
    }

    #[test]
    fn fixture_query_json_filter() {
        let body = load_fixture("query-json-filter");
        let resp: LokiResponse<LokiQueryData> = serde_json::from_value(body).unwrap();
        let result = convert_query_response(resp).unwrap();
        assert!(!result.entries.is_empty());
        for entry in &result.entries {
            assert!(entry.timestamp > 0, "timestamp should be positive");
        }
    }

    #[test]
    fn fixture_system_labels_excluded() {
        let body = load_fixture("query-all");
        let resp: LokiResponse<LokiQueryData> = serde_json::from_value(body).unwrap();
        let result = convert_query_response(resp).unwrap();

        for entry in &result.entries {
            if let Some(attrs) = &entry.attributes {
                for key in SYSTEM_LABELS {
                    assert!(
                        !attrs.contains_key(*key),
                        "system label '{key}' should not be in attributes"
                    );
                }
            }
        }
    }

    #[test]
    fn fixture_entries_with_trace_ids() {
        let body = load_fixture("query-all");
        let resp: LokiResponse<LokiQueryData> = serde_json::from_value(body).unwrap();
        let result = convert_query_response(resp).unwrap();

        // The load-generator entries have trace_id and span_id
        let with_trace = result
            .entries
            .iter()
            .filter(|e| e.trace_id.is_some())
            .count();
        assert!(
            with_trace > 0,
            "some entries should have trace_id extracted"
        );
    }
}
