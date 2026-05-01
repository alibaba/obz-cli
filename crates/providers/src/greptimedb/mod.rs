//! `GreptimeDB` metric provider.
//!
//! `GreptimeDB` exposes a Prometheus-compatible HTTP API under the
//! `/v1/prometheus` path prefix. This module reuses the shared
//! [`PromqlMetricProvider`] for query, list, labels, label-values, and
//! series commands.
//!
//! # Default database
//!
//! `GreptimeDB` routes queries to the `public` database by default.
//! To target a different database, pass the `db` query parameter in
//! your `PromQL` request. The default database is `public`, which suits
//! most deployments.
//!
//! # API Endpoints
//!
//! | Command                | Endpoint                                          |
//! |------------------------|---------------------------------------------------|
//! | `metric query` (instant)  | `GET /v1/prometheus/api/v1/query`              |
//! | `metric query` (range)    | `GET /v1/prometheus/api/v1/query_range`        |
//! | `metric list`             | `GET /v1/prometheus/api/v1/label/__name__/values` |
//! | `metric labels`           | `GET /v1/prometheus/api/v1/labels`             |
//! | `metric label-values`     | `GET /v1/prometheus/api/v1/label/{name}/values` |
//! | `metric series`           | `GET /v1/prometheus/api/v1/series`             |

/// `GreptimeDB` → obz model conversion functions (re-exported from `promql`).
pub(crate) mod convert;
/// `GreptimeDB` API response deserialization types (re-exported from `promql`).
pub(crate) mod response;

use std::collections::BTreeMap;

use async_trait::async_trait;

use obz_core::model::error::ObzError;
use obz_core::model::metric::MetricInfoDetail;
use obz_core::provider::{
    LabelValuesParams, MetricInfoParams, MetricMetadataParams, MetricProvider, MetricQueryParams,
    MetricQueryResult, ProviderResult,
};
use obz_core::registry::{BuiltProvider, ProviderMeta, SupportedCommands};

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

    async fn list(&self, params: &MetricMetadataParams) -> ProviderResult<Vec<String>> {
        self.inner.list(params).await
    }

    async fn info(&self, _params: &MetricInfoParams) -> ProviderResult<Vec<MetricInfoDetail>> {
        Err(ObzError::Unsupported {
            message: "GreptimeDB does not support metric info (metadata) queries".to_string(),
            provider: Some("greptimedb".to_string()),
            suggestion: Some(
                "Use metric list, metric labels, metric label-values, or metric series instead."
                    .to_string(),
            ),
        })
    }

    async fn labels(&self, params: &MetricMetadataParams) -> ProviderResult<Vec<String>> {
        self.inner.labels(params).await
    }

    async fn label_values(&self, params: &LabelValuesParams) -> ProviderResult<Vec<String>> {
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
    validate_endpoint(endpoint)?;
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
        ),
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
            metric_list: true,
            metric_info: false,
            metric_labels: true,
            metric_label_values: true,
            metric_series: true,
            log_search: false,
            trace_search: false,
            trace_get: false,
        },
        build,
        check: Some(|config| Box::pin(crate::probe::http_get_probe(config, "/v1/health"))),
        command_flags: &[],
        extension_commands: &[],
    }
}

#[cfg(test)]
mod tests {
    use obz_core::provider::{MetricInfoParams, ProviderConfig};

    use super::*;

    #[test]
    fn metadata_declares_metric_info_unsupported() {
        assert!(!meta().supported_commands.metric_info);
    }

    #[tokio::test]
    async fn metric_info_returns_unsupported_without_backend_request() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let mut config = ProviderConfig::new();
        config.set("endpoint", "http://localhost:4000");

        let provider = build(&config).unwrap();
        let metric = provider.metric.unwrap();
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
}
