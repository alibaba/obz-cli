//! `GreptimeDB` metric provider.
//!
//! `GreptimeDB` exposes a Prometheus-compatible HTTP API under the
//! `/v1/prometheus` path prefix. This module wraps the shared
//! [`PromqlMetricProvider`] for query, labels, label-values, and series
//! commands while keeping GreptimeDB-specific request validation local.
//!
//! # Database selection
//!
//! `GreptimeDB` routes Prometheus API requests by a `db` query parameter.
//! obz requires this as explicit provider config (`db`) and injects it into
//! every `GreptimeDB` request instead of asking users to put query strings in
//! the endpoint.
//!
//! # API Endpoints
//!
//! | Command                | Endpoint                                          |
//! |------------------------|---------------------------------------------------|
//! | `metric query` (instant)  | `GET /v1/prometheus/api/v1/query`              |
//! | `metric query` (range)    | `GET /v1/prometheus/api/v1/query_range`        |
//! | `metric labels`           | `GET /v1/prometheus/api/v1/labels`             |
//! | `metric label-values`     | `GET /v1/prometheus/api/v1/label/{name}/values` |
//! | `metric series`           | `GET /v1/prometheus/api/v1/series`             |

/// `GreptimeDB` → obz model conversion functions (re-exported from `promql`).
pub(crate) mod convert;
/// `GreptimeDB` API response deserialization types (re-exported from `promql`).
pub(crate) mod response;

use std::collections::BTreeMap;

use async_trait::async_trait;

use obz_core::model::error::{ErrorCode, ObzError};
use obz_core::model::metric::MetricInfoDetail;
use obz_core::provider::{
    LabelValuesParams, MetricInfoParams, MetricMetadataParams, MetricProvider, MetricQueryParams,
    MetricQueryResult, ProviderResult,
};
use obz_core::registry::{
    BuiltProvider, CheckResult, CheckScope, CheckSeverity, ProviderMeta, SupportedCommands,
};

use crate::promql::provider::PromqlMetricProvider;
use crate::util::{
    build_http_client, parse_timeout_config, validate_custom_headers, validate_endpoint,
};

/// `GreptimeDB` metric provider.
///
/// The PromQL-compatible API does not currently expose Prometheus metadata,
/// so `metric info` is rejected locally instead of calling a known-missing
/// backend endpoint.
struct GreptimeDbMetricProvider {
    inner: PromqlMetricProvider,
}

#[async_trait]
impl MetricProvider for GreptimeDbMetricProvider {
    async fn query(&self, params: &MetricQueryParams) -> ProviderResult<MetricQueryResult> {
        self.inner.query(params).await
    }

    async fn list(&self, _params: &MetricMetadataParams) -> ProviderResult<Vec<String>> {
        Err(ObzError::Unsupported {
            message: "GreptimeDB does not provide reliable metric list semantics via the Prometheus API".to_string(),
            provider: Some("greptimedb".to_string()),
            suggestion: Some(
                "Use `obz metric series -p greptimedb --match '{__name__=\"your_metric\"}'` or query known metric names instead."
                    .to_string(),
            ),
        })
    }

    async fn info(&self, _params: &MetricInfoParams) -> ProviderResult<Vec<MetricInfoDetail>> {
        Err(ObzError::Unsupported {
            message: "GreptimeDB does not support metric info (metadata) queries".to_string(),
            provider: Some("greptimedb".to_string()),
            suggestion: Some(
                "Use metric labels, metric label-values with --match, or metric series instead."
                    .to_string(),
            ),
        })
    }

    async fn labels(&self, params: &MetricMetadataParams) -> ProviderResult<Vec<String>> {
        self.inner.labels(params).await
    }

    async fn label_values(&self, params: &LabelValuesParams) -> ProviderResult<Vec<String>> {
        if params.match_expr.is_none() {
            return Err(ObzError::InvalidArgument {
                code: ErrorCode::MissingRequired,
                message: "GreptimeDB metric label-values requires --match".to_string(),
                suggestion: Some(
                    "Pass a series selector with --match, for example `--match '{__name__=\"up\"}'`."
                        .to_string(),
                ),
            });
        }
        self.inner.label_values(params).await
    }

