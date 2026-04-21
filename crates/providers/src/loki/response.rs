//! Loki API response deserialization types.
//!
//! Loki's query API (`/loki/api/v1/query_range`) returns JSON with a
//! `streams` result type containing log streams. Each stream is a set of
//! label key-value pairs plus an array of `[nanosecond_timestamp, log_line]`
//! value tuples.
//!
//! The labels and label-values endpoints return the same envelope as
//! Prometheus (`{ "status": "success", "data": [...] }`).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Top-level Loki API response envelope.
///
/// Used for query endpoints that return structured JSON. Error responses
/// from Loki are typically `text/plain` and handled before deserialization.
#[derive(Debug, Deserialize)]
pub(crate) struct LokiResponse<T> {
    /// `"success"` or `"error"`.
    pub status: String,
    /// Response payload.
    pub data: Option<T>,
    /// Error message (present when `status == "error"`).
    #[serde(default)]
    pub error: Option<String>,
    /// Error type (present when `status == "error"`).
    #[serde(default, rename = "errorType")]
    pub error_type: Option<String>,
}

/// Query result data from `/loki/api/v1/query_range`.
#[derive(Debug, Deserialize)]
pub(crate) struct LokiQueryData {
    /// Result type — `"streams"` for log queries, `"matrix"` for metric queries.
    #[allow(dead_code)]
    #[serde(rename = "resultType")]
    pub result_type: String,
    /// Log stream results.
    pub result: Vec<LokiStream>,
}

/// A single log stream returned by Loki.
///
/// Each stream has a unique set of labels and contains one or more log entries.
#[derive(Debug, Deserialize)]
pub(crate) struct LokiStream {
    /// Label key-value pairs identifying this stream.
    ///
    /// Common labels include `service_name`, `severity_text`, `detected_level`,
    /// `host_name`, `trace_id`, `span_id`, etc.
    pub stream: BTreeMap<String, String>,

    /// Log entry values as `[nanosecond_timestamp_string, log_line_string]` tuples.
    ///
    /// Timestamps are nanosecond Unix epoch strings (e.g., `"1775538346468275580"`).
    pub values: Vec<(String, String)>,
}

/// Response from `/loki/api/v1/detected_fields`.
#[derive(Debug, Deserialize)]
pub(crate) struct LokiDetectedFieldsResponse {
    /// Detected fields in the queried log content.
    #[serde(default, deserialize_with = "deserialize_fields")]
    pub fields: Vec<LokiDetectedField>,
}

fn deserialize_fields<'de, D>(deserializer: D) -> Result<Vec<LokiDetectedField>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Option::<Vec<LokiDetectedField>>::deserialize(deserializer)?.unwrap_or_default())
}

/// A detected log field with metadata returned by Loki.
#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct LokiDetectedField {
    /// Field label name.
    pub label: String,
    /// Field type.
    #[serde(rename = "type")]
    pub field_type: String,
    /// Number of unique values observed for this field.
    pub cardinality: u64,
    /// Parsers that contributed to field detection.
    #[serde(default, deserialize_with = "deserialize_string_vec")]
    pub parsers: Vec<String>,
}

fn deserialize_string_vec<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Option::<Vec<String>>::deserialize(deserializer)?.unwrap_or_default())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Load a fixture file and extract the response body.
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

    #[test]
    fn deserialize_query_all() {
        let body = load_fixture("query-all");
        let resp: LokiResponse<LokiQueryData> = serde_json::from_value(body).unwrap();
        assert_eq!(resp.status, "success");
        let data = resp.data.unwrap();
        assert_eq!(data.result_type, "streams");
        assert_eq!(data.result.len(), 20);

        // Verify first stream has expected structure
        let first = &data.result[0];
        assert!(!first.stream.is_empty());
        assert!(!first.values.is_empty());

        // Check nanosecond timestamp format
        let (ts, _msg) = &first.values[0];
        assert!(ts.len() > 10, "expected nanosecond timestamp, got {ts}");
    }

    #[test]
    fn deserialize_query_empty() {
        let body = load_fixture("query-empty");
        let resp: LokiResponse<LokiQueryData> = serde_json::from_value(body).unwrap();
        assert_eq!(resp.status, "success");
        let data = resp.data.unwrap();
        assert!(data.result.is_empty());
    }

    #[test]
    fn deserialize_labels() {
        let body = load_fixture("labels");
        let resp: LokiResponse<Vec<String>> = serde_json::from_value(body).unwrap();
        assert_eq!(resp.status, "success");
        let data = resp.data.unwrap();
        assert!(!data.is_empty());
    }

    #[test]
    fn deserialize_label_values() {
        let body = load_fixture("label-values-service");
        let resp: LokiResponse<Vec<String>> = serde_json::from_value(body).unwrap();
        assert_eq!(resp.status, "success");
        let data = resp.data.unwrap();
        assert!(data.len() >= 2);
    }

    #[test]
    fn deserialize_detected_fields() {
        let body = serde_json::json!({
            "fields": [
                {
                    "label": "trace_id",
                    "type": "string",
                    "cardinality": 12,
                    "parsers": ["json"]
                }
            ]
        });

        let resp: LokiDetectedFieldsResponse = serde_json::from_value(body).unwrap();
        let fields = resp.fields;
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].label, "trace_id");
        assert_eq!(fields[0].field_type, "string");
    }

    #[test]
    fn deserialize_detected_fields_null_as_empty() {
        let body = serde_json::json!({
            "fields": null
        });

        let resp: LokiDetectedFieldsResponse = serde_json::from_value(body).unwrap();
        assert!(resp.fields.is_empty());
    }

    #[test]
    fn deserialize_detected_field_null_parsers_as_empty() {
        let body = serde_json::json!({
            "fields": [
                {
                    "label": "trace_id",
                    "type": "string",
                    "cardinality": 12,
                    "parsers": null
                }
            ]
        });

        let resp: LokiDetectedFieldsResponse = serde_json::from_value(body).unwrap();
        assert!(resp.fields[0].parsers.is_empty());
    }

    #[test]
    fn deserialize_detected_fields_missing_as_empty() {
        let body = serde_json::json!({});

        let resp: LokiDetectedFieldsResponse = serde_json::from_value(body).unwrap();
        assert!(resp.fields.is_empty());
    }

    #[test]
    fn deserialize_series() {
        let body = load_fixture("series");
        let resp: LokiResponse<Vec<BTreeMap<String, String>>> =
            serde_json::from_value(body).unwrap();
        assert_eq!(resp.status, "success");
        let data = resp.data.unwrap();
        assert!(!data.is_empty());
    }

    #[test]
    fn deserialize_query_by_service() {
        let body = load_fixture("query-by-service");
        let resp: LokiResponse<LokiQueryData> = serde_json::from_value(body).unwrap();
        assert_eq!(resp.status, "success");
        let data = resp.data.unwrap();
        assert!(!data.result.is_empty());
    }

    #[test]
    fn deserialize_query_severity_error() {
        let body = load_fixture("query-severity-error");
        let resp: LokiResponse<LokiQueryData> = serde_json::from_value(body).unwrap();
        assert_eq!(resp.status, "success");
    }
}
