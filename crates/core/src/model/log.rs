//! Log data models.
//!
//! Represents log entries normalized from all supported backend providers.
//! All timestamps are Unix seconds. Severity is a typed enum.
//! Attributes are flattened to `BTreeMap<String, String>`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Normalized log severity level.
///
/// Standard levels follow syslog/OTel conventions. Values that cannot be
/// mapped to a standard level are preserved as `Other(String)`.
///
/// Serializes as UPPERCASE (`"ERROR"`, `"WARN"`, etc.).
/// Deserializes case-insensitively — `"error"`, `"Error"`, and `"ERROR"`
/// all produce [`Severity::Error`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Severity {
    /// Finest-grained debugging information.
    Trace,
    /// Debugging information.
    Debug,
    /// Normal informational messages.
    Info,
    /// Warning conditions.
    Warn,
    /// Error conditions.
    Error,
    /// System is unusable / fatal.
    Fatal,
    /// Unrecognized severity — preserves the original string.
    #[serde(untagged)]
    Other(String),
}

impl<'de> Deserialize<'de> for Severity {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Ok(parse_severity(&s))
    }
}

/// A single log entry normalized from any backend provider.
///
/// Core fields (`timestamp`, `message`) are always present.
/// Optional fields are omitted from Agent View when `None` or empty.
/// `resource` and `extensions` are only present in Full View.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LogEntry {
    /// Unix timestamp in seconds.
    pub timestamp: i64,

    /// Log message body.
    pub message: String,

    /// Normalized severity level.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub severity: Option<Severity>,

    /// Source identifier (hostname, IP address).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,

    /// Service name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service: Option<String>,

    /// Unique log entry ID (provider-specific; not all backends emit this).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,

    /// Structured attributes, flattened to string values.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attributes: Option<BTreeMap<String, String>>,

    /// Resource-level attributes (Full View only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource: Option<BTreeMap<String, String>>,

    /// Correlated Trace ID.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,

    /// Correlated Span ID.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub span_id: Option<String>,

    /// Provider-specific metadata (Full View only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

/// Parse a raw severity string into a [`Severity`] enum.
///
/// Maps common aliases (syslog names, numeric levels, abbreviations)
/// to standard levels. Unrecognized values are preserved as `Other`.
pub fn parse_severity(raw: &str) -> Severity {
    match raw.trim().to_lowercase().as_str() {
        "trace" => Severity::Trace,
        "debug" | "7" => Severity::Debug,
        "info" | "informational" | "notice" | "5" | "6" => Severity::Info,
        "warn" | "warning" | "4" => Severity::Warn,
        "error" | "err" | "3" => Severity::Error,
        "fatal" | "critical" | "crit" | "alert" | "emergency" | "emerg" | "0" | "1" | "2" => {
            Severity::Fatal
        }
        _ => Severity::Other(raw.trim().to_string()),
    }
}

