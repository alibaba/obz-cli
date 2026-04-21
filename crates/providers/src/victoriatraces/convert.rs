//! Re-export shared Jaeger API conversion functions.
//!
//! `VictoriaTraces` uses the standard Jaeger HTTP API response format.
//! All conversion logic is defined in the shared `jaegerapi` module.

pub(crate) use crate::jaegerapi::convert::*;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use crate::jaegerapi::convert::*;
    use crate::jaegerapi::response::{JaegerResponse, JaegerTrace};
    use obz_core::model::trace::SpanStatus;

    /// Load the `response.body` from an obsx fixture file and parse as Jaeger response.
    fn load_fixture(name: &str) -> Vec<JaegerTrace> {
        let manifest = env!("CARGO_MANIFEST_DIR");
        // CARGO_MANIFEST_DIR is the providers crate dir; fixtures are at workspace root.
        let workspace = manifest.trim_end_matches("crates/providers");
        let path = format!("{workspace}fixtures/victoria/traces/{name}.json");
        let raw =
            std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("fixture not found: {path}"));
        let wrapper: serde_json::Value = serde_json::from_str(&raw).expect("invalid JSON");
        let body = &wrapper["response"]["body"];
        serde_json::from_value::<JaegerResponse<Vec<JaegerTrace>>>(body.clone())
            .unwrap_or_else(|e| panic!("parse error in {name}: {e}"))
            .data
            .unwrap_or_default()
    }

    #[test]
    fn fixture_search_by_service() {
        let traces = load_fixture("search-by-service");
        assert!(!traces.is_empty(), "should have at least one trace");
        let result = convert_search_result(traces);
        assert!(!result.spans.is_empty(), "should have spans");
        for span in &result.spans {
            assert!(!span.service.is_empty(), "service should not be empty");
            assert!(!span.trace_id.is_empty(), "trace_id should not be empty");
        }
    }

    #[test]
    fn fixture_search_empty() {
        let traces = load_fixture("search-empty");
        let result = convert_search_result(traces);
        assert_eq!(result.spans.len(), 0);
        assert_eq!(result.total_count, 0);
    }

    #[test]
    fn fixture_trace_by_id() {
        let traces = load_fixture("trace-by-id");
        assert_eq!(traces.len(), 1, "should have exactly one trace");
        let detail = convert_trace_detail(&traces.into_iter().next().unwrap());
        assert!(!detail.trace_id.is_empty());
        assert!(detail.span_count > 0);
        assert!(detail.service_count > 0);
        assert!(!detail.services.is_empty());
        // Verify spans are sorted by start_time ascending.
        let times: Vec<i64> = detail.spans.iter().map(|s| s.start_time).collect();
        let mut sorted = times.clone();
        sorted.sort();
        assert_eq!(times, sorted, "spans should be sorted by start_time asc");
    }

    #[test]
    fn fixture_search_by_operation() {
        let traces = load_fixture("search-by-operation");
        let result = convert_search_result(traces);
        assert!(!result.spans.is_empty(), "should have spans");
    }

    #[test]
    fn fixture_search_by_error() {
        let traces = load_fixture("search-by-error");
        let result = convert_search_result(traces);
        // At least some spans should have error status.
        let has_error = result.spans.iter().any(|s| s.status == SpanStatus::Error);
        assert!(
            has_error,
            "error search should return spans with error status"
        );
    }

    /// Load the Jaeger operations response from a fixture.
    fn load_operations_fixture(name: &str) -> Vec<String> {
        let manifest = env!("CARGO_MANIFEST_DIR");
        let workspace = manifest.trim_end_matches("crates/providers");
        let path = format!("{workspace}fixtures/victoria/traces/{name}.json");
        let raw =
            std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("fixture not found: {path}"));
        let wrapper: serde_json::Value = serde_json::from_str(&raw).expect("invalid JSON");
        let body = &wrapper["response"]["body"];
        serde_json::from_value::<JaegerResponse<Vec<String>>>(body.clone())
            .unwrap_or_else(|e| panic!("parse error in {name}: {e}"))
            .data
            .unwrap_or_default()
    }

    #[test]
    fn fixture_operations_cart() {
        let ops = load_operations_fixture("operations-cart");
        assert!(!ops.is_empty(), "cart service should have operations");
    }

    #[test]
    fn fixture_operations_empty() {
        let ops = load_operations_fixture("operations-empty");
        assert!(ops.is_empty(), "empty operations should return no items");
    }
}
