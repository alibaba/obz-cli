//! Re-export shared PromQL conversion functions.
//!
//! Mimir returns identical response payloads to Prometheus, so we reuse
//! the shared conversion logic from the `promql` module.

#[allow(unused_imports)]
pub(crate) use crate::promql::convert::*;

// ---------------------------------------------------------------------------
// Fixture tests — verify conversion against recorded Mimir (Grafana) fixtures
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::promql::response::{PromqlMetadataEntry, PromqlQueryData, PromqlResponse};
    use std::collections::BTreeMap;

    /// Load a fixture file and extract the response body.
    fn load_fixture(name: &str) -> serde_json::Value {
        let path = format!(
            "{}/fixtures/grafana/metrics/{name}.json",
            env!("CARGO_MANIFEST_DIR").replace("/crates/providers", "")
        );
        let content = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("failed to read fixture {path}: {e}"));
        let fixture: serde_json::Value = serde_json::from_str(&content)
            .unwrap_or_else(|e| panic!("failed to parse fixture {path}: {e}"));
        fixture["response"]["body"].clone()
    }

    // -- Instant query tests ------------------------------------------------

    #[test]
    fn instant_vector_single() {
        let body = load_fixture("instant-vector-single");
        let resp: PromqlResponse<PromqlQueryData> = serde_json::from_value(body).unwrap();
        let result = convert_query_response(resp).unwrap();
        assert!(!result.series.is_empty());
        // Each series in an instant query has exactly one data point
        for s in &result.series {
            assert_eq!(s.points.len(), 1);
        }
    }

    #[test]
    fn instant_vector_multi() {
        let body = load_fixture("instant-vector-multi");
        let resp: PromqlResponse<PromqlQueryData> = serde_json::from_value(body).unwrap();
        let result = convert_query_response(resp).unwrap();
        assert!(result.series.len() > 1);
    }

    #[test]
    fn instant_empty() {
        let body = load_fixture("instant-empty");
        let resp: PromqlResponse<PromqlQueryData> = serde_json::from_value(body).unwrap();
        let result = convert_query_response(resp).unwrap();
        assert!(result.series.is_empty());
    }

    #[test]
    fn instant_scalar_bad_data_error() {
        // Mimir returns HTTP 400 (bad_data) for `scalar(1+1)` — this tests
        // the error envelope handling, not the scalar type conversion logic.
        let body = load_fixture("instant-scalar");
        let resp: PromqlResponse<PromqlQueryData> = serde_json::from_value(body).unwrap();
        let result = convert_query_response(resp);
        assert!(result.is_err());
    }

    // -- Range query tests --------------------------------------------------

    #[test]
    fn range_matrix_multi() {
        let body = load_fixture("range-matrix-multi");
        let resp: PromqlResponse<PromqlQueryData> = serde_json::from_value(body).unwrap();
        let result = convert_query_response(resp).unwrap();
        assert!(result.series.len() > 1);
        // Range results should have multiple data points
        for s in &result.series {
            assert!(s.points.len() > 1);
        }
    }

    #[test]
    fn range_empty_matrix() {
        let body = load_fixture("range-empty-matrix");
        let resp: PromqlResponse<PromqlQueryData> = serde_json::from_value(body).unwrap();
        let result = convert_query_response(resp).unwrap();
        assert!(result.series.is_empty());
    }

    // -- Metadata tests -----------------------------------------------------

    #[test]
    fn labels() {
        let body = load_fixture("labels");
        let resp: PromqlResponse<Vec<String>> = serde_json::from_value(body).unwrap();
        let result = convert_string_list_response(resp).unwrap();
        assert!(!result.is_empty());
    }

    #[test]
    fn label_values() {
        let body = load_fixture("label-values");
        let resp: PromqlResponse<Vec<String>> = serde_json::from_value(body).unwrap();
        let result = convert_string_list_response(resp).unwrap();
        assert!(!result.is_empty());
    }

    #[test]
    fn label_values_empty() {
        let body = load_fixture("label-values-empty");
        let resp: PromqlResponse<Vec<String>> = serde_json::from_value(body).unwrap();
        let result = convert_string_list_response(resp).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn metadata_empty() {
        // The Mimir fixture returns an empty metadata object (no metadata
        // found for the queried metric in the OTel Demo environment).
        let body = load_fixture("metadata");
        let resp: PromqlResponse<BTreeMap<String, Vec<PromqlMetadataEntry>>> =
            serde_json::from_value(body).unwrap();
        let result = convert_metadata_response(resp, None).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn series_metadata() {
        let body = load_fixture("series-metadata");
        let resp: PromqlResponse<Vec<BTreeMap<String, String>>> =
            serde_json::from_value(body).unwrap();
        let result = convert_series_response(resp).unwrap();
        assert!(!result.is_empty());
    }

    #[test]
    fn series_empty() {
        let body = load_fixture("series-empty");
        let resp: PromqlResponse<Vec<BTreeMap<String, String>>> =
            serde_json::from_value(body).unwrap();
        let result = convert_series_response(resp).unwrap();
        assert!(result.is_empty());
    }

    // -- Error tests --------------------------------------------------------

    #[test]
    fn error_syntax() {
        let body = load_fixture("error-syntax");
        let resp: PromqlResponse<PromqlQueryData> = serde_json::from_value(body).unwrap();
        let result = convert_query_response(resp);
        assert!(result.is_err());
    }

    #[test]
    fn error_missing_query() {
        let body = load_fixture("error-missing-query");
        let resp: PromqlResponse<PromqlQueryData> = serde_json::from_value(body).unwrap();
        let result = convert_query_response(resp);
        assert!(result.is_err());
    }
}
