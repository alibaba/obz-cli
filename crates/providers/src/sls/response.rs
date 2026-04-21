//! SLS API response deserialization types.
//!
//! The SLS `GetLogs` API returns a JSON array of log/span objects.
//! Response metadata (progress, count) is carried in HTTP headers:
//! - `x-log-progress`: `Complete` or `Incomplete`
//! - `x-log-count`: number of entries returned
//!
//! # Log entry fields
//!
//! | SLS field | obz field | Notes |
//! |-----------|-----------|-------|
//! | `content` | `message` | Log message body |
//! | `timeUnixNano` | `timestamp` | Nanosecond string → Unix seconds |
//! | `severityText` | `severity` | Parsed via `parse_severity()` |
//! | `host` | `source` | Hostname |
//! | `service` | `service` | Service name |
//! | `attribute` | `attributes` | JSON-encoded string → `BTreeMap` |
//! | `resource` | `resource` | JSON-encoded string → `BTreeMap` |
//! | `traceID` | `trace_id` | Empty string filtered out |
//! | `spanID` | `span_id` | Empty string filtered out |
//!
//! # Trace span fields
//!
//! | SLS field | obz field | Notes |
//! |-----------|-----------|-------|
//! | `traceID` | `trace_id` | Hex trace ID |
//! | `spanID` | `span_id` | Hex span ID |
//! | `parentSpanID` | `parent_span_id` | Empty → None |
//! | `name` | `name` | Operation name |
//! | `service` | `service` | Service name |
//! | `kind` | `kind` | client/server/producer/consumer/internal |
//! | `start` | `start_time` | Microsecond string → Unix seconds |
//! | `duration` | `duration_us` | Microsecond string → i64 |
//! | `attribute` | `attributes` | JSON-encoded string → `BTreeMap` |
//! | `resource` | `resource` | JSON-encoded string → `BTreeMap` |
//! | `statusCode` | `status` | OK/ERROR/UNSET → `SpanStatus` |
//! | `logs` | `events` | JSON-encoded string → `Vec<SpanEvent>` |

use std::collections::BTreeMap;

use serde::Deserialize;

/// A single SLS log/trace entry from the `GetLogs` API response.
///
/// SLS returns all field values as JSON strings. The entry is
/// deserialized as a flat `BTreeMap<String, serde_json::Value>`
/// because fields like `attribute` contain nested JSON strings
/// while other fields like `severityNumber` are plain strings.
pub(crate) type SlsEntry = BTreeMap<String, serde_json::Value>;

/// Parse a SLS `GetLogs` JSON array response body.
///
/// The response body is a JSON array of entry objects.
///
/// # Errors
///
/// Returns [`ObzError::Provider`] if the body fails to parse as JSON.
pub(crate) fn parse_sls_response(body: &str) -> obz_core::provider::ProviderResult<Vec<SlsEntry>> {
    use obz_core::model::error::{ErrorCode, ObzError};

    // SLS returns `[]` for empty results.
    let entries: Vec<SlsEntry> = serde_json::from_str(body).map_err(|e| ObzError::Provider {
        code: ErrorCode::BackendError,
        message: format!("failed to parse SLS response: {e}"),
        raw_error: Some(crate::util::truncate_for_error(body, 500)),
        recoverable: false,
        suggestion: None,
        doc_url: None,
    })?;
    Ok(entries)
}

/// Parse a JSON-encoded string field into a `BTreeMap<String, String>`.
///
/// SLS stores `attribute` and `resource` fields as JSON-encoded strings
/// (e.g. `"{\"key\":\"value\"}"`). This function deserializes them into
/// a flat string map.
///
/// Returns an empty map if parsing fails or the value is not an object.
pub(crate) fn parse_json_string_field(value: &serde_json::Value) -> BTreeMap<String, String> {
    let Some(s) = value.as_str() else {
        return BTreeMap::new();
    };
    // Try to parse as a JSON object.
    let Ok(obj) = serde_json::from_str::<BTreeMap<String, serde_json::Value>>(s) else {
        return BTreeMap::new();
    };
    obj.into_iter()
        .map(|(k, v)| {
            let val = match &v {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Number(n) => n.to_string(),
                serde_json::Value::Bool(b) => b.to_string(),
                serde_json::Value::Null => String::new(),
                other => other.to_string(),
            };
            (k, val)
        })
        .collect()
}