    async fn series(
        &self,
        params: &MetricMetadataParams,
    ) -> ProviderResult<Vec<BTreeMap<String, String>>> {
        self.inner.series(params).await
    }
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

/// Factory function: build a [`BuiltProvider`] for `GreptimeDB`.
fn build(config: &obz_core::provider::ProviderConfig) -> Result<BuiltProvider, ObzError> {
    let endpoint = config.require_config("endpoint")?;
    validate_greptimedb_endpoint(endpoint)?;
    let db = greptimedb_database(config)?;
    let basic = config.basic_auth();
    let verbose = config.verbose();
    let timeout = parse_timeout_config(config);
    let client = build_http_client(timeout)?;

    validate_custom_headers(config.custom_headers())?;
    let extra_headers: Vec<(String, String)> = config
        .custom_headers()
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();

    let provider = GreptimeDbMetricProvider {
        inner: PromqlMetricProvider::new(
            endpoint,
            client,
            config.bearer_token(),
            basic,
            "/v1/prometheus".to_string(),
            extra_headers,
            "GreptimeDB",
            verbose,
        )
        .with_extra_query(vec![("db".to_string(), db)]),
    };
    Ok(BuiltProvider {
        name: "greptimedb",
        metric_query_language: Some("PromQL"),
        log_query_language: None,
        metric: Some(Box::new(provider)),
        log: None,
        trace: None,
        extension: None,
    })
}

/// Return the [`ProviderMeta`] for `GreptimeDB`.
///
/// Called by [`obz_providers::register_all`] at startup.
pub(crate) fn meta() -> ProviderMeta {
    ProviderMeta {
        name: "greptimedb",
        display_name: "GreptimeDB",
        aliases: &["greptimedb", "greptime"],
        supported_commands: SupportedCommands {
            metric_query: true,
            metric_list: false,
            metric_info: false,
            metric_labels: true,
            metric_label_values: true,
            metric_series: true,
            log_search: false,
            trace_search: false,
            trace_get: false,
        },
        build,
        check: Some(|config| Box::pin(greptimedb_check(config))),
        command_flags: &[],
        extension_commands: &[],
    }
}

async fn greptimedb_check(config: &obz_core::provider::ProviderConfig) -> CheckResult {
    let Some(endpoint) = config.get("endpoint") else {
        return CheckResult {
            severity: CheckSeverity::Fail,
            message: "endpoint not configured".to_string(),
            scope: CheckScope::Connectivity,
            latency: None,
        };
    };

    if let Err(error) = validate_greptimedb_endpoint(endpoint) {
        return CheckResult {
            severity: CheckSeverity::Fail,
            message: error.to_string(),
            scope: CheckScope::Connectivity,
            latency: None,
        };
    }

    crate::probe::http_get_probe(config, "/v1/health").await
}

fn validate_greptimedb_endpoint(endpoint: &str) -> Result<(), ObzError> {
    validate_endpoint(endpoint)?;
    let url = reqwest::Url::parse(endpoint).map_err(|e| ObzError::InvalidArgument {
        code: ErrorCode::InvalidFlag,
        message: format!("--endpoint is not a valid URL: '{endpoint}' ({e})"),
        suggestion: None,
    })?;

    if url.query().is_some() || url.fragment().is_some() {
        return Err(ObzError::InvalidArgument {
            code: ErrorCode::InvalidFlag,
            message: "GreptimeDB --endpoint must be a base HTTP URL without query or fragment"
                .to_string(),
            suggestion: Some(
                "Use `endpoint: http://localhost:4000` and set `db: <database>` in the provider config instead of appending `?db=...`."
                    .to_string(),
            ),
        });
    }

    if url.path() != "/" {
        return Err(ObzError::InvalidArgument {
            code: ErrorCode::InvalidFlag,
            message: "GreptimeDB --endpoint must be the root HTTP URL without a path"
                .to_string(),
            suggestion: Some(
                "Use the GreptimeDB base URL such as `http://localhost:4000`; obz appends `/v1/prometheus/api/v1/...` automatically."
                    .to_string(),
            ),
        });
    }

    Ok(())
}

fn greptimedb_database(config: &obz_core::provider::ProviderConfig) -> Result<String, ObzError> {
    match config.get("db") {
        Some(db) if db.trim().is_empty() => Err(ObzError::InvalidArgument {
            code: ErrorCode::InvalidFlag,
            message: "GreptimeDB db config must not be empty".to_string(),
            suggestion: Some(
                "Set `db: public` or another GreptimeDB database name in the provider config."
                    .to_string(),
            ),
        }),
        Some(db) => Ok(db.to_string()),
        None => Err(ObzError::InvalidArgument {
            code: ErrorCode::MissingRequired,
            message: "GreptimeDB db config is required".to_string(),
            suggestion: Some(
                "Set `db: public` or another GreptimeDB database name in the provider config."
                    .to_string(),
            ),
        }),
    }
}

#[cfg(test)]
mod tests {
    use obz_core::provider::{MetricInfoParams, ProviderConfig};
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    fn test_config(endpoint: &str) -> ProviderConfig {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let mut config = ProviderConfig::new();
        config.set("endpoint", endpoint);
        config.set("db", "public");
        config
    }

