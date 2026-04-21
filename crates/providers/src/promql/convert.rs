//! Prometheus-compatible API → obz model conversion.
//!
//! Converts Prometheus HTTP API responses into obz unified data models.
//! Shared across providers with PromQL-compatible endpoints
//! (VictoriaMetrics, SLS MetricStore, etc.).
//!
//! Key transformations:
//! - Extract `__name__` from `metric` map → `MetricSeries.name`
//! - Parse string values to f64 (handling NaN, +Inf, -Inf)
//! - Compute `SeriesStats` from data points
//! - Truncate float timestamps to integer seconds

use std::collections::BTreeMap;

use obz_core::model::error::{ErrorCode, ObzError};
use obz_core::model::metric::{DataPoint, MetricInfoDetail, MetricSeries, MetricType, SeriesStats};
use obz_core::provider::results::MetricResultType;
use obz_core::provider::{MetricQueryResult, ProviderResult};

use super::response::{
    PromqlDataPoint, PromqlMetadataEntry, PromqlQueryData, PromqlResponse, PromqlSample,
};

/// Convert a `PromQL` query response to obz `MetricQueryResult`.
///
/// Handles both instant (vector) and range (matrix) results,
/// as well as scalar results.
///
/// # Errors
///
/// Returns `ObzError::Provider` if the response has `status: "error"`.
pub(crate) fn convert_query_response(
    resp: PromqlResponse<PromqlQueryData>,
) -> ProviderResult<MetricQueryResult> {
    check_error(
        &resp.status,
        resp.error.as_deref(),
        resp.error_type.as_deref(),
    )?;

    let data = resp.data.ok_or_else(|| ObzError::Provider {
        code: ErrorCode::BackendError,
        message: "PromQL response missing data field".to_string(),
        raw_error: None,
        recoverable: false,
        suggestion: None,
        doc_url: None,
    })?;

    let result_type = match data.result_type.as_str() {
        "scalar" => MetricResultType::Scalar,
        "vector" => MetricResultType::Vector,
        "matrix" => MetricResultType::Matrix,
        _other => MetricResultType::Vector, // Treat unknown as vector
    };

    // Handle scalar result type.
    if result_type == MetricResultType::Scalar {
        let scalar = data.result.first().and_then(|s| {
            s.value
                .as_ref()
                .map(|dp| (dp.timestamp, parse_value(&dp.value)))
        });
        return Ok(MetricQueryResult {
            result_type,
            series: Vec::new(),
            scalar,
            total_count: if scalar.is_some() { 1 } else { 0 },
        });
    }

    let mut series = Vec::with_capacity(data.result.len());

    for sample in &data.result {
        if let Some(converted) = convert_sample(sample, result_type) {
            series.push(converted);
        }
    }

    let total_count = series.len();

    Ok(MetricQueryResult {
        result_type,
        series,
        scalar: None,
        total_count,
    })
}

/// Convert a `PromQL` labels/label-values response to a `Vec<String>`.
///
/// # Errors
///
/// Returns `ObzError::Provider` if the response has `status: "error"`.
pub(crate) fn convert_string_list_response(
    resp: PromqlResponse<Vec<String>>,
) -> ProviderResult<Vec<String>> {
    check_error(
        &resp.status,
        resp.error.as_deref(),
        resp.error_type.as_deref(),
    )?;
    Ok(resp.data.unwrap_or_default())
}

/// Convert a `PromQL` series response to a `Vec<BTreeMap<String, String>>`.
///
/// # Errors
///
/// Returns `ObzError::Provider` if the response has `status: "error"`.
pub(crate) fn convert_series_response(
    resp: PromqlResponse<Vec<BTreeMap<String, String>>>,
) -> ProviderResult<Vec<BTreeMap<String, String>>> {
    check_error(
        &resp.status,
        resp.error.as_deref(),
        resp.error_type.as_deref(),
    )?;
    Ok(resp.data.unwrap_or_default())
}

