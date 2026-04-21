//! Unified data models for metrics, logs, and traces.
//!
//! All backend responses are normalized to these models before output.
//! The models are designed for AI Agent consumption (compact, deterministic,
//! pre-computed statistics).

pub mod error;
pub mod log;
pub mod metric;
pub mod response;
pub mod trace;

pub use error::{ErrorCategory, ErrorCode, ErrorDetail, ObzError};
pub use log::{LogEntry, Severity};
pub use metric::{DataPoint, MetricInfoDetail, MetricSeries, MetricType, SeriesStats};
pub use response::{
    LabelValuesData, LogSearchData, MetricInfoData, MetricQueryData, QueryMetadata, Response,
    ResponseStatus, ScalarData, SeriesListData, StringListData, TimeRange, RESULT_TYPE_LABEL_LIST,
    RESULT_TYPE_LABEL_VALUES, RESULT_TYPE_LOG_ENTRIES, RESULT_TYPE_METRIC_INFO,
    RESULT_TYPE_METRIC_LIST, RESULT_TYPE_SERIES, RESULT_TYPE_SPANS, RESULT_TYPE_TRACE_DETAIL,
};
pub use trace::{Span, SpanEvent, SpanKind, SpanStatus, TraceDetail};
