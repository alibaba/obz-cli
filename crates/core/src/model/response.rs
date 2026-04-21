//! Unified response envelope for all obz commands.
//!
//! All CLI output goes through this envelope to ensure consistent structure
//! across all providers and commands. AI Agents can rely on the `status`
//! field to determine success/failure.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::error::ErrorDetail;
use super::log::LogEntry;
use super::metric::{MetricInfoDetail, MetricSeries};
use super::trace::{Span, TraceDetail};
use crate::provider::results::MetricResultType;

/// Unified response envelope.
///
/// All obz commands produce this structure. The `status` field is always
/// `"success"` or `"error"`, allowing AI Agents to reliably parse output.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(bound(
    serialize = "T: Serialize",
    deserialize = "T: serde::de::DeserializeOwned"
))]
pub struct Response<T> {
    /// Response status: "success" or "error".
    pub status: ResponseStatus,

    /// Query metadata (success responses only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<QueryMetadata>,

    /// Response data (success responses only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<T>,

    /// Error detail (error responses only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorDetail>,

    /// Non-fatal warnings from the backend.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warnings: Option<Vec<String>>,
}

/// Response status discriminator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResponseStatus {
    /// The request completed successfully.
    Success,
    /// The request failed.
    Error,
}

/// Query metadata included in success responses.
///
/// Agent View includes only `provider` and `total_count`.
/// Full View additionally includes `provider_type`, `query_language`,
/// `query`, `time_range`, and `is_complete`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryMetadata {
    /// Context name (user-configured), e.g., "dev-vm".
    pub provider: String,

    /// Provider type identifier (Full View only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_type: Option<String>,

    /// Query language used (Full View only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub query_language: Option<String>,

    /// Original query expression (Full View only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,

    /// Query time range (Full View only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time_range: Option<TimeRange>,

    /// Total number of items returned by backend.
    pub total_count: usize,

    /// Number of items after view truncation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub returned_series: Option<usize>,

    /// Whether data is complete (Full View only).
    /// Providers set this to `false` when more results exist beyond the
    /// returned set (e.g., incomplete progress, cursor-based pagination).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_complete: Option<bool>,

    /// Pagination cursor for next page (Full View only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// Time range for a query.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct TimeRange {
    /// Start timestamp (Unix seconds).
    pub start: i64,
    /// End timestamp (Unix seconds).
    pub end: i64,
}

impl<T: Serialize> Response<T> {
    /// Create a success response.
    pub fn success(metadata: QueryMetadata, data: T) -> Self {
        Self {
            status: ResponseStatus::Success,
            metadata: Some(metadata),
            data: Some(data),
            error: None,
            warnings: None,
        }
    }
}

impl<T> Response<T> {
    /// Create an error response.
    ///
    /// Does not require `T: Serialize` since `data` is always `None` for errors.
    pub fn error(error: ErrorDetail) -> Self {
        Self {
            status: ResponseStatus::Error,
            metadata: None,
            data: None,
            error: Some(error),
            warnings: None,
        }
    }
}

// --- Result type discriminators ---
// These constants are the API contract for the `result_type` field.

/// Result type for `metric list`.
pub const RESULT_TYPE_METRIC_LIST: &str = "metric_list";
/// Result type for `metric info`.
pub const RESULT_TYPE_METRIC_INFO: &str = "metric_info";
/// Result type for `metric labels`.
pub const RESULT_TYPE_LABEL_LIST: &str = "label_list";
/// Result type for `metric label-values`.
pub const RESULT_TYPE_LABEL_VALUES: &str = "label_values";
/// Result type for `metric series`.
pub const RESULT_TYPE_SERIES: &str = "series";
/// Result type for `log search`.
pub const RESULT_TYPE_LOG_ENTRIES: &str = "log_entries";
/// Result type for `trace search`.
pub const RESULT_TYPE_SPANS: &str = "spans";
/// Result type for `trace get`.
pub const RESULT_TYPE_TRACE_DETAIL: &str = "trace_detail";

// --- Standard response data types ---
// These define the JSON structure of `Response.data` for each command.
// They are the API contract — AI Agents rely on these field names.

/// Response data for `metric query` (vector/matrix results).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricQueryData {
    /// Result type discriminator.
    pub result_type: MetricResultType,
    /// Query result series.
    pub series: Vec<MetricSeries>,
}

/// Response data for `metric query` (scalar results).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScalarData {
    /// Result type discriminator (always `scalar`).
    pub result_type: MetricResultType,
    /// Scalar value as `[timestamp, value]`.
    pub scalar: (i64, f64),
}

/// Response data for string lists (`metric list`, `metric labels`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StringListData {
    /// Result type discriminator (e.g., `"metric_list"`, `"label_list"`).
    pub result_type: String,
    /// List of string values.
    pub items: Vec<String>,
}