/// Convert a `PromQL` metadata response to `Vec<MetricInfoDetail>`.
///
/// The `/api/v1/metadata` endpoint returns a map of metric name to metadata entries.
/// Each metric can have multiple metadata entries (from different targets).
///
/// # Errors
///
/// Returns `ObzError::Provider` if the response has `status: "error"`.
pub(crate) fn convert_metadata_response(
    resp: PromqlResponse<BTreeMap<String, Vec<PromqlMetadataEntry>>>,
    filter_name: Option<&str>,
) -> ProviderResult<Vec<MetricInfoDetail>> {
    check_error(
        &resp.status,
        resp.error.as_deref(),
        resp.error_type.as_deref(),
    )?;

    let data = resp.data.unwrap_or_default();
    let mut result = Vec::new();

    for (name, entries) in &data {
        if let Some(filter) = filter_name {
            if name != filter {
                continue;
            }
        }

        // Take the first entry for each metric (most common case).
        let entry = entries.first();
        result.push(MetricInfoDetail {
            name: name.clone(),
            metric_type: entry
                .and_then(|e| e.metric_type.as_deref())
                .map(parse_metric_type),
            description: entry.and_then(|e| e.help.clone()),
            unit: entry.and_then(|e| e.unit.clone()).filter(|u| !u.is_empty()),
        });
    }

    Ok(result)
}

/// Convert a single `PromQL` sample to an obz `MetricSeries`.
fn convert_sample(sample: &PromqlSample, result_type: MetricResultType) -> Option<MetricSeries> {
    let (name, labels) = extract_name_and_labels(&sample.metric);

    let points = match result_type {
        MetricResultType::Matrix => sample
            .values
            .as_ref()?
            .iter()
            .map(convert_data_point)
            .collect(),
        MetricResultType::Vector | MetricResultType::Scalar => {
            let val = sample.value.as_ref()?;
            vec![convert_data_point(val)]
        }
    };

    let stats = Some(SeriesStats::from_points(&points));

    Some(MetricSeries {
        name,
        labels,
        points,
        stats,
        extensions: None,
    })
}

/// Extract `__name__` from the metric map and return (name, remaining labels).
fn extract_name_and_labels(
    metric: &BTreeMap<String, String>,
) -> (String, BTreeMap<String, String>) {
    let name = metric.get("__name__").cloned().unwrap_or_default();
    let labels: BTreeMap<String, String> = metric
        .iter()
        .filter(|(k, _)| k.as_str() != "__name__")
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    (name, labels)
}

/// Convert a `PromQL` data point `[timestamp, "value_string"]` to an obz `DataPoint`.
fn convert_data_point(dp: &PromqlDataPoint) -> DataPoint {
    let value = parse_value(&dp.value);
    DataPoint {
        timestamp: dp.timestamp,
        value,
    }
}

/// Parse a `PromQL` string value to f64.
///
/// Prometheus-compatible APIs return values as JSON strings. Special values:
/// - `"NaN"` → `f64::NAN`
/// - `"+Inf"` → `f64::INFINITY`
/// - `"-Inf"` → `f64::NEG_INFINITY`
/// - `"Inf"` → `f64::INFINITY`
fn parse_value(s: &str) -> f64 {
    match s {
        "NaN" => f64::NAN,
        "+Inf" | "Inf" => f64::INFINITY,
        "-Inf" => f64::NEG_INFINITY,
        _ => s.parse::<f64>().unwrap_or(f64::NAN),
    }
}

/// Parse a Prometheus metric type string to obz `MetricType`.
fn parse_metric_type(s: &str) -> MetricType {
    match s {
        "gauge" => MetricType::Gauge,
        "counter" => MetricType::Counter,
        // Prometheus summary and histogram are distinct types, but obz
        // currently maps both to Histogram since the data model does not
        // yet differentiate them.
        "histogram" | "summary" => MetricType::Histogram,
        _ => MetricType::Unknown,
    }
}

