//! Datadog response → obz-core model conversion functions.
//!
//! Each function takes a Datadog-specific response type and converts it
//! into the corresponding obz-core model.  All timestamp conversions
//! (milliseconds → seconds, ISO 8601 → Unix) happen here.

use std::collections::BTreeMap;

use obz_core::model::log::{parse_severity, LogEntry};
use obz_core::model::metric::{DataPoint, MetricInfoDetail, MetricSeries, MetricType, SeriesStats};
use obz_core::model::trace::{Span, SpanKind, SpanStatus, TraceDetail};
use obz_core::provider::results::{
    LogSearchResult, MetricQueryResult, MetricResultType, TraceSearchResult,
};

use super::response::{
    DdLogEvent, DdLogsResponse, DdMetricMetadata, DdMetricQueryResponse, DdSearchResponse,
    DdSpanAttributes, DdSpanEvent, DdSpansResponse,
};

// ---------------------------------------------------------------------------
// Metric Query
// ---------------------------------------------------------------------------

/// Convert a Datadog metric query response to a [`MetricQueryResult`].
pub(crate) fn convert_metric_query(resp: DdMetricQueryResponse) -> MetricQueryResult {
    let series: Vec<MetricSeries> = resp.series.into_iter().map(convert_series).collect();
    let total_count = series.len();

    // Datadog metric queries always return time series (res_type: "time_series"),
    // so the result type is always Matrix regardless of whether data is empty.
    MetricQueryResult {
        result_type: MetricResultType::Matrix,
        series,
        scalar: None,
        total_count,
    }
}

/// Convert a single Datadog series to a [`MetricSeries`].
fn convert_series(s: super::response::DdSeries) -> MetricSeries {
    // Parse tag_set (["key:value", ...]) into labels.
    let mut labels = BTreeMap::new();
    for tag in &s.tag_set {
        if let Some((k, v)) = tag.split_once(':') {
            labels.insert(k.to_string(), v.to_string());
        }
    }

    // Convert pointlist: timestamps from milliseconds to seconds.
    // Use integer division after casting to avoid floating-point precision issues.
    let points: Vec<DataPoint> = s
        .pointlist
        .iter()
        .map(|p| DataPoint {
            timestamp: (p.0 as i64) / 1000,
            value: p.1.unwrap_or(f64::NAN),
        })
        .collect();

    let stats = if points.is_empty() {
        None
    } else {
        Some(SeriesStats::from_points(&points))
    };

    // Build extensions for Datadog-specific metadata.
    let mut extensions = BTreeMap::new();
    if let Some(interval) = s.interval {
        extensions.insert(
            "datadog.interval".to_string(),
            serde_json::Value::Number(serde_json::Number::from(interval)),
        );
    }
    if let Some(scope) = &s.scope {
        extensions.insert(
            "datadog.scope".to_string(),
            serde_json::Value::String(scope.clone()),
        );
    }

    MetricSeries {
        name: s.metric,
        labels,
        points,
        stats,
        extensions: if extensions.is_empty() {
            None
        } else {
            Some(extensions)
        },
    }
}

// ---------------------------------------------------------------------------
// Metric Search (list)
// ---------------------------------------------------------------------------

/// Convert a Datadog metric search response to a list of metric names.
pub(crate) fn convert_metric_search(resp: DdSearchResponse) -> Vec<String> {
    resp.results.metrics
}

// ---------------------------------------------------------------------------
// Metric Metadata (info)
// ---------------------------------------------------------------------------

/// Convert Datadog metric metadata to a [`MetricInfoDetail`].
pub(crate) fn convert_metric_metadata(name: &str, resp: DdMetricMetadata) -> Vec<MetricInfoDetail> {
    let metric_type = resp.metric_type.as_deref().map(parse_metric_type);

    // Combine unit and per_unit into a single string (e.g. "byte/second").
    // Filter empty strings to match the description field handling.
    let unit = match (&resp.unit, &resp.per_unit) {
        (Some(u), Some(pu)) if !u.is_empty() && !pu.is_empty() => Some(format!("{u}/{pu}")),
        (Some(u), _) if !u.is_empty() => Some(u.clone()),
        _ => None,
    };

    vec![MetricInfoDetail {
        name: name.to_string(),
        metric_type,
        description: resp.description.filter(|d| !d.is_empty()),
        unit,
    }]
}

