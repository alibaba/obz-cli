//! Result types returned by provider trait methods.
//!
//! These structs carry the converted backend response data back to the
//! CLI layer. They wrap obz model types with pagination and completeness
//! metadata.

use serde::{Deserialize, Serialize};

use crate::model::error::ObzError;
use crate::model::log::LogEntry;
use crate::model::metric::MetricSeries;
use crate::model::trace::Span;

/// Result type alias for provider operations.
pub type ProviderResult<T> = Result<T, ObzError>;

/// Metric query result type discriminator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetricResultType {
    /// Single value at a single timestamp.
    Scalar,
    /// Multiple series, each with a single data point (instant query).
    Vector,
    /// Multiple series, each with multiple data points (range query).
    Matrix,
}

/// Result from `metric query`.
#[derive(Debug)]
pub struct MetricQueryResult {
    /// The result type.
    pub result_type: MetricResultType,
    /// Query result series.
    pub series: Vec<MetricSeries>,
    /// Scalar result (for scalar queries).
    pub scalar: Option<(i64, f64)>,
    /// Total count from backend.
    pub total_count: usize,
}

/// Result from `log search`.
#[derive(Debug)]
pub struct LogSearchResult {
    /// Log entries.
    pub entries: Vec<LogEntry>,
    /// Total count from backend.
    pub total_count: usize,
    /// Whether the result set is complete.
    ///
    /// - `Some(true)`:  backend confirms all matching entries were returned.
    /// - `Some(false)`: backend confirms there are more entries beyond the
    ///   returned set (e.g., total exceeds returned count, incomplete
    ///   progress, or a pagination cursor is present).
    /// - `None`: backend does not provide a reliable completeness signal.
    ///   Omitted from JSON output so agents can infer from
    ///   `total_count == limit` if needed.
    pub is_complete: Option<bool>,
    /// Pagination cursor for next page.
    pub cursor: Option<String>,
}

/// Result from `trace search`.
#[derive(Debug)]
pub struct TraceSearchResult {
    /// Spans.
    pub spans: Vec<Span>,
    /// Total count from backend.
    pub total_count: usize,
    /// Whether the result set is complete.
    ///
    /// Semantics match [`LogSearchResult::is_complete`].
    pub is_complete: Option<bool>,
    /// Pagination cursor for next page.
    pub cursor: Option<String>,
}

/// Result from an extension command.
///
/// Extension commands can return arbitrary structured data as
/// `serde_json::Value`. For simple string-list scenarios (e.g.
/// service lists, operation lists), use [`ExtensionResult::from_strings`].
#[derive(Debug)]
pub struct ExtensionResult {
    /// Result data — any valid JSON value (array, object, etc.).
    pub data: serde_json::Value,
    /// Total count of items, if applicable.
    pub total_count: Option<usize>,
}

impl ExtensionResult {
    /// Create an `ExtensionResult` from a string list (backward-compatible).
    ///
    /// Converts `Vec<String>` into a JSON array of strings and sets
    /// `total_count` to the list length.
    pub fn from_strings(items: Vec<String>) -> Self {
        let total_count = items.len();
        Self {
            data: serde_json::Value::Array(
                items.into_iter().map(serde_json::Value::String).collect(),
            ),
            total_count: Some(total_count),
        }
    }

    /// Create an `ExtensionResult` from an arbitrary JSON value.
    ///
    /// `total_count` is left as `None`; callers can set it afterward
    /// if the value carries meaningful count semantics.
    pub fn from_value(data: serde_json::Value) -> Self {
        Self {
            data,
            total_count: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_strings_empty_list() {
        let result = ExtensionResult::from_strings(vec![]);
        assert_eq!(result.data, serde_json::json!([]));
        assert_eq!(result.total_count, Some(0));
    }

    #[test]
    fn from_strings_normal_list() {
        let result =
            ExtensionResult::from_strings(vec!["a".to_string(), "b".to_string(), "c".to_string()]);
        assert_eq!(result.data, serde_json::json!(["a", "b", "c"]));
        assert_eq!(result.total_count, Some(3));
    }

    #[test]
    fn from_value_object() {
        let result = ExtensionResult::from_value(serde_json::json!({"key": "val"}));
        assert_eq!(result.data, serde_json::json!({"key": "val"}));
        assert_eq!(result.total_count, None);
    }

    #[test]
    fn from_value_array() {
        let result = ExtensionResult::from_value(serde_json::json!([1, 2, 3]));
        assert_eq!(result.data, serde_json::json!([1, 2, 3]));
        assert_eq!(result.total_count, None);
    }
}
