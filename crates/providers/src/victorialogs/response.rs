//! `VictoriaLogs` API response deserialization.
//!
//! The `/select/logsql/query` endpoint returns NDJSON (`application/stream+json`),
//! where each line is a JSON object with **all string values**. Other endpoints
//! (hits, stats, streams, `field_names`, `field_values`) return standard JSON.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// A single log entry from `VictoriaLogs` NDJSON response.
///
/// All fields are string-valued. The entry is deserialized as a flat
/// `BTreeMap<String, String>` because the field set is fully dynamic.
///
/// # Known fields
///
/// | Field | Description |
/// |-------|-------------|
/// | `_msg` | Log message body |
/// | `_time` | Nanosecond-precision RFC3339 timestamp |
/// | `_stream` | Stream identifier in `LogsQL` format: `{key="val",...}` |
/// | `_stream_id` | Stream unique ID (48-char hex) |
/// | `severity` | Log level (case-insensitive, needs normalization) |
/// | `service.name` | Service name |
/// | `host.name` | Hostname |
/// | `trace_id` | Trace ID (32-char hex, if present) |
/// | `span_id` | Span ID (16-char hex, if present) |
pub(crate) type VlEntry = BTreeMap<String, String>;

/// Parse an NDJSON response body into a list of entries.
///
/// Each line is a separate JSON object. Empty lines and the trailing
/// newline are skipped. Returns an empty vec for empty responses.
///
/// # Errors
///
/// Returns [`ObzError::Provider`] if any line fails to parse as JSON.
pub(crate) fn parse_ndjson(body: &str) -> obz_core::provider::ProviderResult<Vec<VlEntry>> {
    use obz_core::model::error::{ErrorCode, ObzError};

    let mut entries = Vec::new();
    for (i, line) in body.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let entry: VlEntry = serde_json::from_str(trimmed).map_err(|e| ObzError::Provider {
            code: ErrorCode::BackendError,
            message: format!("failed to parse NDJSON line {}: {e}", i + 1),
            raw_error: Some(crate::util::truncate_for_error(body, 500)),
            recoverable: false,
            suggestion: None,
            doc_url: None,
        })?;
        entries.push(entry);
    }
    Ok(entries)
}

/// Parse the `_stream` field from `LogsQL` format to a label map.
///
/// Input format: `{key1="val1",key2="val2"}`
///
/// Returns an empty map if the format is unrecognized.
pub(crate) fn parse_stream(stream: &str) -> BTreeMap<String, String> {
    let mut result = BTreeMap::new();
    let trimmed = stream.trim();

    // Strip outer braces.
    let inner = if trimmed.starts_with('{') && trimmed.ends_with('}') {
        &trimmed[1..trimmed.len() - 1]
    } else {
        return result;
    };

    if inner.is_empty() {
        return result;
    }

    // Split by comma, then by '=' — handle quoted values.
    // Format: key="value",key2="value2"
    let mut remaining = inner;
    while !remaining.is_empty() {
        // Find key=
        let Some(eq_pos) = remaining.find('=') else {
            break;
        };
        let key = remaining[..eq_pos].trim();
        remaining = &remaining[eq_pos + 1..];

        // Value should be quoted.
        if remaining.starts_with('"') {
            remaining = &remaining[1..]; // skip opening quote
                                         // Find closing quote (handle escaped quotes).
            let mut value = String::new();
            let mut chars = remaining.chars();
            let mut found_close = false;
            while let Some(c) = chars.next() {
                if c == '\\' {
                    if let Some(next) = chars.next() {
                        value.push(next);
                    }
                } else if c == '"' {
                    found_close = true;
                    break;
                } else {
                    value.push(c);
                }
            }
            if found_close {
                result.insert(key.to_string(), value);
            }
            remaining = chars.as_str();
            // Skip comma separator.
            remaining = remaining.trim_start_matches(',');
        } else {
            // Unquoted value — read until comma or end.
            let end = remaining.find(',').unwrap_or(remaining.len());
            let value = remaining[..end].trim();
            result.insert(key.to_string(), value.to_string());
            remaining = if end < remaining.len() {
                &remaining[end + 1..]
            } else {
                ""
            };
        }
    }

    result
}

/// A field name or field value entry with hit count.
#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct VlValueHit {
    /// Field name or field value.
    pub value: String,
    /// Number of matching hits.
    pub hits: u64,
}

/// Response from `field_names` and `field_values` endpoints.
#[derive(Debug, Deserialize)]
pub(crate) struct VlValuesResponse {
    /// Returned values with hit counts.
    #[serde(default)]
    pub values: Vec<VlValueHit>,
}

/// One time-series bucket returned by the `hits` endpoint.
#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct VlHitsSeries {
    /// Grouping fields for this series.
    #[serde(default)]
    pub fields: BTreeMap<String, String>,
    /// Bucket timestamps.
    #[serde(default)]
    pub timestamps: Vec<String>,
    /// Bucket values.
    #[serde(default)]
    pub values: Vec<u64>,
    /// Total hits across all buckets.
    pub total: Option<u64>,
}

/// Response from the `hits` endpoint.
#[derive(Debug, Deserialize)]
pub(crate) struct VlHitsResponse {
    /// Returned hit series.
    #[serde(default)]
    pub hits: Vec<VlHitsSeries>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_ndjson_basic() {
        let body = r#"{"_msg":"hello","severity":"info"}
{"_msg":"world","severity":"error"}
"#;
        let entries = parse_ndjson(body).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0]["_msg"], "hello");
        assert_eq!(entries[1]["severity"], "error");
    }

    #[test]
    fn test_parse_ndjson_empty() {
        let entries = parse_ndjson("").unwrap();
        assert!(entries.is_empty());

        let entries = parse_ndjson("\n\n").unwrap();
        assert!(entries.is_empty());
    }

    #[test]
    fn test_parse_stream_basic() {
        let stream = r#"{host.name="web01",service.name="api"}"#;
        let labels = parse_stream(stream);
        assert_eq!(labels.len(), 2);
        assert_eq!(labels["host.name"], "web01");
        assert_eq!(labels["service.name"], "api");
    }

    #[test]
    fn test_parse_stream_empty() {
        assert!(parse_stream("{}").is_empty());
        assert!(parse_stream("").is_empty());
        assert!(parse_stream("invalid").is_empty());
    }

    #[test]
    fn test_parse_stream_escaped_quote() {
        let stream = r#"{key="val\"ue"}"#;
        let labels = parse_stream(stream);
        assert_eq!(labels["key"], "val\"ue");
    }
}