/// Map Datadog metric type strings to [`MetricType`].
fn parse_metric_type(t: &str) -> MetricType {
    match t.to_lowercase().as_str() {
        "gauge" => MetricType::Gauge,
        "count" | "counter" | "rate" => MetricType::Counter,
        "histogram" | "distribution" => MetricType::Histogram,
        _ => MetricType::Unknown,
    }
}

// ---------------------------------------------------------------------------
// Logs Search
// ---------------------------------------------------------------------------

/// Convert a Datadog logs search response to a [`LogSearchResult`].
pub(crate) fn convert_logs_search(resp: DdLogsResponse) -> LogSearchResult {
    let total_count = resp.data.len();
    let entries: Vec<LogEntry> = resp.data.into_iter().map(convert_log_entry).collect();

    let cursor = resp.meta.and_then(|m| m.page).and_then(|p| p.after);

    LogSearchResult {
        entries,
        total_count,
        is_complete: Some(cursor.is_none()),
        cursor,
    }
}

/// Convert a single Datadog log event to a [`LogEntry`].
fn convert_log_entry(event: DdLogEvent) -> LogEntry {
    let attrs = &event.attributes;

    // Parse ISO 8601 timestamp to Unix seconds.
    let timestamp = attrs
        .timestamp
        .as_deref()
        .and_then(parse_iso8601_to_unix)
        .unwrap_or(0);

    let severity = attrs.status.as_deref().map(parse_severity);

    // Flatten nested attributes to string key-value pairs.
    let attributes = attrs.attributes.as_ref().map(|v| flatten_json_value(v, ""));

    // Extract trace_id and span_id from nested OTel attributes.
    let trace_id = attrs
        .attributes
        .as_ref()
        .and_then(|v| v.pointer("/otel/trace_id"))
        .and_then(|v| v.as_str())
        .map(String::from);

    let span_id = attrs
        .attributes
        .as_ref()
        .and_then(|v| v.pointer("/otel/span_id"))
        .and_then(|v| v.as_str())
        .map(String::from);

    // Build resource from tags.
    let resource = if attrs.tags.is_empty() {
        None
    } else {
        let mut map = BTreeMap::new();
        for tag in &attrs.tags {
            if let Some((k, v)) = tag.split_once(':') {
                map.insert(k.to_string(), v.to_string());
            }
        }
        if map.is_empty() {
            None
        } else {
            Some(map)
        }
    };

    LogEntry {
        timestamp,
        message: attrs.message.clone().unwrap_or_default(),
        severity,
        source: attrs.host.clone(),
        service: attrs.service.clone(),
        id: event.id,
        attributes,
        resource,
        trace_id,
        span_id,
        extensions: None,
    }
}

// ---------------------------------------------------------------------------
// Traces / Spans Search
// ---------------------------------------------------------------------------

/// Convert a Datadog spans search response to a [`TraceSearchResult`].
pub(crate) fn convert_spans_search(resp: DdSpansResponse) -> TraceSearchResult {
    let total_count = resp.data.len();
    let spans: Vec<Span> = resp.data.iter().map(convert_span).collect();

    let cursor = resp.meta.and_then(|m| m.page).and_then(|p| p.after);

    TraceSearchResult {
        spans,
        total_count,
        is_complete: Some(cursor.is_none()),
        cursor,
    }
}

/// Convert a Datadog spans search response to a [`TraceDetail`].
///
/// Used by `get_trace()` where we search by `trace_id` and aggregate
/// all returned spans into a single trace detail view.
pub(crate) fn convert_trace_detail(trace_id: &str, resp: &DdSpansResponse) -> TraceDetail {
    let spans: Vec<Span> = resp.data.iter().map(convert_span).collect();
    TraceDetail::from_spans(trace_id.to_string(), spans)
}

