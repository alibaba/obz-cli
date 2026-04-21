//! Query parameter types passed to provider trait methods.
//!
//! These structs carry the parsed CLI arguments to the provider
//! implementation. They are backend-agnostic — the same parameter
//! types are used across all providers.
//!
//! Provider-specific configuration (e.g., project names, tenant IDs)
//! is passed at provider construction time, not per-query.

use std::time::Duration;

use crate::model::error::{ErrorCode, ObzError};

/// Query parameters for `metric query` (instant or range).
#[derive(Debug, Clone)]
pub struct MetricQueryParams {
    /// Query expression (`PromQL`, `MetricsQL`, DQL).
    pub query: String,
    /// Whether the user explicitly requested a range query.
    ///
    /// Set by the shell layer based on the presence of `--from`, `--to`,
    /// or `--step` flags.
    pub is_range: bool,
    /// Start time as Unix seconds.
    pub start: i64,
    /// End time as Unix seconds.
    pub end: i64,
    /// Query step interval in seconds (range queries only).
    pub step: Option<u64>,
    /// Maximum number of series to return.
    pub limit: Option<usize>,
    /// Query timeout.
    pub timeout: Option<Duration>,
}

impl MetricQueryParams {
    /// Default target number of data points when `--step` is omitted.
    pub const DEFAULT_TARGET_POINTS: i64 = 100;

    /// Return the step in seconds, falling back to an auto-calculated value
    /// that targets approximately [`Self::DEFAULT_TARGET_POINTS`] data points.
    pub fn step_or_auto(&self) -> u64 {
        self.step.unwrap_or_else(|| {
            let duration = (self.end - self.start).max(0);
            std::cmp::max(1, (duration / Self::DEFAULT_TARGET_POINTS) as u64)
        })
    }
}

/// Parameters for `metric list`, `labels`, `series`.
#[derive(Debug, Clone)]
pub struct MetricMetadataParams {
    /// Match expression (series selector or prefix filter).
    pub match_expr: Option<String>,
    /// Additional match expressions (for `series` command).
    pub match_exprs: Vec<String>,
    /// Start time as Unix seconds.
    pub start: Option<i64>,
    /// End time as Unix seconds.
    pub end: Option<i64>,
    /// Maximum number of results to return.
    pub limit: Option<usize>,
}

/// Parameters for `metric info`.
#[derive(Debug, Clone)]
pub struct MetricInfoParams {
    /// Metric name to look up.
    pub metric_name: String,
}

/// Parameters for `metric label-values`.
#[derive(Debug, Clone)]
pub struct LabelValuesParams {
    /// Label name to get values for.
    pub label_name: String,
    /// Match expression (series selector filter).
    pub match_expr: Option<String>,
    /// Start time as Unix seconds.
    pub start: Option<i64>,
    /// End time as Unix seconds.
    pub end: Option<i64>,
    /// Maximum number of results to return.
    pub limit: Option<usize>,
}

/// Query parameters for `log search`.
#[derive(Debug, Clone)]
pub struct LogSearchParams {
    /// Query expression.
    pub query: String,
    /// Start time as Unix seconds.
    pub start: i64,
    /// End time as Unix seconds.
    pub end: i64,
    /// Maximum number of entries to return.
    pub limit: usize,
}

/// Query parameters for `trace search`.
#[derive(Debug, Clone)]
pub struct TraceSearchParams {
    /// Query expression.
    pub query: String,
    /// Start time as Unix seconds.
    pub start: i64,
    /// End time as Unix seconds.
    pub end: i64,
    /// Maximum number of spans to return.
    pub limit: usize,
}

/// Parameters for `trace get`.
#[derive(Debug, Clone)]
pub struct TraceGetParams {
    /// Trace ID (hex format).
    pub trace_id: String,
    /// Start time as Unix seconds.
    pub start: i64,
    /// End time as Unix seconds.
    pub end: i64,
}

/// Parameters for extension commands.
///
/// Extension commands receive a dynamic list of key-value arguments
/// parsed from CLI flags declared in [`CommandDescriptor`](crate::descriptor::CommandDescriptor).
/// Repeatable flags produce multiple entries with the same key.
#[derive(Debug, Clone)]
pub struct ExtensionParams {
    /// Start time as Unix seconds (if applicable).
    pub start: Option<i64>,
    /// End time as Unix seconds (if applicable).
    pub end: Option<i64>,
    /// The signal group this extension command belongs to.
    pub signal: String,
    /// Dynamic key-value arguments from extension command flags.
    pub args: Vec<(String, String)>,
}

impl ExtensionParams {
    /// Get an optional single-value argument by name.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.args
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    /// Get a required single-value argument by name.
    ///
    /// # Errors
    ///
    /// Returns [`ObzError::InvalidArgument`] when the argument is missing.
    pub fn require(&self, name: &str) -> Result<&str, ObzError> {
        self.get(name).ok_or_else(|| ObzError::InvalidArgument {
            code: ErrorCode::MissingRequired,
            message: format!("missing required argument: --{name}"),
            suggestion: None,
        })
    }

    /// Get all values for a repeatable argument (same key may appear multiple times).
    pub fn get_all(&self, name: &str) -> Vec<&str> {
        self.args
            .iter()
            .filter(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
            .collect()
    }
}