/// Response data for `metric label-values`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LabelValuesData {
    /// Result type discriminator (always `"label_values"`).
    pub result_type: String,
    /// The label name that was queried.
    pub label: String,
    /// Label values.
    pub items: Vec<String>,
}

/// Response data for `metric info`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricInfoData {
    /// Result type discriminator (always `"metric_info"`).
    pub result_type: String,
    /// Metric metadata, or `None` if not found.
    pub info: Option<MetricInfoDetail>,
}

/// Response data for `metric series`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SeriesListData {
    /// Result type discriminator (always `"series"`).
    pub result_type: String,
    /// Series label sets.
    pub series: Vec<BTreeMap<String, String>>,
}

/// Response data for `log search`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogSearchData {
    /// Result type discriminator (always `"log_entries"`).
    pub result_type: String,
    /// Log entries.
    pub entries: Vec<LogEntry>,
}

/// Response data for `trace search`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceSearchData {
    /// Result type discriminator (always `"spans"`).
    pub result_type: String,
    /// Matching spans.
    pub spans: Vec<Span>,
}

/// Response data for `trace get`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceDetailData {
    /// Result type discriminator (always `"trace_detail"`).
    pub result_type: String,
    /// Trace detail with summary and all spans.
    #[serde(flatten)]
    pub detail: TraceDetail,
}

/// Response data for extension commands.
///
/// `data` can be any valid JSON value — a string array for simple lists,
/// an object array for structured results, or a single object.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtensionData {
    /// Result type discriminator (e.g. `"services"`, `"operations"`).
    pub result_type: String,
    /// Result data — arbitrary JSON value.
    pub data: serde_json::Value,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_response_status_serialization() {
        assert_eq!(
            serde_json::to_string(&ResponseStatus::Success).unwrap(),
            r#""success""#
        );
        assert_eq!(
            serde_json::to_string(&ResponseStatus::Error).unwrap(),
            r#""error""#
        );
    }

    #[test]
    fn test_success_response_structure() {
        let metadata = QueryMetadata {
            provider: "dev-vm".to_string(),
            provider_type: None,
            query_language: None,
            query: None,
            time_range: None,
            total_count: 2,
            returned_series: None,
            is_complete: None,
            cursor: None,
        };

        let resp = Response::success(
            metadata,
            serde_json::json!({"result_type": "matrix", "series": []}),
        );

        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["status"], "success");
        assert!(json.get("error").is_none());
        assert_eq!(json["metadata"]["provider"], "dev-vm");
        assert_eq!(json["metadata"]["total_count"], 2);
    }

    #[test]
    fn test_error_response_structure() {
        use super::super::error::{ErrorCategory, ErrorCode};

        let detail = ErrorDetail {
            category: ErrorCategory::Provider,
            code: ErrorCode::QuerySyntax,
            provider: Some("dev-vm".to_string()),
            message: "invalid expression".to_string(),
            raw_error: Some("bad_data".to_string()),
            recoverable: false,
            suggestion: Some("Check PromQL syntax".to_string()),
            doc_url: None,
            source_chain: None,
        };

        let resp: Response<serde_json::Value> = Response::error(detail);

        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["status"], "error");
        assert!(json.get("data").is_none());
        assert_eq!(json["error"]["category"], "provider");
        assert_eq!(json["error"]["code"], "query_syntax");
    }

    #[test]
    fn test_extension_data_serialization_string_list() {
        let metadata = QueryMetadata {
            provider: "dev-vt".to_string(),
            provider_type: None,
            query_language: None,
            query: None,
            time_range: None,
            total_count: 3,
            returned_series: None,
            is_complete: None,
            cursor: None,
        };

        let resp = Response::success(
            metadata,
            ExtensionData {
                result_type: "services".to_string(),
                data: serde_json::json!(["cart", "payment", "frontend"]),
            },
        );

        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["status"], "success");
        assert_eq!(json["metadata"]["total_count"], 3);
        assert_eq!(json["data"]["result_type"], "services");
        let items = json["data"]["data"].as_array().unwrap();
        assert_eq!(items.len(), 3);
        assert_eq!(items[0], "cart");
    }

    #[test]
    fn test_extension_data_serialization_structured() {
        let metadata = QueryMetadata {
            provider: "dev-vt".to_string(),
            provider_type: None,
            query_language: None,
            query: None,
            time_range: None,
            total_count: 2,
            returned_series: None,
            is_complete: None,
            cursor: None,
        };

        let resp = Response::success(
            metadata,
            ExtensionData {
                result_type: "services".to_string(),
                data: serde_json::json!([
                    {"name": "cart", "spans": 100},
                    {"name": "payment", "spans": 50}
                ]),
            },
        );

        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["status"], "success");
        assert_eq!(json["data"]["result_type"], "services");
        let data = json["data"]["data"].as_array().unwrap();
        assert_eq!(data.len(), 2);
        assert_eq!(data[0]["name"], "cart");
        assert_eq!(data[0]["spans"], 100);
    }
}
