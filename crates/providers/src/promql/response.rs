//! Prometheus-compatible API response deserialization types.
//!
//! These types map directly to the JSON responses from the
//! Prometheus HTTP API (used by VictoriaMetrics, SLS MetricStore, etc.).
//! They are shared across providers that expose a PromQL-compatible
//! endpoint and are converted to obz models before being returned.

use std::collections::BTreeMap;

use serde::Deserialize;

/// Top-level `PromQL` API response envelope.
///
/// The `data` field is polymorphic — it can be a query result object,
/// a string array (labels/label-values), or a label-set array (series).
#[derive(Debug, Deserialize)]
pub(crate) struct PromqlResponse<T> {
    pub status: String,
    pub data: Option<T>,
    pub error: Option<String>,
    #[serde(rename = "errorType")]
    pub error_type: Option<String>,
}

/// Query result data (`/api/v1/query` and `/api/v1/query_range`).
#[derive(Debug, Deserialize)]
pub(crate) struct PromqlQueryData {
    #[serde(rename = "resultType")]
    pub result_type: String,
    pub result: Vec<PromqlSample>,
}

/// A single sample from a `PromQL` query result.
///
/// For instant queries, `value` is populated (single data point).
/// For range queries, `values` is populated (multiple data points).
#[derive(Debug, Deserialize)]
pub(crate) struct PromqlSample {
    /// Label set. May be empty for scalar queries.
    #[serde(default)]
    pub metric: BTreeMap<String, String>,

    /// Single data point for instant queries: `[timestamp, "value"]`.
    pub value: Option<PromqlDataPoint>,

    /// Multiple data points for range queries: `[[ts, "val"], ...]`.
    pub values: Option<Vec<PromqlDataPoint>>,
}

/// A `PromQL` data point: `[timestamp_number, "value_string"]`.
///
/// The Prometheus HTTP API always returns values as JSON strings,
/// even for integers. Timestamps are JSON numbers (Unix seconds).
#[derive(Debug)]
pub(crate) struct PromqlDataPoint {
    pub timestamp: i64,
    pub value: String,
}

impl<'de> Deserialize<'de> for PromqlDataPoint {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let (ts, val): (serde_json::Value, String) = Deserialize::deserialize(deserializer)?;
        let timestamp = match &ts {
            serde_json::Value::Number(n) => n
                .as_i64()
                .or_else(|| n.as_f64().map(|f| f as i64))
                .ok_or_else(|| {
                    serde::de::Error::custom(format!("invalid timestamp number: {n}"))
                })?,
            _ => {
                return Err(serde::de::Error::custom(format!(
                    "expected number for timestamp, got: {ts}"
                )));
            }
        };
        Ok(PromqlDataPoint {
            timestamp,
            value: val,
        })
    }
}

/// Prometheus metadata entry for `/api/v1/metadata`.
#[derive(Debug, Deserialize)]
pub(crate) struct PromqlMetadataEntry {
    #[serde(rename = "type")]
    pub metric_type: Option<String>,
    pub help: Option<String>,
    pub unit: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_promql_data_point_deserialization() {
        let json = r#"[1774605500, "5"]"#;
        let dp: PromqlDataPoint = serde_json::from_str(json).unwrap();
        assert_eq!(dp.timestamp, 1774605500);
        assert_eq!(dp.value, "5");
    }

    #[test]
    fn test_promql_data_point_float_value() {
        let json = r#"[1774605500, "0.03333333333333333"]"#;
        let dp: PromqlDataPoint = serde_json::from_str(json).unwrap();
        assert_eq!(dp.value, "0.03333333333333333");
    }

    #[test]
    fn test_promql_query_response_vector() {
        let json = r#"{
            "status": "success",
            "data": {
                "resultType": "vector",
                "result": [{
                    "metric": {"__name__": "up", "job": "api"},
                    "value": [1774605500, "1"]
                }]
            }
        }"#;
        let resp: PromqlResponse<PromqlQueryData> = serde_json::from_str(json).unwrap();
        assert_eq!(resp.status, "success");
        let data = resp.data.unwrap();
        assert_eq!(data.result_type, "vector");
        assert_eq!(data.result.len(), 1);
        assert_eq!(data.result[0].metric["__name__"], "up");
        let val = data.result[0].value.as_ref().unwrap();
        assert_eq!(val.timestamp, 1774605500);
        assert_eq!(val.value, "1");
    }

    #[test]
    fn test_promql_query_response_matrix() {
        let json = r#"{
            "status": "success",
            "data": {
                "resultType": "matrix",
                "result": [{
                    "metric": {"__name__": "up", "job": "api"},
                    "values": [[1774603700, "3"], [1774603760, "3"]]
                }]
            }
        }"#;
        let resp: PromqlResponse<PromqlQueryData> = serde_json::from_str(json).unwrap();
        let data = resp.data.unwrap();
        assert_eq!(data.result_type, "matrix");
        let vals = data.result[0].values.as_ref().unwrap();
        assert_eq!(vals.len(), 2);
        assert_eq!(vals[0].timestamp, 1774603700);
    }

    #[test]
    fn test_promql_labels_response() {
        let json = r#"{"status": "success", "data": ["__name__", "job", "instance"]}"#;
        let resp: PromqlResponse<Vec<String>> = serde_json::from_str(json).unwrap();
        let data = resp.data.unwrap();
        assert_eq!(data.len(), 3);
        assert_eq!(data[0], "__name__");
    }

    #[test]
    fn test_promql_series_response() {
        let json = r#"{
            "status": "success",
            "data": [
                {"__name__": "up", "job": "api", "instance": "localhost:9090"}
            ]
        }"#;
        let resp: PromqlResponse<Vec<BTreeMap<String, String>>> =
            serde_json::from_str(json).unwrap();
        let data = resp.data.unwrap();
        assert_eq!(data.len(), 1);
        assert_eq!(data[0]["__name__"], "up");
    }

    #[test]
    fn test_promql_error_response() {
        let json = r#"{
            "status": "error",
            "errorType": "422",
            "error": "missing `query` arg"
        }"#;
        let resp: PromqlResponse<PromqlQueryData> = serde_json::from_str(json).unwrap();
        assert_eq!(resp.status, "error");
        assert_eq!(resp.error.as_deref(), Some("missing `query` arg"));
        assert_eq!(resp.error_type.as_deref(), Some("422"));
    }

    #[test]
    fn test_promql_empty_metric_scalar() {
        let json = r#"{
            "status": "success",
            "data": {
                "resultType": "vector",
                "result": [{
                    "metric": {},
                    "value": [1774605500, "2"]
                }]
            }
        }"#;
        let resp: PromqlResponse<PromqlQueryData> = serde_json::from_str(json).unwrap();
        let data = resp.data.unwrap();
        assert!(data.result[0].metric.is_empty());
    }
}