/// Convert an OpenTelemetry `severityNumber` (1-24) into a normalized [`Severity`].
///
/// OpenTelemetry defines six 4-value buckets:
/// 1-4 = TRACE, 5-8 = DEBUG, 9-12 = INFO, 13-16 = WARN,
/// 17-20 = ERROR, and 21-24 = FATAL.
///
/// Returns `None` for `0` (unspecified) and values above `24` (undefined).
pub fn severity_from_otel_number(n: u32) -> Option<Severity> {
    match n {
        1..=4 => Some(Severity::Trace),
        5..=8 => Some(Severity::Debug),
        9..=12 => Some(Severity::Info),
        13..=16 => Some(Severity::Warn),
        17..=20 => Some(Severity::Error),
        21..=24 => Some(Severity::Fatal),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_severity_standard() {
        assert_eq!(parse_severity("trace"), Severity::Trace);
        assert_eq!(parse_severity("TRACE"), Severity::Trace);
        assert_eq!(parse_severity("debug"), Severity::Debug);
        assert_eq!(parse_severity("info"), Severity::Info);
        assert_eq!(parse_severity("warn"), Severity::Warn);
        assert_eq!(parse_severity("error"), Severity::Error);
        assert_eq!(parse_severity("fatal"), Severity::Fatal);
    }

    #[test]
    fn test_parse_severity_aliases() {
        assert_eq!(parse_severity("warning"), Severity::Warn);
        assert_eq!(parse_severity("err"), Severity::Error);
        assert_eq!(parse_severity("critical"), Severity::Fatal);
        assert_eq!(parse_severity("crit"), Severity::Fatal);
        assert_eq!(parse_severity("emerg"), Severity::Fatal);
        assert_eq!(parse_severity("informational"), Severity::Info);
        assert_eq!(parse_severity("notice"), Severity::Info);
    }

    #[test]
    fn test_parse_severity_syslog_numeric() {
        assert_eq!(parse_severity("0"), Severity::Fatal);
        assert_eq!(parse_severity("3"), Severity::Error);
        assert_eq!(parse_severity("4"), Severity::Warn);
        assert_eq!(parse_severity("6"), Severity::Info);
        assert_eq!(parse_severity("7"), Severity::Debug);
    }

    #[test]
    fn test_parse_severity_unknown() {
        assert_eq!(
            parse_severity("custom_level"),
            Severity::Other("custom_level".to_string())
        );
        assert_eq!(
            parse_severity(" verbose "),
            Severity::Other("verbose".to_string())
        );
    }

    #[test]
    fn test_parse_severity_trims_whitespace_in_other() {
        // Whitespace is trimmed even for unrecognized values
        assert_eq!(
            parse_severity(" custom_level "),
            Severity::Other("custom_level".to_string())
        );
    }

    #[test]
    fn test_parse_severity_trims_whitespace() {
        assert_eq!(parse_severity(" error "), Severity::Error);
        assert_eq!(parse_severity(" WARN"), Severity::Warn);
        assert_eq!(parse_severity("info "), Severity::Info);
    }

    #[test]
    fn test_severity_from_otel_number_buckets() {
        assert_eq!(severity_from_otel_number(1), Some(Severity::Trace));
        assert_eq!(severity_from_otel_number(4), Some(Severity::Trace));
        assert_eq!(severity_from_otel_number(5), Some(Severity::Debug));
        assert_eq!(severity_from_otel_number(8), Some(Severity::Debug));
        assert_eq!(severity_from_otel_number(9), Some(Severity::Info));
        assert_eq!(severity_from_otel_number(12), Some(Severity::Info));
        assert_eq!(severity_from_otel_number(13), Some(Severity::Warn));
        assert_eq!(severity_from_otel_number(16), Some(Severity::Warn));
        assert_eq!(severity_from_otel_number(17), Some(Severity::Error));
        assert_eq!(severity_from_otel_number(20), Some(Severity::Error));
        assert_eq!(severity_from_otel_number(21), Some(Severity::Fatal));
        assert_eq!(severity_from_otel_number(24), Some(Severity::Fatal));
    }

    #[test]
    fn test_severity_from_otel_number_unspecified() {
        assert_eq!(severity_from_otel_number(0), None);
    }

    #[test]
    fn test_severity_from_otel_number_out_of_range() {
        assert_eq!(severity_from_otel_number(25), None);
        assert_eq!(severity_from_otel_number(100), None);
    }

    #[test]
    fn test_severity_json_serialization() {
        assert_eq!(
            serde_json::to_string(&Severity::Error).unwrap(),
            r#""ERROR""#
        );
        assert_eq!(serde_json::to_string(&Severity::Warn).unwrap(), r#""WARN""#);
        assert_eq!(
            serde_json::to_string(&Severity::Other("verbose".into())).unwrap(),
            r#""verbose""#
        );
    }

    #[test]
    fn test_severity_json_deserialization_case_insensitive() {
        // Uppercase — canonical form
        let s: Severity = serde_json::from_str(r#""ERROR""#).unwrap();
        assert_eq!(s, Severity::Error);

        // Lowercase — must also map to standard variant
        let s: Severity = serde_json::from_str(r#""error""#).unwrap();
        assert_eq!(s, Severity::Error);

        // Mixed case
        let s: Severity = serde_json::from_str(r#""Error""#).unwrap();
        assert_eq!(s, Severity::Error);

        // All standard levels (lowercase)
        assert_eq!(
            serde_json::from_str::<Severity>(r#""trace""#).unwrap(),
            Severity::Trace
        );
        assert_eq!(
            serde_json::from_str::<Severity>(r#""debug""#).unwrap(),
            Severity::Debug
        );
        assert_eq!(
            serde_json::from_str::<Severity>(r#""info""#).unwrap(),
            Severity::Info
        );
        assert_eq!(
            serde_json::from_str::<Severity>(r#""warn""#).unwrap(),
            Severity::Warn
        );
        assert_eq!(
            serde_json::from_str::<Severity>(r#""fatal""#).unwrap(),
            Severity::Fatal
        );

        // Aliases via deserialization
        assert_eq!(
            serde_json::from_str::<Severity>(r#""warning""#).unwrap(),
            Severity::Warn
        );
        assert_eq!(
            serde_json::from_str::<Severity>(r#""critical""#).unwrap(),
            Severity::Fatal
        );

        // Unknown → Other (preserved as-is)
        assert_eq!(
            serde_json::from_str::<Severity>(r#""verbose""#).unwrap(),
            Severity::Other("verbose".to_string())
        );
    }

    #[test]
    fn test_severity_json_roundtrip() {
        // Standard variants roundtrip correctly
        for severity in [
            Severity::Trace,
            Severity::Debug,
            Severity::Info,
            Severity::Warn,
            Severity::Error,
            Severity::Fatal,
        ] {
            let json = serde_json::to_string(&severity).unwrap();
            let back: Severity = serde_json::from_str(&json).unwrap();
            assert_eq!(back, severity, "roundtrip failed for {json}");
        }

        // Other with non-colliding value roundtrips
        let other = Severity::Other("verbose".to_string());
        let json = serde_json::to_string(&other).unwrap();
        let back: Severity = serde_json::from_str(&json).unwrap();
        assert_eq!(back, other);
    }

    #[test]
    fn test_log_entry_serialization() {
        let entry = LogEntry {
            timestamp: 1711266323,
            message: "Connection refused".to_string(),
            severity: Some(Severity::Error),
            source: Some("172.16.1.100".to_string()),
            service: Some("api-gateway".to_string()),
            id: None,
            attributes: None,
            resource: None,
            trace_id: None,
            span_id: None,
            extensions: None,
        };

        let json = serde_json::to_value(&entry).unwrap();
        assert_eq!(json["severity"], "ERROR");
        assert!(json.get("id").is_none());
    }
}