/// Convert a single Datadog span event to a [`Span`].
fn convert_span(event: &DdSpanEvent) -> Span {
    let attrs = &event.attributes;

    let start_time = attrs
        .start_timestamp
        .as_deref()
        .and_then(parse_iso8601_to_unix)
        .unwrap_or(0);

    // Compute duration from custom.duration (nanoseconds) or from
    // start/end timestamps.
    let duration_us = extract_duration_us(attrs);

    let status = match attrs.status.as_deref() {
        Some("error") => SpanStatus::Error,
        Some("ok") => SpanStatus::Ok,
        _ => SpanStatus::Unset,
    };

    let kind = extract_span_kind(attrs);

    let parent_span_id = attrs
        .parent_id
        .as_deref()
        .filter(|id| *id != "0")
        .map(String::from);

    // Flatten custom attributes for the attributes field.
    let flat_attrs = attrs.custom.as_ref().map(|v| flatten_json_value(v, ""));

    // Build extensions with Datadog-specific metadata.
    let mut extensions = BTreeMap::new();
    if let Some(span_type) = &attrs.span_type {
        extensions.insert(
            "datadog.type".to_string(),
            serde_json::Value::String(span_type.clone()),
        );
    }
    if let Some(resource_name) = &attrs.resource_name {
        extensions.insert(
            "datadog.resource_name".to_string(),
            serde_json::Value::String(resource_name.clone()),
        );
    }
    if let Some(env) = &attrs.env {
        extensions.insert(
            "datadog.env".to_string(),
            serde_json::Value::String(env.clone()),
        );
    }

    Span {
        trace_id: attrs.trace_id.clone().unwrap_or_default(),
        span_id: attrs.span_id.clone().unwrap_or_default(),
        parent_span_id,
        name: attrs.operation_name.clone().unwrap_or_default(),
        service: attrs.service.clone().unwrap_or_default(),
        kind,
        status,
        start_time,
        duration_us,
        attributes: flat_attrs,
        events: None,
        resource: None,
        extensions: if extensions.is_empty() {
            None
        } else {
            Some(extensions)
        },
    }
}

/// Extract duration in microseconds from Datadog span attributes.
///
/// Prefers `custom.duration` (nanoseconds) when available, otherwise
/// falls back to computing from start/end timestamps.
fn extract_duration_us(attrs: &DdSpanAttributes) -> i64 {
    // Try custom.duration first (value is in nanoseconds).
    if let Some(custom) = &attrs.custom {
        if let Some(dur) = custom.get("duration") {
            if let Some(ns) = dur
                .as_i64()
                .or_else(|| dur.as_f64().filter(|f| f.is_finite()).map(|f| f as i64))
            {
                return ns / 1000; // nanoseconds → microseconds
            }
        }
    }

    // Fallback: compute from start/end timestamps.
    let start_ms = attrs
        .start_timestamp
        .as_deref()
        .and_then(parse_iso8601_to_millis)
        .unwrap_or(0);
    let end_ms = attrs
        .end_timestamp
        .as_deref()
        .and_then(parse_iso8601_to_millis)
        .unwrap_or(0);

    (end_ms - start_ms) * 1000 // milliseconds → microseconds
}

/// Extract `SpanKind` from Datadog's `custom.span.kind` field.
fn extract_span_kind(attrs: &DdSpanAttributes) -> Option<SpanKind> {
    attrs
        .custom
        .as_ref()
        .and_then(|c| c.pointer("/span/kind"))
        .and_then(|v| v.as_str())
        .and_then(|k| match k.to_lowercase().as_str() {
            "server" => Some(SpanKind::Server),
            "client" => Some(SpanKind::Client),
            "producer" => Some(SpanKind::Producer),
            "consumer" => Some(SpanKind::Consumer),
            "internal" => Some(SpanKind::Internal),
            _ => None,
        })
}

// ---------------------------------------------------------------------------
// Utility functions
// ---------------------------------------------------------------------------

/// Parse an ISO 8601 timestamp string to Unix seconds.
///
/// Handles formats like `"2026-03-27T09:06:02.482Z"` and
/// `"2026-03-27T09:06:07.765Z"`.
fn parse_iso8601_to_unix(s: &str) -> Option<i64> {
    s.parse::<jiff::Timestamp>()
        .ok()
        .map(jiff::Timestamp::as_second)
}

