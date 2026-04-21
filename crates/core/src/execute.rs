//! Core command execution functions.
//!
//! Each function receives an already-built provider (via trait objects in
//! [`BuiltProvider`]) and typed parameter structs, then executes the
//! operation, wraps the result in a [`Response`] envelope, and formats
//! output. No clap types appear here.
//!
//! The obz shell calls these functions after:
//!   1. Parsing CLI args with clap
//!   2. Looking up the provider metadata from the registry
//!   3. Instantiating the provider with the user-supplied endpoint/config

use std::io::{self, Write};

use crate::model::error::ObzError;
use crate::model::response::{
    ExtensionData, LabelValuesData, LogSearchData, MetricInfoData, MetricQueryData, QueryMetadata,
    Response, ScalarData, SeriesListData, StringListData, TimeRange, TraceDetailData,
    TraceSearchData, RESULT_TYPE_LABEL_LIST, RESULT_TYPE_LABEL_VALUES, RESULT_TYPE_LOG_ENTRIES,
    RESULT_TYPE_METRIC_INFO, RESULT_TYPE_METRIC_LIST, RESULT_TYPE_SERIES, RESULT_TYPE_SPANS,
    RESULT_TYPE_TRACE_DETAIL,
};
use crate::output::{format_and_print, OutputFormat};
use crate::provider::params::{
    ExtensionParams, LabelValuesParams, LogSearchParams, MetricInfoParams, MetricMetadataParams,
    MetricQueryParams, TraceGetParams, TraceSearchParams,
};
use crate::provider::results::MetricResultType;
use crate::registry::BuiltProvider;