    fn build_metric(config: &ProviderConfig) -> Box<dyn MetricProvider> {
        build(config)
            .expect("provider should build")
            .metric
            .expect("metric provider should exist")
    }

    fn build_error(endpoint: &str) -> ObzError {
        match build(&test_config(endpoint)) {
            Ok(_) => panic!("provider build should fail"),
            Err(err) => err,
        }
    }

    #[test]
    fn metadata_declares_metric_info_unsupported() {
        assert!(!meta().supported_commands.metric_info);
    }

    #[test]
    fn metadata_declares_metric_list_unsupported() {
        assert!(!meta().supported_commands.metric_list);
    }

    #[test]
    fn rejects_endpoint_with_query() {
        let err = build_error("http://localhost:4000?db=public");

        match err {
            ObzError::InvalidArgument {
                message,
                suggestion,
                ..
            } => {
                assert!(message.contains("base HTTP URL"));
                assert!(suggestion
                    .as_deref()
                    .is_some_and(|s| s.contains("db: <database>")));
            }
            other => panic!("expected InvalidArgument error, got {other:?}"),
        }
    }

    #[test]
    fn rejects_endpoint_with_fragment() {
        let err = build_error("http://localhost:4000#prometheus");

        assert!(matches!(err, ObzError::InvalidArgument { .. }));
    }

    #[test]
    fn rejects_endpoint_with_path() {
        let err = build_error("http://localhost:4000/v1/prometheus");

        match err {
            ObzError::InvalidArgument {
                message,
                suggestion,
                ..
            } => {
                assert!(message.contains("root HTTP URL"));
                assert!(suggestion
                    .as_deref()
                    .is_some_and(|s| s.contains("base URL")));
            }
            other => panic!("expected InvalidArgument error, got {other:?}"),
        }
    }

    #[test]
    fn rejects_endpoint_with_nested_path() {
        let err = build_error("http://localhost:4000/v1/prometheus/api/v1/query");

        assert!(matches!(err, ObzError::InvalidArgument { .. }));
    }

    #[test]
    fn rejects_endpoint_with_arbitrary_path() {
        let err = build_error("http://localhost:4000/dashboard");

        assert!(matches!(err, ObzError::InvalidArgument { .. }));
    }

    #[test]
    fn rejects_endpoint_with_api_v1_path() {
        let err = build_error("http://localhost:4000/api/v1");

        assert!(matches!(err, ObzError::InvalidArgument { .. }));
    }

    #[test]
    fn rejects_endpoint_with_api_v1_nested_path() {
        let err = build_error("http://localhost:4000/api/v1/query");

        assert!(matches!(err, ObzError::InvalidArgument { .. }));
    }

    #[test]
    fn rejects_empty_db_config() {
        let mut config = test_config("http://localhost:4000");
        config.set("db", "   ");

        let Err(err) = build(&config) else {
            panic!("provider build should fail");
        };

        match err {
            ObzError::InvalidArgument {
                message,
                suggestion,
                ..
            } => {
                assert!(message.contains("db"));
                assert!(suggestion.as_deref().is_some_and(|s| s.contains("public")));
            }
            other => panic!("expected InvalidArgument error, got {other:?}"),
        }
    }

    #[test]
    fn rejects_missing_db_config() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let mut config = ProviderConfig::new();
        config.set("endpoint", "http://localhost:4000");

        let Err(err) = build(&config) else {
            panic!("provider build should fail");
        };

