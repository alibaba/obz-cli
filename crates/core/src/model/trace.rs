//! Trace data models.
//!
//! Represents spans and traces normalized from all supported backend
//! providers. All IDs are lowercase hex strings. Timestamps are Unix
//! seconds. Duration is in microseconds (i64) for sub-millisecond
//! precision without floating point.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// A single span in a distributed trace.
///
/// All IDs are lowercase hex strings. `parent_span_id` is `None` for
/// root spans. Attributes are flattened to string values.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Span {
    /// Trace ID in lowercase hex (16 or 32 chars).
    pub trace_id: String,

    /// Span ID in lowercase hex (16 chars).
    pub span_id: String,

    /// Parent span ID. `None` for root spans.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_span_id: Option<String>,

    /// Operation name.
    pub name: String,

    /// Service name.
    pub service: String,

    /// Span kind, normalized to `OTel` `SpanKind`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<SpanKind>,

    /// Span status: ok, error, or unset.
    pub status: SpanStatus,

    /// Start time as Unix seconds.
    pub start_time: i64,

    /// Duration in microseconds (integer, no floating point precision loss).
    pub duration_us: i64,

    /// Span attributes, flattened to string values.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attributes: Option<BTreeMap<String, String>>,

    /// Span events (Full View only). Not all providers populate this field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub events: Option<Vec<SpanEvent>>,

    /// Resource attributes (Full View only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource: Option<BTreeMap<String, String>>,

    /// Provider-specific metadata (Full View only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

/// An event within a span (e.g., exception, log message).
///
/// Support varies by backend — some providers populate events while
/// others omit them.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SpanEvent {
    /// Event name.
    pub name: String,

    /// Event timestamp as Unix seconds.
    pub timestamp: i64,

    /// Event attributes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attributes: Option<BTreeMap<String, String>>,
}

/// Span kind, aligned with `OTel` `SpanKind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpanKind {
    /// Synchronous remote call receiver.
    Server,
    /// Synchronous remote call sender.
    Client,
    /// Asynchronous message sender.
    Producer,
    /// Asynchronous message receiver.
    Consumer,
    /// In-process operation.
    Internal,
}

impl SpanKind {
    /// Parse a span kind string (case-insensitive).
    ///
    /// Backends use different casing conventions — `OTel`-based stores
    /// typically use `"Client"` / `"Server"`, while Jaeger uses lowercase
    /// `"client"` / `"server"`.  This method normalises them all.
    ///
    /// Unknown or empty values map to [`SpanKind::Internal`].
    pub fn parse(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "client" => Self::Client,
            "server" => Self::Server,
            "producer" => Self::Producer,
            "consumer" => Self::Consumer,
            _ => Self::Internal,
        }
    }
}

/// Span status, aligned with `OTel` `StatusCode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpanStatus {
    /// Operation completed successfully.
    Ok,
    /// Operation encountered an error.
    Error,
    /// Status was not set.
    Unset,
}

/// Trace detail with pre-computed summary, returned by `trace get`.
///
/// Contains all spans for a single trace plus computed summary fields
/// so AI Agents can assess the trace without iterating over all spans.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TraceDetail {
    /// The queried trace ID.
    pub trace_id: String,

    /// Total number of spans in this trace.
    pub span_count: usize,

    /// Number of distinct services involved.
    pub service_count: usize,

    /// End-to-end duration in microseconds.
    pub duration_us: i64,

    /// List of distinct service names (sorted alphabetically).
    pub services: Vec<String>,

    /// All spans in the trace, sorted by `start_time` ascending.
    pub spans: Vec<Span>,
}