/// Check if a `PromQL` response is an error and convert to `ObzError`.
fn check_error(status: &str, error: Option<&str>, error_type: Option<&str>) -> ProviderResult<()> {
    if status == "error" {
        let message = error.unwrap_or("unknown PromQL error").to_owned();

        let code = match error_type {
            // VictoriaMetrics returns errorType "422" for PromQL errors.
            // SLS returns errorType "bad_data" (Prometheus-standard) for the same.
            Some("422") | Some("bad_data") => {
                if message.contains("query")
                    || message.contains("syntax")
                    || message.contains("parse")
                    || message.contains("parameter")
                {
                    ErrorCode::QuerySyntax
                } else if message.contains("time") || message.contains("step") {
                    ErrorCode::InvalidTimeRange
                } else {
                    ErrorCode::BackendError
                }
            }
            _ => ErrorCode::BackendError,
        };

        let recoverable = matches!(error_type, Some("503") | Some("429"));

        return Err(ObzError::Provider {
            code,
            message,
            // raw_error carries supplementary context (error_type / HTTP status code)
            // that is distinct from the human-readable message above.
            raw_error: error_type.map(|t| format!("error_type={t}")),
            recoverable,
            suggestion: None,
            doc_url: None,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_value_normal() {
        assert!((parse_value("5") - 5.0).abs() < f64::EPSILON);
        assert!((parse_value("0.03333333333333333") - 0.03333333333333333).abs() < f64::EPSILON);
    }

    #[test]
    fn test_parse_value_special() {
        assert!(parse_value("NaN").is_nan());
        assert!(parse_value("+Inf").is_infinite() && parse_value("+Inf").is_sign_positive());
        assert!(parse_value("-Inf").is_infinite() && parse_value("-Inf").is_sign_negative());
        assert!(parse_value("Inf").is_infinite());
    }

    #[test]
    fn test_extract_name_and_labels() {
        let mut metric = BTreeMap::new();
        metric.insert("__name__".to_string(), "cpu_usage".to_string());
        metric.insert("host".to_string(), "web01".to_string());
        metric.insert("env".to_string(), "prod".to_string());

        let (name, labels) = extract_name_and_labels(&metric);
        assert_eq!(name, "cpu_usage");
        assert_eq!(labels.len(), 2);
        assert_eq!(labels["env"], "prod");
        assert_eq!(labels["host"], "web01");
        assert!(!labels.contains_key("__name__"));
    }

    #[test]
    fn test_extract_name_empty_metric() {
        let metric = BTreeMap::new();
        let (name, labels) = extract_name_and_labels(&metric);
        assert_eq!(name, "");
        assert!(labels.is_empty());
    }

    #[test]
    fn test_convert_vector_response() {
        let json = r#"{
            "status": "success",
            "data": {
                "resultType": "vector",
                "result": [
                    {
                        "metric": {"__name__": "up", "job": "api"},
                        "value": [1774605500, "1"]
                    },
                    {
                        "metric": {"__name__": "up", "job": "web"},
                        "value": [1774605500, "0"]
                    }
                ]
            }
        }"#;
        let resp: PromqlResponse<PromqlQueryData> = serde_json::from_str(json).unwrap();
        let result = convert_query_response(resp).unwrap();

        assert_eq!(result.result_type, MetricResultType::Vector);
        assert_eq!(result.total_count, 2);
        assert_eq!(result.series[0].name, "up");
        assert_eq!(result.series[0].labels["job"], "api");
        assert_eq!(result.series[0].points.len(), 1);
        assert!((result.series[0].points[0].value - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_convert_matrix_response() {
        let json = r#"{
            "status": "success",
            "data": {
                "resultType": "matrix",
                "result": [
                    {
                        "metric": {"__name__": "cpu", "host": "a"},
                        "values": [[100, "10.5"], [160, "20.3"], [220, "15.0"]]
                    }
                ]
            }
        }"#;
        let resp: PromqlResponse<PromqlQueryData> = serde_json::from_str(json).unwrap();
        let result = convert_query_response(resp).unwrap();

        assert_eq!(result.result_type, MetricResultType::Matrix);
        assert_eq!(result.series[0].points.len(), 3);

        let stats = result.series[0].stats.as_ref().unwrap();
        assert_eq!(stats.count, 3);
        assert!((stats.min.unwrap() - 10.5).abs() < f64::EPSILON);
        assert!((stats.max.unwrap() - 20.3).abs() < f64::EPSILON);
    }

    #[test]
    fn test_convert_error_response() {
        let json = r#"{
            "status": "error",
            "errorType": "422",
            "error": "cannot parse query: syntax error"
        }"#;
        let resp: PromqlResponse<PromqlQueryData> = serde_json::from_str(json).unwrap();
        let err = convert_query_response(resp).unwrap_err();

        match err {
            ObzError::Provider { code, message, .. } => {
                assert_eq!(code, ErrorCode::QuerySyntax);
                assert!(message.contains("syntax error"));
            }
            _ => panic!("expected Provider error"),
        }
    }

    #[test]
    fn test_convert_empty_result() {
        let json = r#"{
            "status": "success",
            "data": {"resultType": "vector", "result": []}
        }"#;
        let resp: PromqlResponse<PromqlQueryData> = serde_json::from_str(json).unwrap();
        let result = convert_query_response(resp).unwrap();
        assert_eq!(result.total_count, 0);
        assert!(result.series.is_empty());
    }

    #[test]
    fn test_parse_metric_type_values() {
        assert_eq!(parse_metric_type("gauge"), MetricType::Gauge);
        assert_eq!(parse_metric_type("counter"), MetricType::Counter);
        assert_eq!(parse_metric_type("histogram"), MetricType::Histogram);
        assert_eq!(parse_metric_type("summary"), MetricType::Histogram);
        assert_eq!(parse_metric_type("unknown"), MetricType::Unknown);
        assert_eq!(parse_metric_type("untyped"), MetricType::Unknown);
    }

    // --- Fixture-based tests ---
    // Load real VM API responses from tests/fixtures/ and verify conversion.

    fn fixture_dir() -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/victoria/metrics")
    }

    fn load_fixture(name: &str) -> serde_json::Value {
        let path = fixture_dir().join(name);
        let content = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("fixture {}: {e}", path.display()));
        serde_json::from_str(&content).expect("invalid fixture JSON")
    }

    fn get_body(f: &serde_json::Value) -> &serde_json::Value {
        &f["response"]["body"]
    }

    fn get_facts(f: &serde_json::Value) -> &serde_json::Value {
        &f["facts"]
    }

    #[test]
    fn fixture_instant_vector_single() {
        let f = load_fixture("instant-vector-single.json");
        let resp: PromqlResponse<PromqlQueryData> =
            serde_json::from_value(get_body(&f).clone()).unwrap();
        let r = convert_query_response(resp).unwrap();
        assert_eq!(r.result_type, MetricResultType::Vector);
        assert_eq!(
            r.series.len(),
            get_facts(&f)["series_count"].as_u64().unwrap() as usize
        );
    }

    #[test]
    fn fixture_instant_vector_multi() {
        let f = load_fixture("instant-vector-multi.json");
        let resp: PromqlResponse<PromqlQueryData> =
            serde_json::from_value(get_body(&f).clone()).unwrap();
        let r = convert_query_response(resp).unwrap();
        assert_eq!(
            r.series.len(),
            get_facts(&f)["series_count"].as_u64().unwrap() as usize
        );
    }

    #[test]
    fn fixture_instant_empty() {
        let f = load_fixture("instant-empty.json");
        let resp: PromqlResponse<PromqlQueryData> =
            serde_json::from_value(get_body(&f).clone()).unwrap();
        let r = convert_query_response(resp).unwrap();
        assert!(r.series.is_empty());
    }

    #[test]
    fn fixture_range_matrix_small() {
        let f = load_fixture("range-matrix-small.json");
        let resp: PromqlResponse<PromqlQueryData> =
            serde_json::from_value(get_body(&f).clone()).unwrap();
        let r = convert_query_response(resp).unwrap();
        assert_eq!(r.result_type, MetricResultType::Matrix);
        assert_eq!(
            r.series.len(),
            get_facts(&f)["series_count"].as_u64().unwrap() as usize
        );
        let total: usize = r.series.iter().map(|s| s.points.len()).sum();
        assert_eq!(
            total,
            get_facts(&f)["total_points"].as_u64().unwrap() as usize
        );
    }

    #[test]
    fn fixture_range_matrix_multi() {
        let f = load_fixture("range-matrix-multi.json");
        let resp: PromqlResponse<PromqlQueryData> =
            serde_json::from_value(get_body(&f).clone()).unwrap();
        let r = convert_query_response(resp).unwrap();
        assert_eq!(
            r.series.len(),
            get_facts(&f)["series_count"].as_u64().unwrap() as usize
        );
        let total: usize = r.series.iter().map(|s| s.points.len()).sum();
        assert_eq!(
            total,
            get_facts(&f)["total_points"].as_u64().unwrap() as usize
        );
    }

    #[test]
    fn fixture_range_empty() {
        let f = load_fixture("range-empty-matrix.json");
        let resp: PromqlResponse<PromqlQueryData> =
            serde_json::from_value(get_body(&f).clone()).unwrap();
        let r = convert_query_response(resp).unwrap();
        assert!(r.series.is_empty());
    }

    #[test]
    fn fixture_labels() {
        let f = load_fixture("labels.json");
        let resp: PromqlResponse<Vec<String>> =
            serde_json::from_value(get_body(&f).clone()).unwrap();
        let r = convert_string_list_response(resp).unwrap();
        assert_eq!(
            r.len(),
            get_facts(&f)["data_count"].as_u64().unwrap() as usize
        );
    }

    #[test]
    fn fixture_label_values() {
        let f = load_fixture("label-values.json");
        let resp: PromqlResponse<Vec<String>> =
            serde_json::from_value(get_body(&f).clone()).unwrap();
        let r = convert_string_list_response(resp).unwrap();
        assert_eq!(
            r.len(),
            get_facts(&f)["data_count"].as_u64().unwrap() as usize
        );
    }

    #[test]
    fn fixture_series_metadata() {
        let f = load_fixture("series-metadata.json");
        let resp: PromqlResponse<Vec<BTreeMap<String, String>>> =
            serde_json::from_value(get_body(&f).clone()).unwrap();
        let r = convert_series_response(resp).unwrap();
        assert_eq!(
            r.len(),
            get_facts(&f)["data_count"].as_u64().unwrap() as usize
        );
    }

    #[test]
    fn fixture_error_syntax() {
        let f = load_fixture("error-syntax.json");
        let resp: PromqlResponse<PromqlQueryData> =
            serde_json::from_value(get_body(&f).clone()).unwrap();
        assert!(convert_query_response(resp).is_err());
    }

    #[test]
    fn fixture_stats_consistency() {
        let f = load_fixture("range-matrix-multi.json");
        let resp: PromqlResponse<PromqlQueryData> =
            serde_json::from_value(get_body(&f).clone()).unwrap();
        let r = convert_query_response(resp).unwrap();
        for s in &r.series {
            let stats = s.stats.as_ref().unwrap();
            if stats.count > 0 {
                let min = stats.min.unwrap();
                let max = stats.max.unwrap();
                let avg = stats.avg.unwrap();
                assert!(min <= max);
                assert!(avg >= min && avg <= max);
            }
        }
    }

    // --- SLS Metric fixture tests ---
    // Verify that SLS PromQL responses parse identically to VM responses.

    fn sls_fixture_dir() -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/sls/metrics")
    }

    fn load_sls_fixture(name: &str) -> serde_json::Value {
        let path = sls_fixture_dir().join(name);
        let content = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("fixture {}: {e}", path.display()));
        serde_json::from_str(&content).expect("invalid fixture JSON")
    }

    #[test]
    fn sls_fixture_metric_instant_query() {
        let f = load_sls_fixture("metric-instant-query.json");
        let resp: PromqlResponse<PromqlQueryData> =
            serde_json::from_value(get_body(&f).clone()).unwrap();
        let r = convert_query_response(resp).unwrap();
        assert_eq!(r.result_type, MetricResultType::Vector);
        assert!(!r.series.is_empty());
        // All series should have the name "container_cpu_utilization".
        for s in &r.series {
            assert_eq!(s.name, "container_cpu_utilization");
            assert_eq!(s.points.len(), 1);
        }
    }

    #[test]
    fn sls_fixture_metric_range_query() {
        let f = load_sls_fixture("metric-range-query.json");
        let resp: PromqlResponse<PromqlQueryData> =
            serde_json::from_value(get_body(&f).clone()).unwrap();
        let r = convert_query_response(resp).unwrap();
        assert_eq!(r.result_type, MetricResultType::Matrix);
        assert!(!r.series.is_empty());
        // Each series should have multiple points.
        for s in &r.series {
            assert!(
                s.points.len() > 1,
                "range query should have multiple points"
            );
        }
    }

    #[test]
    fn sls_fixture_metric_query_empty() {
        let f = load_sls_fixture("metric-query-empty.json");
        let resp: PromqlResponse<PromqlQueryData> =
            serde_json::from_value(get_body(&f).clone()).unwrap();
        let r = convert_query_response(resp).unwrap();
        assert!(r.series.is_empty());
        assert_eq!(r.total_count, 0);
    }

    #[test]
    fn sls_fixture_metric_label_values() {
        let f = load_sls_fixture("metric-label-values.json");
        let resp: PromqlResponse<Vec<String>> =
            serde_json::from_value(get_body(&f).clone()).unwrap();
        let r = convert_string_list_response(resp).unwrap();
        assert!(!r.is_empty(), "should have metric names");
    }

    #[test]
    fn sls_fixture_metric_query_aggr() {
        let f = load_sls_fixture("metric-query-aggr.json");
        let resp: PromqlResponse<PromqlQueryData> =
            serde_json::from_value(get_body(&f).clone()).unwrap();
        let r = convert_query_response(resp).unwrap();
        assert_eq!(r.result_type, MetricResultType::Vector);
        assert!(!r.series.is_empty());
    }

    // --- Error classification tests ---

    #[test]
    fn test_check_error_vm_422_syntax() {
        let result = check_error("error", Some("parse error: unexpected"), Some("422"));
        assert!(result.is_err());
        let err = result.unwrap_err();
        match err {
            ObzError::Provider { code, .. } => {
                assert_eq!(code, ErrorCode::QuerySyntax);
            }
            _ => panic!("expected Provider error, got {err:?}"),
        }
    }

    #[test]
    fn test_check_error_sls_bad_data_syntax() {
        // SLS returns errorType "bad_data" with a parse error message.
        let result = check_error(
            "error",
            Some("bad_data: invalid parameter '\"query\"': 1:9: parse error"),
            Some("bad_data"),
        );
        assert!(result.is_err());
        let err = result.unwrap_err();
        match err {
            ObzError::Provider { code, .. } => {
                assert_eq!(code, ErrorCode::QuerySyntax);
            }
            _ => panic!("expected Provider error, got {err:?}"),
        }
    }

    #[test]
    fn test_check_error_unknown_type() {
        let result = check_error("error", Some("something failed"), Some("unknown"));
        assert!(result.is_err());
        let err = result.unwrap_err();
        match err {
            ObzError::Provider { code, .. } => {
                assert_eq!(code, ErrorCode::BackendError);
            }
            _ => panic!("expected Provider error, got {err:?}"),
        }
    }

    #[test]
    fn test_check_error_success() {
        let result = check_error("success", None, None);
        assert!(result.is_ok());
    }

    #[test]
    fn vm_fixture_metric_error_syntax() {
        let f = load_fixture("error-syntax.json");
        let resp: PromqlResponse<PromqlQueryData> =
            serde_json::from_value(get_body(&f).clone()).unwrap();
        let result = convert_query_response(resp);
        assert!(result.is_err(), "syntax error should return Err");
        let err = result.unwrap_err();
        match err {
            ObzError::Provider { code, .. } => {
                assert_eq!(code, ErrorCode::QuerySyntax);
            }
            _ => panic!("expected Provider error, got {err:?}"),
        }
    }

    #[test]
    fn sls_fixture_metric_error_syntax() {
        let f = load_sls_fixture("metric-error-syntax.json");
        let resp: PromqlResponse<PromqlQueryData> =
            serde_json::from_value(get_body(&f).clone()).unwrap();
        let result = convert_query_response(resp);
        assert!(result.is_err(), "SLS syntax error should return Err");
        let err = result.unwrap_err();
        match err {
            ObzError::Provider { code, .. } => {
                assert_eq!(code, ErrorCode::QuerySyntax);
            }
            _ => panic!("expected Provider error, got {err:?}"),
        }
    }

    #[test]
    fn sls_fixture_metric_series() {
        let f = load_sls_fixture("metric-series.json");
        let resp: PromqlResponse<Vec<std::collections::BTreeMap<String, String>>> =
            serde_json::from_value(get_body(&f).clone()).unwrap();
        let r = convert_series_response(resp).unwrap();
        assert!(!r.is_empty(), "should have series");
    }

    #[test]
    fn sls_fixture_metric_series_empty() {
        let f = load_sls_fixture("metric-series-empty.json");
        let resp: PromqlResponse<Vec<std::collections::BTreeMap<String, String>>> =
            serde_json::from_value(get_body(&f).clone()).unwrap();
        let r = convert_series_response(resp).unwrap();
        assert!(r.is_empty(), "should have no series");
    }

    #[test]
    fn sls_fixture_metric_labels() {
        let f = load_sls_fixture("metric-labels.json");
        let resp: PromqlResponse<Vec<String>> =
            serde_json::from_value(get_body(&f).clone()).unwrap();
        let r = convert_string_list_response(resp).unwrap();
        assert!(!r.is_empty(), "should have labels");
    }
}