        match err {
            ObzError::InvalidArgument {
                code,
                message,
                suggestion,
            } => {
                assert_eq!(code, ErrorCode::MissingRequired);
                assert!(message.contains("db"));
                assert!(suggestion
                    .as_deref()
                    .is_some_and(|s| s.contains("db: public")));
            }
            other => panic!("expected InvalidArgument error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn provider_check_rejects_invalid_greptimedb_endpoint() {
        let config = test_config("http://localhost:4000?db=public");

        let result = greptimedb_check(&config).await;

        assert_eq!(result.severity, CheckSeverity::Fail);
        assert!(result.message.contains("base HTTP URL"));
    }

    #[tokio::test]
    async fn provider_check_uses_health_endpoint_for_valid_config() {
        let server = MockServer::start().await;
        let config = test_config(&server.uri());

        Mock::given(method("GET"))
            .and(path("/v1/health"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
            .expect(1)
            .mount(&server)
            .await;

        let result = greptimedb_check(&config).await;

        assert_eq!(result.severity, CheckSeverity::Ok);
        assert!(result.message.contains("reachable"));
    }

    #[tokio::test]
    async fn metric_info_returns_unsupported_without_backend_request() {
        let config = test_config("http://localhost:4000");
        let metric = build_metric(&config);
        let err = metric
            .info(&MetricInfoParams {
                metric_name: "up".to_string(),
            })
            .await
            .unwrap_err();

        match err {
            ObzError::Unsupported {
                provider,
                suggestion,
                ..
            } => {
                assert_eq!(provider.as_deref(), Some("greptimedb"));
                assert!(suggestion.is_some());
            }
            other => panic!("expected Unsupported error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn metric_list_returns_unsupported_without_backend_request() {
        let server = MockServer::start().await;
        let config = test_config(&server.uri());
        let metric = build_metric(&config);

        let err = metric
            .list(&MetricMetadataParams {
                match_expr: None,
                match_exprs: Vec::new(),
                start: None,
                end: None,
                limit: None,
            })
            .await
            .unwrap_err();

        match err {
            ObzError::Unsupported {
                provider,
                suggestion,
                ..
            } => {
                assert_eq!(provider.as_deref(), Some("greptimedb"));
                assert!(suggestion
                    .as_deref()
                    .is_some_and(|s| s.contains("metric series")));
            }
            other => panic!("expected Unsupported error, got {other:?}"),
        }
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn label_values_requires_match_without_backend_request() {
        let server = MockServer::start().await;
        let config = test_config(&server.uri());
        let metric = build_metric(&config);

        let err = metric
            .label_values(&LabelValuesParams {
                label_name: "job".to_string(),
                match_expr: None,
                start: None,
                end: None,
                limit: None,
            })
            .await
            .unwrap_err();

        match err {
            ObzError::InvalidArgument {
                code,
                message,
                suggestion,
            } => {
                assert_eq!(code, ErrorCode::MissingRequired);
                assert!(message.contains("--match"));
                assert!(suggestion.as_deref().is_some_and(|s| s.contains("--match")));
            }
            other => panic!("expected InvalidArgument error, got {other:?}"),
        }
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn injects_db_config_into_prometheus_api_requests() {
        let server = MockServer::start().await;
        let mut config = test_config(&server.uri());
        config.set("db", "metrics_db");
        let metric = build_metric(&config);

        Mock::given(method("GET"))
            .and(path("/v1/prometheus/api/v1/query"))
            .and(query_param("db", "metrics_db"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "status": "success",
                "data": { "resultType": "vector", "result": [] }
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v1/prometheus/api/v1/labels"))
            .and(query_param("db", "metrics_db"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "status": "success",
                "data": []
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v1/prometheus/api/v1/label/job/values"))
            .and(query_param("db", "metrics_db"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "status": "success",
                "data": []
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v1/prometheus/api/v1/series"))
            .and(query_param("db", "metrics_db"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "status": "success",
                "data": []
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v1/prometheus/api/v1/query_range"))
            .and(query_param("db", "metrics_db"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "status": "success",
                "data": { "resultType": "matrix", "result": [] }
            })))
            .expect(1)
            .mount(&server)
            .await;

        metric
            .query(&MetricQueryParams {
                query: "up".to_string(),
                is_range: false,
                start: 1,
                end: 2,
                step: None,
                limit: None,
                timeout: None,
            })
            .await
            .unwrap();
        metric
            .labels(&MetricMetadataParams {
                match_expr: None,
                match_exprs: Vec::new(),
                start: None,
                end: None,
                limit: None,
            })
            .await
            .unwrap();
        metric
            .label_values(&LabelValuesParams {
                label_name: "job".to_string(),
                match_expr: Some("up".to_string()),
                start: None,
                end: None,
                limit: None,
            })
            .await
            .unwrap();
        metric
            .series(&MetricMetadataParams {
                match_expr: Some("up".to_string()),
                match_exprs: Vec::new(),
                start: None,
                end: None,
                limit: None,
            })
            .await
            .unwrap();
        metric
            .query(&MetricQueryParams {
                query: "up".to_string(),
                is_range: true,
                start: 1,
                end: 2,
                step: Some(1),
                limit: None,
                timeout: None,
            })
            .await
            .unwrap();
    }
}