impl TraceDetail {
    /// Compute a trace detail summary from a list of spans.
    ///
    /// Spans are sorted by `start_time` ascending (ties broken by
    /// `duration_us` descending for readability). Summary fields
    /// are computed from the spans — the root span's duration is used
    /// if available, otherwise the time range from earliest start to
    /// latest end is used.
    pub fn from_spans(trace_id: String, mut spans: Vec<Span>) -> Self {
        // Sort by start_time ascending, then duration_us descending.
        spans.sort_by(|a, b| {
            a.start_time
                .cmp(&b.start_time)
                .then(b.duration_us.cmp(&a.duration_us))
        });

        let span_count = spans.len();

        // Collect unique services (sorted).
        let mut services: Vec<String> = spans.iter().map(|s| s.service.clone()).collect();
        services.sort();
        services.dedup();
        let service_count = services.len();

        // Compute end-to-end duration from root span or earliest-to-latest.
        let duration_us = if let Some(root) = spans.iter().find(|s| s.parent_span_id.is_none()) {
            root.duration_us
        } else if !spans.is_empty() {
            // Convert everything to microseconds for consistent arithmetic.
            let earliest_us = spans
                .iter()
                .map(|s| s.start_time * 1_000_000)
                .min()
                .unwrap_or(0);
            let latest_end_us = spans
                .iter()
                .map(|s| s.start_time * 1_000_000 + s.duration_us)
                .max()
                .unwrap_or(0);
            latest_end_us - earliest_us
        } else {
            0
        };

        Self {
            trace_id,
            span_count,
            service_count,
            duration_us,
            services,
            spans,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_span_kind_serialization() {
        assert_eq!(
            serde_json::to_string(&SpanKind::Server).unwrap(),
            r#""server""#
        );
        assert_eq!(
            serde_json::to_string(&SpanKind::Client).unwrap(),
            r#""client""#
        );
        assert_eq!(
            serde_json::to_string(&SpanKind::Internal).unwrap(),
            r#""internal""#
        );
    }

    #[test]
    fn test_span_status_serialization() {
        assert_eq!(serde_json::to_string(&SpanStatus::Ok).unwrap(), r#""ok""#);
        assert_eq!(
            serde_json::to_string(&SpanStatus::Error).unwrap(),
            r#""error""#
        );
        assert_eq!(
            serde_json::to_string(&SpanStatus::Unset).unwrap(),
            r#""unset""#
        );
    }

    #[test]
    fn test_trace_detail_from_spans() {
        let spans = vec![
            Span {
                trace_id: "abc123".to_string(),
                span_id: "span1".to_string(),
                parent_span_id: None,
                name: "GET /api".to_string(),
                service: "gateway".to_string(),
                kind: Some(SpanKind::Server),
                status: SpanStatus::Ok,
                start_time: 1000,
                duration_us: 100_000, // 100ms in microseconds
                attributes: None,
                events: None,
                resource: None,
                extensions: None,
            },
            Span {
                trace_id: "abc123".to_string(),
                span_id: "span2".to_string(),
                parent_span_id: Some("span1".to_string()),
                name: "SELECT".to_string(),
                service: "db".to_string(),
                kind: Some(SpanKind::Client),
                status: SpanStatus::Ok,
                start_time: 1000,
                duration_us: 20_000, // 20ms in microseconds
                attributes: None,
                events: None,
                resource: None,
                extensions: None,
            },
        ];

        let detail = TraceDetail::from_spans("abc123".to_string(), spans);
        assert_eq!(detail.span_count, 2);
        assert_eq!(detail.service_count, 2);
        assert_eq!(detail.duration_us, 100_000); // root span duration (100ms)
        assert_eq!(detail.services, vec!["db", "gateway"]);
    }

    #[test]
    fn test_trace_detail_no_root_span() {
        let spans = vec![
            Span {
                trace_id: "abc".to_string(),
                span_id: "s1".to_string(),
                parent_span_id: Some("missing".to_string()),
                name: "op1".to_string(),
                service: "svc".to_string(),
                kind: None,
                status: SpanStatus::Ok,
                start_time: 1000,
                duration_us: 50_000,
                attributes: None,
                events: None,
                resource: None,
                extensions: None,
            },
            Span {
                trace_id: "abc".to_string(),
                span_id: "s2".to_string(),
                parent_span_id: Some("missing".to_string()),
                name: "op2".to_string(),
                service: "svc".to_string(),
                kind: None,
                status: SpanStatus::Ok,
                start_time: 1000,
                duration_us: 80_000,
                attributes: None,
                events: None,
                resource: None,
                extensions: None,
            },
        ];
        let detail = TraceDetail::from_spans("abc".to_string(), spans);
        // No root span → duration = latest_end - earliest_start
        assert_eq!(detail.duration_us, 80_000);
    }

    #[test]
    fn test_trace_detail_empty_spans() {
        let detail = TraceDetail::from_spans("empty".to_string(), vec![]);
        assert_eq!(detail.span_count, 0);
        assert_eq!(detail.duration_us, 0);
        assert!(detail.services.is_empty());
    }

    #[test]
    fn test_span_kind_parse_capitalized() {
        assert_eq!(SpanKind::parse("Client"), SpanKind::Client);
        assert_eq!(SpanKind::parse("Server"), SpanKind::Server);
        assert_eq!(SpanKind::parse("Producer"), SpanKind::Producer);
        assert_eq!(SpanKind::parse("Consumer"), SpanKind::Consumer);
    }

    #[test]
    fn test_span_kind_parse_lowercase() {
        assert_eq!(SpanKind::parse("client"), SpanKind::Client);
        assert_eq!(SpanKind::parse("server"), SpanKind::Server);
        assert_eq!(SpanKind::parse("producer"), SpanKind::Producer);
        assert_eq!(SpanKind::parse("consumer"), SpanKind::Consumer);
        assert_eq!(SpanKind::parse("internal"), SpanKind::Internal);
    }

    #[test]
    fn test_span_kind_parse_unknown_defaults_to_internal() {
        assert_eq!(SpanKind::parse("unknown"), SpanKind::Internal);
        assert_eq!(SpanKind::parse(""), SpanKind::Internal);
    }
}