/// Error returned by core execute functions.
///
/// Combines provider errors with IO errors so the obz shell can handle
/// both with a single `?`.
#[derive(Debug, thiserror::Error)]
pub enum ExecuteError {
    /// A provider or registry error.
    #[error(transparent)]
    Obz(#[from] ObzError),
    /// An IO error writing output (e.g. broken pipe).
    #[error("output error: {0}")]
    Io(#[from] io::Error),
}

/// Execute `obz metric query`.
///
/// # Errors
///
/// Returns an error if the provider does not support metrics, the query
/// fails, or output formatting fails.
pub async fn execute_metric_query(
    provider: &BuiltProvider,
    params: &MetricQueryParams,
    limit: usize,
    output: OutputFormat,
    fields: Option<&[String]>,
    truncate: Option<usize>,
    writer: &mut impl Write,
) -> Result<(), ExecuteError> {
    let mp = provider
        .metric
        .as_deref()
        .ok_or_else(|| unsupported(provider.name, "metric"))?;

    let result = mp.query(params).await?;
    let time_range = TimeRange {
        start: params.start,
        end: params.end,
    };

    if let Some((ts, val)) = result.scalar {
        let resp = Response::success(
            build_metric_metadata(provider, 1, Some(&params.query), Some(time_range)),
            ScalarData {
                result_type: MetricResultType::Scalar,
                scalar: (ts, val),
            },
        );
        format_and_print(&resp, output, fields, truncate, writer)?;
    } else {
        let total = result.total_count;
        let mut series = result.series;
        // Enforce client-side limit: truncate if backend returned more than requested.
        if series.len() > limit {
            series.truncate(limit);
        }
        let mut metadata =
            build_metric_metadata(provider, total, Some(&params.query), Some(time_range));
        // Set returned_series whenever we have fewer series than total_count,
        // regardless of whether the truncation happened client-side or server-side.
        if series.len() < total {
            metadata.returned_series = Some(series.len());
        }
        let resp = Response::success(
            metadata,
            MetricQueryData {
                result_type: result.result_type,
                series,
            },
        );
        format_and_print(&resp, output, fields, truncate, writer)?;
    }
    Ok(())
}

/// Execute `obz metric list`.
///
/// # Errors
///
/// Returns an error if the provider does not support metrics or the request fails.
pub async fn execute_metric_list(
    provider: &BuiltProvider,
    params: &MetricMetadataParams,
    output: OutputFormat,
    fields: Option<&[String]>,
    truncate: Option<usize>,
    writer: &mut impl Write,
) -> Result<(), ExecuteError> {
    let mp = provider
        .metric
        .as_deref()
        .ok_or_else(|| unsupported(provider.name, "metric"))?;

    let items = mp.list(params).await?;
    let time_range = match (params.start, params.end) {
        (Some(s), Some(e)) => Some(TimeRange { start: s, end: e }),
        _ => None,
    };
    let resp = Response::success(
        build_metric_metadata(
            provider,
            items.len(),
            params.match_expr.as_deref(),
            time_range,
        ),
        StringListData {
            result_type: RESULT_TYPE_METRIC_LIST.to_string(),
            items,
        },
    );
    format_and_print(&resp, output, fields, truncate, writer)?;
    Ok(())
}

/// Execute `obz metric info`.
///
/// # Errors
///
/// Returns an error if the provider does not support metrics or the request fails.
pub async fn execute_metric_info(
    provider: &BuiltProvider,
    params: &MetricInfoParams,
    output: OutputFormat,
    fields: Option<&[String]>,
    truncate: Option<usize>,
    writer: &mut impl Write,
) -> Result<(), ExecuteError> {
    let mp = provider
        .metric
        .as_deref()
        .ok_or_else(|| unsupported(provider.name, "metric"))?;

    let entries = mp.info(params).await?;
    let info = entries.into_iter().next();
    let resp = Response::success(
        build_metric_metadata(
            provider,
            usize::from(info.is_some()),
            Some(&params.metric_name),
            None,
        ),
        MetricInfoData {
            result_type: RESULT_TYPE_METRIC_INFO.to_string(),
            info,
        },
    );
    format_and_print(&resp, output, fields, truncate, writer)?;
    Ok(())
}

/// Execute `obz metric labels`.
///
/// # Errors
///
/// Returns an error if the provider does not support metrics or the request fails.
pub async fn execute_metric_labels(
    provider: &BuiltProvider,
    params: &MetricMetadataParams,
    output: OutputFormat,
    fields: Option<&[String]>,
    truncate: Option<usize>,
    writer: &mut impl Write,
) -> Result<(), ExecuteError> {
    let mp = provider
        .metric
        .as_deref()
        .ok_or_else(|| unsupported(provider.name, "metric"))?;

    let items = mp.labels(params).await?;
    let resp = Response::success(
        build_metric_metadata(provider, items.len(), params.match_expr.as_deref(), None),
        StringListData {
            result_type: RESULT_TYPE_LABEL_LIST.to_string(),
            items,
        },
    );
    format_and_print(&resp, output, fields, truncate, writer)?;
    Ok(())
}

/// Execute `obz metric label-values`.
///
/// # Errors
///
/// Returns an error if the provider does not support metrics or the request fails.
pub async fn execute_metric_label_values(
    provider: &BuiltProvider,
    params: &LabelValuesParams,
    output: OutputFormat,
    fields: Option<&[String]>,
    truncate: Option<usize>,
    writer: &mut impl Write,
) -> Result<(), ExecuteError> {
    let mp = provider
        .metric
        .as_deref()
        .ok_or_else(|| unsupported(provider.name, "metric"))?;

    let label_name = params.label_name.clone();
    let items = mp.label_values(params).await?;
    let resp = Response::success(
        build_metric_metadata(provider, items.len(), params.match_expr.as_deref(), None),
        LabelValuesData {
            result_type: RESULT_TYPE_LABEL_VALUES.to_string(),
            label: label_name,
            items,
        },
    );
    format_and_print(&resp, output, fields, truncate, writer)?;
    Ok(())
}

/// Execute `obz metric series`.
///
/// # Errors
///
/// Returns an error if the provider does not support metrics or the request fails.
pub async fn execute_metric_series(
    provider: &BuiltProvider,
    params: &MetricMetadataParams,
    output: OutputFormat,
    fields: Option<&[String]>,
    truncate: Option<usize>,
    writer: &mut impl Write,
) -> Result<(), ExecuteError> {
    let mp = provider
        .metric
        .as_deref()
        .ok_or_else(|| unsupported(provider.name, "metric"))?;

    let series = mp.series(params).await?;
    let time_range = match (params.start, params.end) {
        (Some(s), Some(e)) => Some(TimeRange { start: s, end: e }),
        _ => None,
    };
    let resp = Response::success(
        build_metric_metadata(
            provider,
            series.len(),
            params.match_expr.as_deref(),
            time_range,
        ),
        SeriesListData {
            result_type: RESULT_TYPE_SERIES.to_string(),
            series,
        },
    );
    format_and_print(&resp, output, fields, truncate, writer)?;
    Ok(())
}

/// Execute `obz log search`.
///
/// # Errors
///
/// Returns an error if the provider does not support logs or the request fails.
pub async fn execute_log_search(
    provider: &BuiltProvider,
    params: &LogSearchParams,
    output: OutputFormat,
    fields: Option<&[String]>,
    truncate: Option<usize>,
    writer: &mut impl Write,
) -> Result<(), ExecuteError> {
    let lp = provider
        .log
        .as_deref()
        .ok_or_else(|| unsupported(provider.name, "log"))?;

    let result = lp.search(params).await?;
    let resp = Response::success(
        QueryMetadata {
            provider: provider.name.to_string(),
            provider_type: None,
            query_language: provider.log_query_language.map(str::to_string),
            query: Some(params.query.clone()),
            time_range: Some(TimeRange {
                start: params.start,
                end: params.end,
            }),
            total_count: result.total_count,
            returned_series: None,
            is_complete: result.is_complete,
            cursor: result.cursor,
        },
        LogSearchData {
            result_type: RESULT_TYPE_LOG_ENTRIES.to_string(),
            entries: result.entries,
        },
    );
    format_and_print(&resp, output, fields, truncate, writer)?;
    Ok(())
}

// --- Internal helpers ---

/// Build query metadata for metric commands, including `query_language` from the provider.
fn build_metric_metadata(
    provider: &BuiltProvider,
    total_count: usize,
    query: Option<&str>,
    time_range: Option<TimeRange>,
) -> QueryMetadata {
    QueryMetadata {
        provider: provider.name.to_string(),
        provider_type: None,
        query_language: provider.metric_query_language.map(str::to_string),
        query: query.map(str::to_string),
        time_range,
        total_count,
        returned_series: None,
        is_complete: None,
        cursor: None,
    }
}

/// Infer a reasonable `total_count` from a JSON value when the provider
/// did not supply one explicitly.
///
/// Only arrays have a natural count (their length). For all other shapes
/// (objects, primitives, null) we return 0 — providers that return
/// non-array data should set `total_count` explicitly via
/// `ExtensionResult { total_count: Some(n), .. }` to avoid ambiguity.
fn infer_count(data: &serde_json::Value) -> usize {
    match data {
        serde_json::Value::Array(arr) => arr.len(),
        _ => 0,
    }
}

fn unsupported(provider_name: &str, signal: &str) -> ObzError {
    ObzError::Unsupported {
        message: format!(
            "provider '{}' does not support {} queries",
            provider_name, signal
        ),
        provider: Some(provider_name.to_string()),
        suggestion: Some(
            "This may be a configuration issue — some providers require \
             additional fields (e.g. project, logstore) to enable all signals"
                .to_string(),
        ),
    }
}

/// Execute `obz trace search`.
///
/// # Errors
///
/// Returns an error if the provider does not support traces or the request fails.
pub async fn execute_trace_search(
    provider: &BuiltProvider,
    params: &TraceSearchParams,
    output: OutputFormat,
    fields: Option<&[String]>,
    truncate: Option<usize>,
    writer: &mut impl Write,
) -> Result<(), ExecuteError> {
    let tp = provider
        .trace
        .as_deref()
        .ok_or_else(|| unsupported(provider.name, "trace"))?;

    let result = tp.search(params).await?;
    let total = result.total_count;
    let resp = Response::success(
        QueryMetadata {
            provider: provider.name.to_string(),
            provider_type: None,
            query_language: None,
            query: Some(params.query.clone()),
            time_range: Some(TimeRange {
                start: params.start,
                end: params.end,
            }),
            total_count: total,
            returned_series: None,
            is_complete: result.is_complete,
            cursor: result.cursor,
        },
        TraceSearchData {
            result_type: RESULT_TYPE_SPANS.to_string(),
            spans: result.spans,
        },
    );
    format_and_print(&resp, output, fields, truncate, writer)?;
    Ok(())
}

/// Execute `obz trace get`.
///
/// # Errors
///
/// Returns an error if the provider does not support traces or the request fails.
pub async fn execute_trace_get(
    provider: &BuiltProvider,
    params: &TraceGetParams,
    output: OutputFormat,
    fields: Option<&[String]>,
    truncate: Option<usize>,
    writer: &mut impl Write,
) -> Result<(), ExecuteError> {
    let tp = provider
        .trace
        .as_deref()
        .ok_or_else(|| unsupported(provider.name, "trace"))?;

    let detail = tp.get_trace(params).await?;
    let span_count = detail.span_count;
    let resp = Response::success(
        QueryMetadata {
            provider: provider.name.to_string(),
            provider_type: None,
            query_language: None,
            query: Some(params.trace_id.clone()),
            time_range: Some(TimeRange {
                start: params.start,
                end: params.end,
            }),
            total_count: span_count,
            returned_series: None,
            is_complete: None,
            cursor: None,
        },
        TraceDetailData {
            result_type: RESULT_TYPE_TRACE_DETAIL.to_string(),
            detail,
        },
    );
    format_and_print(&resp, output, fields, truncate, writer)?;
    Ok(())
}

/// Execute a provider-specific extension command.
///
/// # Errors
///
/// Returns an error if the provider does not support extension commands,
/// the command fails, or output formatting fails.
pub async fn execute_extension_command(
    provider: &BuiltProvider,
    command: &str,
    params: &ExtensionParams,
    output: OutputFormat,
    fields: Option<&[String]>,
    truncate: Option<usize>,
    writer: &mut impl Write,
) -> Result<(), ExecuteError> {
    let ep = provider
        .extension
        .as_deref()
        .ok_or_else(|| ObzError::Unsupported {
            message: format!(
                "provider '{}' does not support extension commands",
                provider.name
            ),
            provider: Some(provider.name.to_string()),
            suggestion: None,
        })?;

    let result = ep.execute(command, params).await?;
    let total = result
        .total_count
        .unwrap_or_else(|| infer_count(&result.data));
    let resp = Response::success(
        QueryMetadata {
            provider: provider.name.to_string(),
            provider_type: None,
            query_language: None,
            query: None,
            time_range: None,
            total_count: total,
            returned_series: None,
            is_complete: None,
            cursor: None,
        },
        ExtensionData {
            result_type: command.to_string(),
            data: result.data,
        },
    );
    format_and_print(&resp, output, fields, truncate, writer)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::log::LogEntry;
    use crate::model::metric::{DataPoint, MetricSeries};
    use crate::model::trace::{Span, SpanStatus, TraceDetail};
    use crate::provider::results::{
        ExtensionResult, LogSearchResult, MetricQueryResult, ProviderResult, TraceSearchResult,
    };
    use crate::provider::traits::{ExtensionProvider, LogProvider, MetricProvider, TraceProvider};
    use async_trait::async_trait;
    use std::collections::BTreeMap;
    // -----------------------------------------------------------------------
    // Mock providers
    // -----------------------------------------------------------------------

    /// Mock metric provider that returns configurable results.
    struct MockMetricProvider {
        result: MetricQueryResult,
    }

    #[async_trait]
    impl MetricProvider for MockMetricProvider {
        async fn query(&self, _params: &MetricQueryParams) -> ProviderResult<MetricQueryResult> {
            // Reconstruct because MetricQueryResult is not Clone
            Ok(MetricQueryResult {
                result_type: self.result.result_type,
                series: self.result.series.clone(),
                scalar: self.result.scalar,
                total_count: self.result.total_count,
            })
        }
        async fn list(&self, _params: &MetricMetadataParams) -> ProviderResult<Vec<String>> {
            Ok(vec!["cpu_usage".to_string(), "mem_usage".to_string()])
        }
        async fn info(
            &self,
            _params: &crate::provider::params::MetricInfoParams,
        ) -> ProviderResult<Vec<crate::model::metric::MetricInfoDetail>> {
            Ok(vec![])
        }
        async fn labels(&self, _params: &MetricMetadataParams) -> ProviderResult<Vec<String>> {
            Ok(vec!["__name__".to_string(), "instance".to_string()])
        }
        async fn label_values(&self, _params: &LabelValuesParams) -> ProviderResult<Vec<String>> {
            Ok(vec!["value1".to_string(), "value2".to_string()])
        }
        async fn series(
            &self,
            _params: &MetricMetadataParams,
        ) -> ProviderResult<Vec<BTreeMap<String, String>>> {
            Ok(vec![BTreeMap::from([
                ("__name__".to_string(), "up".to_string()),
                ("instance".to_string(), "localhost:9090".to_string()),
            ])])
        }
    }

    /// Mock log provider.
    struct MockLogProvider {
        is_complete: Option<bool>,
        cursor: Option<String>,
    }

    #[async_trait]
    impl LogProvider for MockLogProvider {
        async fn search(&self, _params: &LogSearchParams) -> ProviderResult<LogSearchResult> {
            Ok(LogSearchResult {
                entries: vec![LogEntry {
                    timestamp: 1_700_000_000,
                    message: "test log message".to_string(),
                    severity: None,
                    source: None,
                    service: None,
                    id: None,
                    attributes: None,
                    resource: None,
                    trace_id: None,
                    span_id: None,
                    extensions: None,
                }],
                total_count: 1,
                is_complete: self.is_complete,
                cursor: self.cursor.clone(),
            })
        }
    }

    /// Mock trace provider.
    struct MockTraceProvider;

    fn make_span(trace_id: &str, span_id: &str, name: &str) -> Span {
        Span {
            trace_id: trace_id.to_string(),
            span_id: span_id.to_string(),
            parent_span_id: None,
            name: name.to_string(),
            service: "test-service".to_string(),
            kind: None,
            status: SpanStatus::Ok,
            start_time: 1_700_000_000_i64,
            duration_us: 1000,
            attributes: None,
            events: None,
            resource: None,
            extensions: None,
        }
    }

    #[async_trait]
    impl TraceProvider for MockTraceProvider {
        async fn search(&self, _params: &TraceSearchParams) -> ProviderResult<TraceSearchResult> {
            Ok(TraceSearchResult {
                spans: vec![make_span("abc123", "span1", "GET /api")],
                total_count: 1,
                is_complete: None,
                cursor: None,
            })
        }
        async fn get_trace(&self, params: &TraceGetParams) -> ProviderResult<TraceDetail> {
            let spans = vec![make_span(&params.trace_id, "span1", "GET /api")];
            Ok(TraceDetail::from_spans(params.trace_id.clone(), spans))
        }
    }

    /// Mock extension provider returning a string list.
    struct MockExtensionProvider;

    #[async_trait]
    impl ExtensionProvider for MockExtensionProvider {
        async fn execute(
            &self,
            _command: &str,
            _params: &ExtensionParams,
        ) -> ProviderResult<ExtensionResult> {
            Ok(ExtensionResult::from_strings(vec![
                "service-a".to_string(),
                "service-b".to_string(),
            ]))
        }
    }

    /// Mock extension provider returning structured data via `from_value`.
    struct MockStructuredExtensionProvider;

    #[async_trait]
    impl ExtensionProvider for MockStructuredExtensionProvider {
        async fn execute(
            &self,
            _command: &str,
            _params: &ExtensionParams,
        ) -> ProviderResult<ExtensionResult> {
            Ok(ExtensionResult::from_value(serde_json::json!([
                {"name": "cart", "spans": 100},
                {"name": "payment", "spans": 50}
            ])))
        }
    }

    // -----------------------------------------------------------------------
    // Helper: build a BuiltProvider with optional trait objects
    // -----------------------------------------------------------------------

    fn make_series(name: &str, points: Vec<DataPoint>) -> MetricSeries {
        MetricSeries {
            name: name.to_string(),
            labels: BTreeMap::new(),
            points,
            stats: None,
            extensions: None,
        }
    }

    fn make_provider_metric_only(mp: impl MetricProvider + 'static) -> BuiltProvider {
        BuiltProvider {
            name: "test",
            metric_query_language: Some("TestQL"),
            log_query_language: None,
            metric: Some(Box::new(mp)),
            log: None,
            trace: None,
            extension: None,
        }
    }

    fn make_provider_log_only(lp: impl LogProvider + 'static) -> BuiltProvider {
        BuiltProvider {
            name: "test",
            metric_query_language: None,
            log_query_language: Some("TestLogQL"),
            metric: None,
            log: Some(Box::new(lp)),
            trace: None,
            extension: None,
        }
    }

    fn make_provider_trace_only(tp: impl TraceProvider + 'static) -> BuiltProvider {
        BuiltProvider {
            name: "test",
            metric_query_language: None,
            log_query_language: None,
            metric: None,
            log: None,
            trace: Some(Box::new(tp)),
            extension: None,
        }
    }

    fn make_provider_extension_only(ep: impl ExtensionProvider + 'static) -> BuiltProvider {
        BuiltProvider {
            name: "test",
            metric_query_language: None,
            log_query_language: None,
            metric: None,
            log: None,
            trace: None,
            extension: Some(Box::new(ep)),
        }
    }

    fn empty_provider() -> BuiltProvider {
        BuiltProvider {
            name: "empty",
            metric_query_language: None,
            log_query_language: None,
            metric: None,
            log: None,
            trace: None,
            extension: None,
        }
    }

    fn default_metric_params() -> MetricQueryParams {
        MetricQueryParams {
            query: "up".to_string(),
            is_range: false,
            start: 1_700_000_000,
            end: 1_700_003_600,
            step: None,
            limit: None,
            timeout: None,
        }
    }

    // -----------------------------------------------------------------------
    // Tests: internal helpers (build_metric_metadata, truncation logic)
    // -----------------------------------------------------------------------

    #[test]
    fn build_metric_metadata_includes_query_language() {
        let provider = make_provider_metric_only(MockMetricProvider {
            result: MetricQueryResult {
                result_type: MetricResultType::Vector,
                series: vec![],
                scalar: None,
                total_count: 0,
            },
        });
        let md = build_metric_metadata(&provider, 5, Some("up"), None);
        assert_eq!(md.provider, "test");
        assert_eq!(md.query_language.as_deref(), Some("TestQL"));
        assert_eq!(md.query.as_deref(), Some("up"));
        assert_eq!(md.total_count, 5);
        assert!(md.returned_series.is_none());
        assert!(md.time_range.is_none());
    }

    #[test]
    fn truncation_logic_client_side() {
        // Simulate the truncation logic from execute_metric_query.
        let mut series: Vec<MetricSeries> = (0..10)
            .map(|i| {
                make_series(
                    &format!("s{i}"),
                    vec![DataPoint {
                        timestamp: 100,
                        value: 1.0,
                    }],
                )
            })
            .collect();
        let total = 10;
        let limit = 5;

        if series.len() > limit {
            series.truncate(limit);
        }
        assert_eq!(series.len(), 5, "should truncate to limit");

        let returned_series = if series.len() < total {
            Some(series.len())
        } else {
            None
        };
        assert_eq!(returned_series, Some(5), "should report truncated count");
    }

    #[test]
    fn truncation_logic_no_truncation_needed() {
        let mut series: Vec<MetricSeries> = (0..3)
            .map(|i| {
                make_series(
                    &format!("s{i}"),
                    vec![DataPoint {
                        timestamp: 100,
                        value: 1.0,
                    }],
                )
            })
            .collect();
        let total = 3;
        let limit = 100;

        if series.len() > limit {
            series.truncate(limit);
        }
        assert_eq!(series.len(), 3, "should not truncate");

        let returned_series = if series.len() < total {
            Some(series.len())
        } else {
            None
        };
        assert!(
            returned_series.is_none(),
            "should be None when no truncation"
        );
    }

    #[test]
    fn truncation_logic_server_side() {
        // Server already truncated: returned 5 but total_count is 20.
        let mut series: Vec<MetricSeries> = (0..5)
            .map(|i| {
                make_series(
                    &format!("s{i}"),
                    vec![DataPoint {
                        timestamp: 100,
                        value: 1.0,
                    }],
                )
            })
            .collect();
        let total = 20;
        let limit = 100;

        if series.len() > limit {
            series.truncate(limit);
        }
        assert_eq!(series.len(), 5);

        let returned_series = if series.len() < total {
            Some(series.len())
        } else {
            None
        };
        assert_eq!(
            returned_series,
            Some(5),
            "should report server-truncated count"
        );
    }

    // -----------------------------------------------------------------------
    // Tests: execute_metric_query
    // -----------------------------------------------------------------------

    /// Run an execute function capturing JSON output, parse and return it.
    fn parse_output(buf: &[u8]) -> serde_json::Value {
        serde_json::from_slice(buf).expect("output should be valid JSON")
    }

    #[tokio::test]
    async fn metric_query_series_path() {
        let series = vec![
            make_series(
                "s1",
                vec![DataPoint {
                    timestamp: 100,
                    value: 1.0,
                }],
            ),
            make_series(
                "s2",
                vec![DataPoint {
                    timestamp: 100,
                    value: 2.0,
                }],
            ),
        ];
        let provider = make_provider_metric_only(MockMetricProvider {
            result: MetricQueryResult {
                result_type: MetricResultType::Vector,
                series,
                scalar: None,
                total_count: 2,
            },
        });

        let mut buf = Vec::new();
        execute_metric_query(
            &provider,
            &default_metric_params(),
            100,
            OutputFormat::Json,
            None,
            None,
            &mut buf,
        )
        .await
        .unwrap();

        let json = parse_output(&buf);
        assert_eq!(json["status"], "success");
        assert_eq!(json["metadata"]["provider"], "test");
        assert_eq!(json["metadata"]["total_count"], 2);
        assert_eq!(json["data"]["series"].as_array().unwrap().len(), 2);
        assert_eq!(json["data"]["series"][0]["name"], "s1");
    }

    #[tokio::test]
    async fn metric_query_scalar_path() {
        let provider = make_provider_metric_only(MockMetricProvider {
            result: MetricQueryResult {
                result_type: MetricResultType::Scalar,
                series: vec![],
                scalar: Some((1_700_000_000, 42.0)),
                total_count: 1,
            },
        });

        let mut buf = Vec::new();
        execute_metric_query(
            &provider,
            &default_metric_params(),
            100,
            OutputFormat::Json,
            None,
            None,
            &mut buf,
        )
        .await
        .unwrap();

        let json = parse_output(&buf);
        assert_eq!(json["status"], "success");
        assert_eq!(json["data"]["result_type"], "scalar");
        assert_eq!(json["data"]["scalar"][1], 42.0);
    }

    #[tokio::test]
    async fn metric_query_client_side_truncation() {
        let series: Vec<MetricSeries> = (0..10)
            .map(|i| {
                make_series(
                    &format!("s{i}"),
                    vec![DataPoint {
                        timestamp: 100,
                        value: 1.0,
                    }],
                )
            })
            .collect();
        let provider = make_provider_metric_only(MockMetricProvider {
            result: MetricQueryResult {
                result_type: MetricResultType::Vector,
                series,
                scalar: None,
                total_count: 10,
            },
        });

        let mut buf = Vec::new();
        execute_metric_query(
            &provider,
            &default_metric_params(),
            5,
            OutputFormat::Json,
            None,
            None,
            &mut buf,
        )
        .await
        .unwrap();

        let json = parse_output(&buf);
        assert_eq!(json["metadata"]["total_count"], 10);
        assert_eq!(json["metadata"]["returned_series"], 5);
        assert_eq!(json["data"]["series"].as_array().unwrap().len(), 5);
    }

    #[tokio::test]
    async fn metric_query_no_truncation() {
        let series: Vec<MetricSeries> = (0..3)
            .map(|i| {
                make_series(
                    &format!("s{i}"),
                    vec![DataPoint {
                        timestamp: 100,
                        value: 1.0,
                    }],
                )
            })
            .collect();
        let provider = make_provider_metric_only(MockMetricProvider {
            result: MetricQueryResult {
                result_type: MetricResultType::Vector,
                series,
                scalar: None,
                total_count: 3,
            },
        });

        let result = execute_metric_query(
            &provider,
            &default_metric_params(),
            100,
            OutputFormat::Json,
            None,
            None,
            &mut io::sink(),
        )
        .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn metric_query_unsupported_provider() {
        let provider = empty_provider();
        let result = execute_metric_query(
            &provider,
            &default_metric_params(),
            100,
            OutputFormat::Json,
            None,
            None,
            &mut io::sink(),
        )
        .await;

        match result {
            Err(ExecuteError::Obz(ObzError::Unsupported {
                message,
                suggestion,
                ..
            })) => {
                assert!(message.contains("metric"));
                assert!(message.contains("empty"));
                assert!(
                    suggestion.is_some(),
                    "unsupported should include suggestion"
                );
                let hint = suggestion.unwrap();
                assert!(
                    !hint.contains("obz "),
                    "core suggestion must not reference CLI commands, got: {hint}"
                );
            }
            other => panic!("expected Unsupported error, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // Tests: execute_metric_list / info / labels / label_values / series
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn metric_list_success() {
        let provider = make_provider_metric_only(MockMetricProvider {
            result: MetricQueryResult {
                result_type: MetricResultType::Vector,
                series: vec![],
                scalar: None,
                total_count: 0,
            },
        });

        let result = execute_metric_list(
            &provider,
            &MetricMetadataParams {
                match_expr: Some("cpu".to_string()),
                match_exprs: vec![],
                start: Some(100),
                end: Some(200),
                limit: Some(100),
            },
            OutputFormat::Json,
            None,
            None,
            &mut io::sink(),
        )
        .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn metric_list_partial_time_range() {
        let provider = make_provider_metric_only(MockMetricProvider {
            result: MetricQueryResult {
                result_type: MetricResultType::Vector,
                series: vec![],
                scalar: None,
                total_count: 0,
            },
        });

        let result = execute_metric_list(
            &provider,
            &MetricMetadataParams {
                match_expr: None,
                match_exprs: vec![],
                start: Some(1_700_000_000),
                end: None,
                limit: Some(100),
            },
            OutputFormat::Json,
            None,
            None,
            &mut io::sink(),
        )
        .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn metric_info_success() {
        let provider = make_provider_metric_only(MockMetricProvider {
            result: MetricQueryResult {
                result_type: MetricResultType::Vector,
                series: vec![],
                scalar: None,
                total_count: 0,
            },
        });

        let result = execute_metric_info(
            &provider,
            &MetricInfoParams {
                metric_name: "cpu_usage".to_string(),
            },
            OutputFormat::Json,
            None,
            None,
            &mut io::sink(),
        )
        .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn metric_labels_success() {
        let provider = make_provider_metric_only(MockMetricProvider {
            result: MetricQueryResult {
                result_type: MetricResultType::Vector,
                series: vec![],
                scalar: None,
                total_count: 0,
            },
        });

        let result = execute_metric_labels(
            &provider,
            &MetricMetadataParams {
                match_expr: None,
                match_exprs: vec![],
                start: None,
                end: None,
                limit: None,
            },
            OutputFormat::Json,
            None,
            None,
            &mut io::sink(),
        )
        .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn metric_label_values_success() {
        let provider = make_provider_metric_only(MockMetricProvider {
            result: MetricQueryResult {
                result_type: MetricResultType::Vector,
                series: vec![],
                scalar: None,
                total_count: 0,
            },
        });

        let result = execute_metric_label_values(
            &provider,
            &LabelValuesParams {
                label_name: "instance".to_string(),
                match_expr: None,
                start: None,
                end: None,
                limit: Some(100),
            },
            OutputFormat::Json,
            None,
            None,
            &mut io::sink(),
        )
        .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn metric_series_success() {
        let provider = make_provider_metric_only(MockMetricProvider {
            result: MetricQueryResult {
                result_type: MetricResultType::Vector,
                series: vec![],
                scalar: None,
                total_count: 0,
            },
        });

        let result = execute_metric_series(
            &provider,
            &MetricMetadataParams {
                match_expr: None,
                match_exprs: vec!["{__name__=\"up\"}".to_string()],
                start: Some(100),
                end: Some(200),
                limit: Some(1000),
            },
            OutputFormat::Json,
            None,
            None,
            &mut io::sink(),
        )
        .await;
        assert!(result.is_ok());
    }

    // -----------------------------------------------------------------------
    // Tests: execute_log_search
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn log_search_metadata_propagation() {
        let provider = make_provider_log_only(MockLogProvider {
            is_complete: Some(true),
            cursor: Some("next-page-token".to_string()),
        });

        let mut buf = Vec::new();
        execute_log_search(
            &provider,
            &LogSearchParams {
                query: "error".to_string(),
                start: 1_700_000_000,
                end: 1_700_003_600,
                limit: 100,
            },
            OutputFormat::Json,
            None,
            None,
            &mut buf,
        )
        .await
        .unwrap();

        let json = parse_output(&buf);
        assert_eq!(json["status"], "success");
        assert_eq!(json["metadata"]["cursor"], "next-page-token");
        assert_eq!(json["metadata"]["is_complete"], true);
        assert_eq!(json["metadata"]["query_language"], "TestLogQL");
        assert_eq!(json["data"]["entries"].as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn log_search_unsupported_provider() {
        let provider = empty_provider();
        let result = execute_log_search(
            &provider,
            &LogSearchParams {
                query: "*".to_string(),
                start: 0,
                end: 0,
                limit: 10,
            },
            OutputFormat::Json,
            None,
            None,
            &mut io::sink(),
        )
        .await;

        match result {
            Err(ExecuteError::Obz(ObzError::Unsupported {
                message,
                suggestion,
                ..
            })) => {
                assert!(message.contains("log"));
                assert!(
                    suggestion.is_some(),
                    "log unsupported should include suggestion"
                );
                let hint = suggestion.unwrap();
                assert!(
                    !hint.contains("obz "),
                    "core suggestion must not reference CLI commands, got: {hint}"
                );
            }
            other => panic!("expected Unsupported error, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // Tests: execute_trace_search
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn trace_search_success() {
        let provider = make_provider_trace_only(MockTraceProvider);
        let result = execute_trace_search(
            &provider,
            &TraceSearchParams {
                query: "cart".to_string(),
                start: 1_700_000_000,
                end: 1_700_003_600,
                limit: 20,
            },
            OutputFormat::Json,
            None,
            None,
            &mut io::sink(),
        )
        .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn trace_search_unsupported_provider() {
        let provider = empty_provider();
        let result = execute_trace_search(
            &provider,
            &TraceSearchParams {
                query: "cart".to_string(),
                start: 0,
                end: 0,
                limit: 10,
            },
            OutputFormat::Json,
            None,
            None,
            &mut io::sink(),
        )
        .await;

        match result {
            Err(ExecuteError::Obz(ObzError::Unsupported {
                message,
                suggestion,
                ..
            })) => {
                assert!(message.contains("trace"));
                assert!(
                    suggestion.is_some(),
                    "trace unsupported should include suggestion"
                );
                let hint = suggestion.unwrap();
                assert!(
                    !hint.contains("obz "),
                    "core suggestion must not reference CLI commands, got: {hint}"
                );
            }
            other => panic!("expected Unsupported error, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // Tests: execute_trace_get
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn trace_get_pass_through() {
        let provider = make_provider_trace_only(MockTraceProvider);
        let result = execute_trace_get(
            &provider,
            &TraceGetParams {
                trace_id: "abc123".to_string(),
                start: 1_700_000_000,
                end: 1_700_003_600,
            },
            OutputFormat::Json,
            None,
            None,
            &mut io::sink(),
        )
        .await;
        assert!(result.is_ok());
    }

    // -----------------------------------------------------------------------
    // Tests: execute_extension_command
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn extension_command_unsupported() {
        let provider = empty_provider();
        let result = execute_extension_command(
            &provider,
            "services",
            &ExtensionParams {
                start: None,
                end: None,
                signal: String::new(),
                args: Vec::new(),
            },
            OutputFormat::Json,
            None,
            None,
            &mut io::sink(),
        )
        .await;

        match result {
            Err(ExecuteError::Obz(ObzError::Unsupported { message, .. })) => {
                assert!(message.contains("extension"));
            }
            other => panic!("expected Unsupported error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn extension_command_success() {
        let provider = make_provider_extension_only(MockExtensionProvider);
        let result = execute_extension_command(
            &provider,
            "services",
            &ExtensionParams {
                start: None,
                end: None,
                signal: String::new(),
                args: Vec::new(),
            },
            OutputFormat::Json,
            None,
            None,
            &mut io::sink(),
        )
        .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn extension_command_structured_data() {
        let provider = make_provider_extension_only(MockStructuredExtensionProvider);
        let mut buf = Vec::new();
        execute_extension_command(
            &provider,
            "services",
            &ExtensionParams {
                start: None,
                end: None,
                signal: String::new(),
                args: Vec::new(),
            },
            OutputFormat::Json,
            None,
            None,
            &mut buf,
        )
        .await
        .unwrap();

        let json = parse_output(&buf);
        assert_eq!(json["status"], "success");
        // total_count inferred from array length (from_value sets None)
        assert_eq!(json["metadata"]["total_count"], 2);
        assert_eq!(json["data"]["result_type"], "services");
        let data = json["data"]["data"].as_array().unwrap();
        assert_eq!(data.len(), 2);
        assert_eq!(data[0]["name"], "cart");
        assert_eq!(data[1]["spans"], 50);
    }

    #[test]
    fn infer_count_array() {
        assert_eq!(infer_count(&serde_json::json!([1, 2, 3])), 3);
    }

    #[test]
    fn infer_count_object() {
        // Objects have no natural count; providers should set total_count explicitly.
        assert_eq!(infer_count(&serde_json::json!({"key": "val"})), 0);
    }

    #[test]
    fn infer_count_empty_array() {
        assert_eq!(infer_count(&serde_json::json!([])), 0);
    }

    #[test]
    fn infer_count_primitive() {
        assert_eq!(infer_count(&serde_json::json!("hello")), 0);
        assert_eq!(infer_count(&serde_json::json!(null)), 0);
    }
}