/// Parsed span event from the `logs` JSON string field in SLS trace entries.
#[derive(Debug, Deserialize)]
pub(crate) struct SlsSpanLog {
    /// Timestamp of the event in microseconds since Unix epoch (may be missing).
    #[serde(default)]
    pub timestamp: Option<i64>,
    /// Event attributes.
    #[serde(default)]
    pub attribute: Option<BTreeMap<String, serde_json::Value>>,
}

/// Parse the `logs` field from an SLS trace entry into span events.
///
/// The `logs` field is a JSON-encoded array string, e.g.
/// `"[{\"timestamp\":123,\"attribute\":{\"exception.message\":\"...\"}}]"`.
///
/// Returns an empty vec if parsing fails or the field is empty/`[]`.
pub(crate) fn parse_span_logs(value: &serde_json::Value) -> Vec<SlsSpanLog> {
    let s = match value.as_str() {
        Some(s) if s != "[]" => s,
        _ => return Vec::new(),
    };
    serde_json::from_str(s).unwrap_or_default()
}

/// Extract a string value from an SLS entry field.
///
/// Handles both string values and other JSON types.
pub(crate) fn field_as_str(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Null => String::new(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_sls_response_basic() {
        let body = r#"[{"content":"hello","service":"api"},{"content":"world","service":"web"}]"#;
        let entries = parse_sls_response(body).unwrap();
        assert_eq!(entries.len(), 2);
    }

    #[test]
    fn test_parse_sls_response_empty() {
        let entries = parse_sls_response("[]").unwrap();
        assert!(entries.is_empty());
    }

    #[test]
    fn test_parse_json_string_field_basic() {
        let val = serde_json::json!("{\"key1\":\"value1\",\"key2\":42}");
        let map = parse_json_string_field(&val);
        assert_eq!(map["key1"], "value1");
        assert_eq!(map["key2"], "42");
    }

    #[test]
    fn test_parse_json_string_field_empty() {
        let val = serde_json::json!("{}");
        let map = parse_json_string_field(&val);
        assert!(map.is_empty());
    }

    #[test]
    fn test_parse_json_string_field_invalid() {
        let val = serde_json::json!("not json");
        let map = parse_json_string_field(&val);
        assert!(map.is_empty());
    }

    #[test]
    fn test_parse_json_string_field_null() {
        let val = serde_json::Value::Null;
        let map = parse_json_string_field(&val);
        assert!(map.is_empty());
    }

    #[test]
    fn test_parse_span_logs_basic() {
        let val = serde_json::json!("[{\"timestamp\":123,\"attribute\":{\"key\":\"val\"}}]");
        let logs = parse_span_logs(&val);
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].timestamp, Some(123));
    }

    #[test]
    fn test_parse_span_logs_empty_array() {
        let val = serde_json::json!("[]");
        let logs = parse_span_logs(&val);
        assert!(logs.is_empty());
    }

    #[test]
    fn test_parse_sls_response_invalid_json() {
        let result = parse_sls_response("not json at all");
        assert!(result.is_err());
        let err = result.unwrap_err();
        match err {
            obz_core::model::error::ObzError::Provider { code, .. } => {
                assert_eq!(code, obz_core::model::error::ErrorCode::BackendError);
            }
            _ => panic!("expected Provider error"),
        }
    }

    #[test]
    fn test_parse_sls_response_html_error() {
        // SLS may return HTML error pages for auth failures.
        let result = parse_sls_response("<html><body>Forbidden</body></html>");
        assert!(result.is_err());
    }

    #[test]
    fn test_field_as_str_string() {
        let val = serde_json::json!("hello");
        assert_eq!(field_as_str(&val), "hello");
    }

    #[test]
    fn test_field_as_str_number() {
        let val = serde_json::json!(42);
        assert_eq!(field_as_str(&val), "42");
    }

    #[test]
    fn test_field_as_str_bool() {
        let val = serde_json::json!(true);
        assert_eq!(field_as_str(&val), "true");
    }

    #[test]
    fn test_field_as_str_null() {
        let val = serde_json::Value::Null;
        assert_eq!(field_as_str(&val), "");
    }

    #[test]
    fn test_field_as_str_object() {
        let val = serde_json::json!({"key": "val"});
        let result = field_as_str(&val);
        assert!(result.contains("key"));
    }
}