/// Parse an ISO 8601 timestamp string to milliseconds since epoch.
fn parse_iso8601_to_millis(s: &str) -> Option<i64> {
    s.parse::<jiff::Timestamp>()
        .ok()
        .map(jiff::Timestamp::as_millisecond)
}

/// Flatten a nested JSON value into a `BTreeMap<String, String>`.
///
/// Uses dot-notation for nested keys (e.g. `"http.method"`, `"otel.trace_id"`).
/// Skips null values and arrays to keep the output concise.
fn flatten_json_value(value: &serde_json::Value, prefix: &str) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    flatten_recursive(value, prefix, &mut map);
    map
}

fn flatten_recursive(value: &serde_json::Value, prefix: &str, map: &mut BTreeMap<String, String>) {
    match value {
        serde_json::Value::Object(obj) => {
            for (k, v) in obj {
                let key = if prefix.is_empty() {
                    k.clone()
                } else {
                    format!("{prefix}.{k}")
                };
                flatten_recursive(v, &key, map);
            }
        }
        serde_json::Value::String(s) if !prefix.is_empty() => {
            map.insert(prefix.to_string(), s.clone());
        }
        serde_json::Value::Number(n) if !prefix.is_empty() => {
            map.insert(prefix.to_string(), n.to_string());
        }
        serde_json::Value::Bool(b) if !prefix.is_empty() => {
            map.insert(prefix.to_string(), b.to_string());
        }
        // Skip Null and Array to keep output concise.
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use obz_core::model::log::Severity;
    use obz_core::model::trace::SpanStatus;

    // --- Unit tests ---

    #[test]
    fn test_parse_iso8601() {
        let ts = parse_iso8601_to_unix("2026-03-27T09:06:02.482Z");
        assert!(ts.is_some());
        assert!(ts.unwrap() > 0);
    }

    #[test]
    fn test_parse_iso8601_exact_value() {
        // Unix epoch → must return 0.
        assert_eq!(parse_iso8601_to_unix("1970-01-01T00:00:00Z"), Some(0));
        // Known fixed point: 2024-03-23T19:00:00Z = 1711220400
        assert_eq!(
            parse_iso8601_to_unix("2024-03-23T19:00:00Z"),
            Some(1_711_220_400)
        );
        // Sub-second part must be truncated, not rounded.
        assert_eq!(
            parse_iso8601_to_unix("2024-03-23T19:00:00.999Z"),
            Some(1_711_220_400)
        );
        // Pre-epoch with fractional seconds: jiff truncates toward zero.
        // -0.5s → as_second() = 0 (not -1).
        // This is "truncate toward zero" semantics, matching jiff's behavior
        // (chrono used floor/toward-negative-infinity).
        assert_eq!(parse_iso8601_to_unix("1969-12-31T23:59:59.500Z"), Some(0));
        // Whole pre-epoch second is unambiguous.
        assert_eq!(parse_iso8601_to_unix("1969-12-31T23:59:59Z"), Some(-1));
    }

    #[test]
    fn test_parse_iso8601_invalid() {
        assert!(parse_iso8601_to_unix("not-a-date").is_none());
        assert!(parse_iso8601_to_unix("").is_none());
        assert!(parse_iso8601_to_unix("2024-13-01T00:00:00Z").is_none()); // month 13
    }

    #[test]
    fn test_parse_iso8601_to_millis() {
        let ms = parse_iso8601_to_millis("2026-03-27T09:06:02.482Z");
        assert!(ms.is_some());
        let ms = ms.unwrap();
        let secs = parse_iso8601_to_unix("2026-03-27T09:06:02.482Z").unwrap();
        // Milliseconds should be roughly seconds * 1000.
        assert_eq!(ms / 1000, secs);
    }

    #[test]
    fn test_parse_iso8601_to_millis_exact() {
        // 1711220400_000 ms = 1711220400 s exactly.
        assert_eq!(
            parse_iso8601_to_millis("2024-03-23T19:00:00Z"),
            Some(1_711_220_400_000)
        );
        // Sub-second precision: .482 → 482 ms past the second.
        let ms = parse_iso8601_to_millis("2024-03-23T19:00:00.482Z").unwrap();
        assert_eq!(ms, 1_711_220_400_482);
    }

    #[test]
    fn test_flatten_json() {
        let json: serde_json::Value = serde_json::json!({
            "http": {
                "method": "GET",
                "status_code": 200
            },
            "service": "web"
        });
        let flat = flatten_json_value(&json, "");
        assert_eq!(flat.get("http.method").unwrap(), "GET");
        assert_eq!(flat.get("http.status_code").unwrap(), "200");
        assert_eq!(flat.get("service").unwrap(), "web");
    }

    #[test]
    fn test_flatten_json_skips_null_and_array() {
        let json: serde_json::Value = serde_json::json!({
            "name": "test",
            "null_field": null,
            "array_field": [1, 2, 3],
            "bool_field": true
        });
        let flat = flatten_json_value(&json, "");
        assert_eq!(flat.get("name").unwrap(), "test");
        assert_eq!(flat.get("bool_field").unwrap(), "true");
        assert!(!flat.contains_key("null_field"));
        assert!(!flat.contains_key("array_field"));
    }

    #[test]
    fn test_parse_metric_type() {
        assert_eq!(parse_metric_type("gauge"), MetricType::Gauge);
        assert_eq!(parse_metric_type("count"), MetricType::Counter);
        assert_eq!(parse_metric_type("rate"), MetricType::Counter);
        assert_eq!(parse_metric_type("counter"), MetricType::Counter);
        assert_eq!(parse_metric_type("histogram"), MetricType::Histogram);
        assert_eq!(parse_metric_type("distribution"), MetricType::Histogram);
        assert_eq!(parse_metric_type("unknown_type"), MetricType::Unknown);
    }

    #[test]
    fn test_convert_metric_query_empty() {
        let resp = DdMetricQueryResponse {
            status: "ok".to_string(),
            series: vec![],
            error: None,
        };
        let result = convert_metric_query(resp);
        assert_eq!(result.total_count, 0);
        assert!(result.series.is_empty());
        assert_eq!(result.result_type, MetricResultType::Matrix);
    }

    #[test]
    fn test_tag_set_to_labels() {
        let series = super::super::response::DdSeries {
            metric: "test.metric".to_string(),
            pointlist: vec![],
            interval: None,
            scope: None,
            tag_set: vec!["host:web01".to_string(), "env:prod".to_string()],
            unit: None,
        };
        let converted = convert_series(series);
        assert_eq!(converted.labels.get("host").unwrap(), "web01");
        assert_eq!(converted.labels.get("env").unwrap(), "prod");
    }

    #[test]
    fn test_convert_metric_metadata_full() {
        let resp = DdMetricMetadata {
            description: Some("CPU usage total".to_string()),
            metric_type: Some("count".to_string()),
            unit: Some("nanosecond".to_string()),
            per_unit: Some("second".to_string()),
        };
        let details = convert_metric_metadata("cpu.usage", resp);
        assert_eq!(details.len(), 1);
        assert_eq!(details[0].name, "cpu.usage");
        assert_eq!(details[0].metric_type, Some(MetricType::Counter));
        assert_eq!(details[0].description.as_deref(), Some("CPU usage total"));
        assert_eq!(details[0].unit.as_deref(), Some("nanosecond/second"));
    }

    #[test]
    fn test_convert_metric_metadata_empty_fields() {
        let resp = DdMetricMetadata {
            description: Some(String::new()),
            metric_type: None,
            unit: Some(String::new()),
            per_unit: None,
        };
        let details = convert_metric_metadata("m", resp);
        assert!(details[0].description.is_none());
        assert!(details[0].unit.is_none());
        assert!(details[0].metric_type.is_none());
    }

    #[test]
    fn test_convert_log_entry_basic() {
        let event = DdLogEvent {
            id: Some("log-123".to_string()),
            attributes: super::super::response::DdLogAttributes {
                message: Some("Connection failed".to_string()),
                status: Some("error".to_string()),
                timestamp: Some("2026-03-27T09:06:02.482Z".to_string()),
                service: Some("api-gateway".to_string()),
                host: Some("web01".to_string()),
                tags: vec!["env:prod".to_string(), "team:backend".to_string()],
                attributes: Some(serde_json::json!({
                    "otel": { "trace_id": "abc123", "span_id": "def456" },
                    "http": { "method": "GET" }
                })),
            },
        };
        let entry = convert_log_entry(event);
        assert!(entry.timestamp > 0);
        assert_eq!(entry.message, "Connection failed");
        assert_eq!(entry.severity, Some(Severity::Error));
        assert_eq!(entry.service.as_deref(), Some("api-gateway"));
        assert_eq!(entry.source.as_deref(), Some("web01"));
        assert_eq!(entry.id.as_deref(), Some("log-123"));
        assert_eq!(entry.trace_id.as_deref(), Some("abc123"));
        assert_eq!(entry.span_id.as_deref(), Some("def456"));
        // Flattened attributes should contain http.method.
        let attrs = entry.attributes.unwrap();
        assert_eq!(attrs.get("http.method").unwrap(), "GET");
        // Resource from tags.
        let res = entry.resource.unwrap();
        assert_eq!(res.get("env").unwrap(), "prod");
    }

    #[test]
    fn test_convert_log_entry_minimal() {
        let event = DdLogEvent {
            id: None,
            attributes: super::super::response::DdLogAttributes {
                message: None,
                status: None,
                timestamp: None,
                service: None,
                host: None,
                tags: vec![],
                attributes: None,
            },
        };
        let entry = convert_log_entry(event);
        assert_eq!(entry.timestamp, 0);
        assert_eq!(entry.message, "");
        assert!(entry.severity.is_none());
        assert!(entry.service.is_none());
        assert!(entry.source.is_none());
        assert!(entry.id.is_none());
        assert!(entry.trace_id.is_none());
        assert!(entry.resource.is_none());
    }

    #[test]
    fn test_convert_span_basic() {
        let event = DdSpanEvent {
            attributes: DdSpanAttributes {
                operation_name: Some("http.client.request".to_string()),
                resource_name: Some("GET".to_string()),
                service: Some("frontend".to_string()),
                trace_id: Some("abc123".to_string()),
                span_id: Some("def456".to_string()),
                parent_id: Some("parent789".to_string()),
                status: Some("error".to_string()),
                start_timestamp: Some("2026-03-27T09:06:07.765Z".to_string()),
                end_timestamp: Some("2026-03-27T09:06:07.769Z".to_string()),
                span_type: Some("http".to_string()),
                env: Some("prod".to_string()),
                host: None,
                tags: vec![],
                custom: Some(serde_json::json!({
                    "duration": 4197591,
                    "span": { "kind": "client" }
                })),
            },
        };
        let span = convert_span(&event);
        assert_eq!(span.trace_id, "abc123");
        assert_eq!(span.span_id, "def456");
        assert_eq!(span.parent_span_id.as_deref(), Some("parent789"));
        assert_eq!(span.name, "http.client.request");
        assert_eq!(span.service, "frontend");
        assert_eq!(span.status, SpanStatus::Error);
        assert_eq!(span.kind, Some(SpanKind::Client));
        // duration: 4197591 ns / 1000 = 4197 μs
        assert_eq!(span.duration_us, 4197);
        assert!(span.start_time > 0);
        // Extensions should contain datadog.type and datadog.resource_name.
        let ext = span.extensions.unwrap();
        assert_eq!(ext.get("datadog.type").unwrap(), "http");
        assert_eq!(ext.get("datadog.resource_name").unwrap(), "GET");
        assert_eq!(ext.get("datadog.env").unwrap(), "prod");
    }

    #[test]
    fn test_convert_span_root_span() {
        let event = DdSpanEvent {
            attributes: DdSpanAttributes {
                operation_name: Some("Internal".to_string()),
                resource_name: None,
                service: Some("svc".to_string()),
                trace_id: Some("t1".to_string()),
                span_id: Some("s1".to_string()),
                parent_id: Some("0".to_string()),
                status: Some("ok".to_string()),
                start_timestamp: None,
                end_timestamp: None,
                span_type: None,
                env: None,
                host: None,
                tags: vec![],
                custom: None,
            },
        };
        let span = convert_span(&event);
        // "0" parent_id should become None (root span).
        assert!(span.parent_span_id.is_none());
        assert_eq!(span.status, SpanStatus::Ok);
        assert!(span.kind.is_none());
    }

    #[test]
    fn test_extract_duration_fallback_to_timestamps() {
        // When custom.duration is absent, should compute from timestamps.
        let attrs = DdSpanAttributes {
            operation_name: None,
            resource_name: None,
            service: None,
            trace_id: None,
            span_id: None,
            parent_id: None,
            status: None,
            start_timestamp: Some("2026-03-27T09:06:07.000Z".to_string()),
            end_timestamp: Some("2026-03-27T09:06:07.500Z".to_string()),
            span_type: None,
            env: None,
            host: None,
            tags: vec![],
            custom: None,
        };
        let dur = extract_duration_us(&attrs);
        // 500ms difference = 500_000 μs
        assert_eq!(dur, 500_000);
    }

    #[test]
    fn test_format_unix_to_iso8601() {
        // Epoch zero → UTC midnight 1970-01-01.
        assert_eq!(
            super::super::format_unix_to_iso8601(0),
            "1970-01-01T00:00:00Z"
        );
        // Known fixed point: 1711234800 = 2024-03-23T23:00:00Z
        assert_eq!(
            super::super::format_unix_to_iso8601(1_711_234_800),
            "2024-03-23T23:00:00Z"
        );
        // Negative timestamp: -1 = 1969-12-31T23:59:59Z
        assert_eq!(
            super::super::format_unix_to_iso8601(-1),
            "1969-12-31T23:59:59Z"
        );
        // Out-of-range values must fall back to the raw integer string, not panic.
        let s = super::super::format_unix_to_iso8601(i64::MAX);
        assert_eq!(s, i64::MAX.to_string());
        let s = super::super::format_unix_to_iso8601(i64::MIN);
        assert_eq!(s, i64::MIN.to_string());
    }

    // --- Fixture-based tests ---

    fn fixture_dir() -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/datadog")
    }

    fn load_fixture(subdir: &str, name: &str) -> serde_json::Value {
        let path = fixture_dir().join(subdir).join(name);
        let content = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("fixture {}: {e}", path.display()));
        serde_json::from_str(&content).expect("invalid fixture JSON")
    }

    /// Extract `response.body` from a fixture wrapper as a typed response.
    fn get_body<T: serde::de::DeserializeOwned>(f: &serde_json::Value) -> T {
        serde_json::from_value(f["response"]["body"].clone())
            .expect("failed to parse response body")
    }

    // -- Metric fixtures --

    #[test]
    fn fixture_metric_query_cpu() {
        let f = load_fixture("metrics", "metric-query-cpu.json");
        let resp: DdMetricQueryResponse = get_body(&f);
        assert_eq!(resp.status, "ok");
        assert!(!resp.series.is_empty());

        let result = convert_metric_query(resp);
        assert!(result.total_count > 0);
        assert_eq!(result.result_type, MetricResultType::Matrix);

        for series in &result.series {
            assert!(!series.name.is_empty());
            assert!(!series.points.is_empty());
            assert!(series.stats.is_some());
            // Timestamps should be in seconds (not milliseconds).
            for point in &series.points {
                assert!(
                    point.timestamp < 10_000_000_000,
                    "timestamp {} looks like milliseconds, expected seconds",
                    point.timestamp
                );
            }
        }
    }

    #[test]
    fn fixture_metric_query_empty() {
        let f = load_fixture("metrics", "metric-query-empty.json");
        let resp: DdMetricQueryResponse = get_body(&f);
        let result = convert_metric_query(resp);
        assert_eq!(result.total_count, 0);
        assert!(result.series.is_empty());
    }

    #[test]
    fn fixture_metric_search() {
        let f = load_fixture("metrics", "metric-search-container.json");
        let resp: DdSearchResponse = get_body(&f);
        let names = convert_metric_search(resp);
        assert!(!names.is_empty());
        // All names should start with "container."
        for name in &names {
            assert!(
                name.starts_with("container."),
                "expected container.* prefix, got: {name}"
            );
        }
    }

    #[test]
    fn fixture_metric_metadata() {
        let f = load_fixture("metrics", "metric-metadata.json");
        let resp: DdMetricMetadata = get_body(&f);
        let details = convert_metric_metadata("container.cpu.usage.total", resp);
        assert_eq!(details.len(), 1);
        assert_eq!(details[0].name, "container.cpu.usage.total");
        // Fixture has description and type.
        assert!(details[0].description.is_some());
        assert!(details[0].metric_type.is_some());
    }

    // -- Log fixtures --

    #[test]
    fn fixture_logs_search() {
        let f = load_fixture("logs", "logs-search.json");
        let resp: DdLogsResponse = get_body(&f);
        assert!(!resp.data.is_empty());

        let result = convert_logs_search(resp);
        assert!(result.total_count > 0);
        for entry in &result.entries {
            assert!(entry.timestamp > 0);
            assert!(!entry.message.is_empty());
            assert!(entry.severity.is_some());
            assert!(entry.service.is_some());
        }
    }

    #[test]
    fn fixture_logs_search_empty() {
        let f = load_fixture("logs", "logs-search-empty.json");
        let resp: DdLogsResponse = get_body(&f);
        let result = convert_logs_search(resp);
        assert_eq!(result.total_count, 0);
        assert!(result.entries.is_empty());
        assert_eq!(result.is_complete, Some(true));
    }

    #[test]
    fn fixture_logs_search_error_severity() {
        let f = load_fixture("logs", "logs-search-error.json");
        let resp: DdLogsResponse = get_body(&f);
        let result = convert_logs_search(resp);
        for entry in &result.entries {
            assert_eq!(
                entry.severity,
                Some(Severity::Error),
                "expected error severity for log: {}",
                entry.message
            );
        }
    }

    // -- Trace fixtures --

    #[test]
    fn fixture_traces_search() {
        let f = load_fixture("traces", "traces-search.json");
        let resp: DdSpansResponse = get_body(&f);
        assert!(!resp.data.is_empty());

        let result = convert_spans_search(resp);
        assert!(result.total_count > 0);
        for span in &result.spans {
            assert!(!span.trace_id.is_empty());
            assert!(!span.span_id.is_empty());
            assert!(!span.service.is_empty());
            assert!(span.duration_us >= 0);
        }
    }

    #[test]
    fn fixture_traces_search_empty() {
        let f = load_fixture("traces", "traces-search-empty.json");
        let resp: DdSpansResponse = get_body(&f);
        let result = convert_spans_search(resp);
        assert_eq!(result.total_count, 0);
        assert!(result.spans.is_empty());
        // No cursor in empty response → complete.
        assert_eq!(result.is_complete, Some(true));
    }

    #[test]
    fn fixture_traces_search_error_status() {
        let f = load_fixture("traces", "traces-search-error.json");
        let resp: DdSpansResponse = get_body(&f);
        let result = convert_spans_search(resp);
        for span in &result.spans {
            assert_eq!(
                span.status,
                SpanStatus::Error,
                "expected error status for span: {}",
                span.name
            );
        }
    }

    #[test]
    fn fixture_trace_by_id() {
        let f = load_fixture("traces", "traces-by-id.json");
        let resp: DdSpansResponse = get_body(&f);
        assert!(!resp.data.is_empty());

        let detail = convert_trace_detail("64a5507a20d48041ac35dea4d3a02054", &resp);
        assert_eq!(detail.trace_id, "64a5507a20d48041ac35dea4d3a02054");
        assert!(detail.span_count > 0);
        assert!(detail.service_count > 0);
        assert!(!detail.services.is_empty());

        // Verify spans are sorted by start_time ascending.
        let times: Vec<i64> = detail.spans.iter().map(|s| s.start_time).collect();
        let mut sorted = times.clone();
        sorted.sort();
        assert_eq!(times, sorted, "spans should be sorted by start_time");
    }

    #[test]
    fn fixture_trace_by_id_not_found() {
        let f = load_fixture("traces", "traces-by-id-not-found.json");
        let resp: DdSpansResponse = get_body(&f);
        assert!(resp.data.is_empty());

        let detail = convert_trace_detail("00000000000000000000000000000000", &resp);
        assert_eq!(detail.span_count, 0);
    }
}
