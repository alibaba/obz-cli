//! Re-export shared Prometheus API conversion functions.
//!
//! Prometheus uses the standard PromQL HTTP API response format.
//! All conversion logic is defined in the shared `promql` module.

// Re-export kept for test imports and future provider-specific overrides.
#[allow(unused_imports)]
pub(crate) use crate::promql::convert::*;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use crate::promql::convert::*;
    use crate::promql::response::{PromqlQueryData, PromqlResponse};
    use obz_core::provider::results::MetricResultType;
    use std::collections::BTreeMap;

    fn fixture_dir() -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/prometheus/metrics")
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
        let result = convert_query_response(resp).unwrap();
        let facts = get_facts(&f);
        assert_eq!(result.result_type, MetricResultType::Vector);
        assert_eq!(
            result.series.len(),
            facts["series_count"].as_u64().unwrap() as usize
        );
    }

    #[test]
    fn fixture_instant_vector_multi() {
        let f = load_fixture("instant-vector-multi.json");
        let resp: PromqlResponse<PromqlQueryData> =
            serde_json::from_value(get_body(&f).clone()).unwrap();
        let result = convert_query_response(resp).unwrap();
        let facts = get_facts(&f);
        assert_eq!(result.result_type, MetricResultType::Vector);
        assert_eq!(
            result.series.len(),
            facts["series_count"].as_u64().unwrap() as usize
        );
    }

    #[test]
    fn fixture_range_matrix_rate() {
        let f = load_fixture("range-matrix-rate.json");
        let resp: PromqlResponse<PromqlQueryData> =
            serde_json::from_value(get_body(&f).clone()).unwrap();
        let result = convert_query_response(resp).unwrap();
        let facts = get_facts(&f);
        assert_eq!(result.result_type, MetricResultType::Matrix);
        assert_eq!(
            result.series.len(),
            facts["series_count"].as_u64().unwrap() as usize
        );
    }

    #[test]
    fn fixture_instant_empty() {
        let f = load_fixture("instant-empty.json");
        let resp: PromqlResponse<PromqlQueryData> =
            serde_json::from_value(get_body(&f).clone()).unwrap();
        let result = convert_query_response(resp).unwrap();
        assert_eq!(result.total_count, 0);
        assert!(result.series.is_empty());
    }

    #[test]
    fn fixture_range_empty_matrix() {
        let f = load_fixture("range-empty-matrix.json");
        let resp: PromqlResponse<PromqlQueryData> =
            serde_json::from_value(get_body(&f).clone()).unwrap();
        let result = convert_query_response(resp).unwrap();
        assert_eq!(result.total_count, 0);
    }

    #[test]
    fn fixture_labels() {
        let f = load_fixture("labels.json");
        let resp: PromqlResponse<Vec<String>> =
            serde_json::from_value(get_body(&f).clone()).unwrap();
        let result = convert_string_list_response(resp).unwrap();
        assert!(!result.is_empty());
    }

    #[test]
    fn fixture_label_values() {
        let f = load_fixture("label-values.json");
        let resp: PromqlResponse<Vec<String>> =
            serde_json::from_value(get_body(&f).clone()).unwrap();
        let result = convert_string_list_response(resp).unwrap();
        assert!(!result.is_empty());
    }

    #[test]
    fn fixture_series_metadata() {
        let f = load_fixture("series-metadata.json");
        let resp: PromqlResponse<Vec<BTreeMap<String, String>>> =
            serde_json::from_value(get_body(&f).clone()).unwrap();
        let result = convert_series_response(resp).unwrap();
        assert!(!result.is_empty());
    }

    #[test]
    fn fixture_error_syntax() {
        let f = load_fixture("error-syntax.json");
        let resp: PromqlResponse<PromqlQueryData> =
            serde_json::from_value(get_body(&f).clone()).unwrap();
        let err = convert_query_response(resp).unwrap_err();
        match err {
            obz_core::model::error::ObzError::Provider { code, .. } => {
                assert_eq!(code, obz_core::model::error::ErrorCode::QuerySyntax);
            }
            _ => panic!("expected Provider error"),
        }
    }
}
