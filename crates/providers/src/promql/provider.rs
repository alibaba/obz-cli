//! Shared [`MetricProvider`] implementation for PromQL-compatible backends.
//!
//! VictoriaMetrics, Prometheus, and Grafana Mimir all expose the standard
//! Prometheus HTTP API (`/api/v1/query`, `/api/v1/query_range`, etc.) with
//! identical request/response formats.  Rather than duplicating the six
//! `MetricProvider` methods in each provider module, this struct provides a
//! single implementation that any PromQL-compatible backend can reuse.
//!
//! # Usage
//!
//! Each backend creates a `PromqlMetricProvider` by providing:
//! - An HTTP client and base URL
//! - Authentication credentials (bearer/basic)
//! - An optional API path prefix (default: none)
//! - Optional extra headers (e.g. `X-Scope-OrgID` for Grafana multi-tenancy)
//! - A provider display name for error messages

use std::collections::BTreeMap;

use async_trait::async_trait;
use reqwest::Client;

use crate::util::{apply_standard_auth, send_and_parse_json};
use obz_core::model::metric::MetricInfoDetail;
use obz_core::provider::{
    LabelValuesParams, MetricInfoParams, MetricMetadataParams, MetricProvider, MetricQueryParams,
    MetricQueryResult, ProviderResult,
};

use super::convert;
use super::response::{PromqlMetadataEntry, PromqlQueryData, PromqlResponse};

/// Shared `MetricProvider` for backends with a PromQL-compatible HTTP API.
///
/// All six `MetricProvider` methods are implemented once here:
/// `query`, `list`, `info`, `labels`, `label_values`, `series`.
#[derive(Clone)]
pub(crate) struct PromqlMetricProvider {
    /// Base URL of the backend instance (e.g. `http://localhost:9090`).
    base_url: String,
    /// HTTP client with connection pooling.
    client: Client,
    /// Optional bearer token for authentication.
    bearer_token: Option<String>,
    /// Optional basic auth credentials.
    basic_auth: Option<(String, String)>,
    /// Path prefix prepended to all standard `PromQL` API paths.
    ///
    /// Empty string for standard Prometheus/VM endpoints.
    /// Example for SLS: `"/prometheus/{project}/{metricstore}"`.
    path_prefix: String,
    /// Extra headers applied to every request.
    ///
    /// Used for Grafana multi-tenancy (`X-Scope-OrgID`).
    extra_headers: Vec<(String, String)>,
    /// Provider display name for error messages (e.g. `"VictoriaMetrics"`).
    provider_name: &'static str,
    /// Whether to print HTTP request/response details to stderr.
    verbose: bool,
}

impl PromqlMetricProvider {
    /// Create a new `PromqlMetricProvider`.
    ///
    /// # Arguments
    ///
    /// * `base_url` — Base URL of the backend (without trailing slash).
    /// * `client` — HTTP client (shared via `Arc` when cloned).
    /// * `bearer_token` — Optional bearer token.
    /// * `basic_auth` — Optional `(username, password)`.
    /// * `path_prefix` — API path prefix (empty for standard Prometheus).
    /// * `extra_headers` — Additional headers for every request.
    /// * `provider_name` — Display name for error messages.
    /// * `verbose` — Whether to print HTTP request/response details.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        base_url: &str,
        client: Client,
        bearer_token: Option<String>,
        basic_auth: Option<(String, String)>,
        path_prefix: String,
        extra_headers: Vec<(String, String)>,
        provider_name: &'static str,
        verbose: bool,
    ) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            client,
            bearer_token,
            basic_auth,
            path_prefix,
            extra_headers,
            provider_name,
            verbose,
        }
    }

    /// Build a GET request with authentication and extra headers applied.
    fn get(&self, path: &str) -> reqwest::RequestBuilder {
        let url = format!("{}{}{path}", self.base_url, self.path_prefix);
        let mut req = self.client.get(&url);
        req = apply_standard_auth(req, &self.bearer_token, &self.basic_auth);
        for (name, value) in &self.extra_headers {
            req = req.header(name.as_str(), value.as_str());
        }
        req
    }

    /// Send a request and deserialize the JSON response.
    async fn send_json<T: serde::de::DeserializeOwned>(
        &self,
        req: reqwest::RequestBuilder,
    ) -> ProviderResult<T> {
        send_and_parse_json(&self.client, req, self.provider_name, self.verbose).await
    }

    fn build_query_request(&self, params: &MetricQueryParams) -> reqwest::RequestBuilder {
        if params.is_range {
            let step = params.step_or_auto();
            let mut req = self.get("/api/v1/query_range");
            req = req.query(&[
                ("query", params.query.as_str()),
                ("start", &params.start.to_string()),
                ("end", &params.end.to_string()),
                ("step", &step.to_string()),
            ]);
            if let Some(timeout) = &params.timeout {
                req = req.query(&[("timeout", &format!("{}ms", timeout.as_millis()))]);
            }
            req
        } else {
            let mut req = self.get("/api/v1/query");
            req = req.query(&[("query", params.query.as_str())]);
            req = req.query(&[("time", &params.end.to_string())]);
            if let Some(timeout) = &params.timeout {
                req = req.query(&[("timeout", &format!("{}ms", timeout.as_millis()))]);
            }
            req
        }
    }
}

#[async_trait]
impl MetricProvider for PromqlMetricProvider {
    async fn query(&self, params: &MetricQueryParams) -> ProviderResult<MetricQueryResult> {
        let req = self.build_query_request(params);
        let resp: PromqlResponse<PromqlQueryData> = self.send_json(req).await?;

        convert::convert_query_response(resp)
    }

    async fn list(&self, params: &MetricMetadataParams) -> ProviderResult<Vec<String>> {
        let mut req = self.get("/api/v1/label/__name__/values");
        if let Some(start) = params.start {
            req = req.query(&[("start", &start.to_string())]);
        }
        if let Some(end) = params.end {
            req = req.query(&[("end", &end.to_string())]);
        }
        if let Some(limit) = params.limit {
            req = req.query(&[("limit", &limit.to_string())]);
        }
        if let Some(match_expr) = &params.match_expr {
            req = req.query(&[("match[]", match_expr.as_str())]);
        }

        let resp: PromqlResponse<Vec<String>> = self.send_json(req).await?;
        convert::convert_string_list_response(resp)
    }

    async fn info(&self, params: &MetricInfoParams) -> ProviderResult<Vec<MetricInfoDetail>> {
        let req = self
            .get("/api/v1/metadata")
            .query(&[("metric", &params.metric_name)]);

        let resp: PromqlResponse<BTreeMap<String, Vec<PromqlMetadataEntry>>> =
            self.send_json(req).await?;
        convert::convert_metadata_response(resp, Some(&params.metric_name))
    }

    async fn labels(&self, params: &MetricMetadataParams) -> ProviderResult<Vec<String>> {
        let mut req = self.get("/api/v1/labels");
        if let Some(start) = params.start {
            req = req.query(&[("start", &start.to_string())]);
        }
        if let Some(end) = params.end {
            req = req.query(&[("end", &end.to_string())]);
        }
        if let Some(limit) = params.limit {
            req = req.query(&[("limit", &limit.to_string())]);
        }
        if let Some(match_expr) = &params.match_expr {
            req = req.query(&[("match[]", match_expr.as_str())]);
        }
        for m in &params.match_exprs {
            req = req.query(&[("match[]", m.as_str())]);
        }

        let resp: PromqlResponse<Vec<String>> = self.send_json(req).await?;
        convert::convert_string_list_response(resp)
    }

    async fn label_values(&self, params: &LabelValuesParams) -> ProviderResult<Vec<String>> {
        let encoded_name = urlencoding::encode(&params.label_name);
        let path = format!("/api/v1/label/{encoded_name}/values");
        let mut req = self.get(&path);
        if let Some(start) = params.start {
            req = req.query(&[("start", &start.to_string())]);
        }
        if let Some(end) = params.end {
            req = req.query(&[("end", &end.to_string())]);
        }
        if let Some(limit) = params.limit {
            req = req.query(&[("limit", &limit.to_string())]);
        }
        if let Some(match_expr) = &params.match_expr {
            req = req.query(&[("match[]", match_expr.as_str())]);
        }

        let resp: PromqlResponse<Vec<String>> = self.send_json(req).await?;
        convert::convert_string_list_response(resp)
    }

    async fn series(
        &self,
        params: &MetricMetadataParams,
    ) -> ProviderResult<Vec<BTreeMap<String, String>>> {
        let mut req = self.get("/api/v1/series");
        if let Some(start) = params.start {
            req = req.query(&[("start", &start.to_string())]);
        }
        if let Some(end) = params.end {
            req = req.query(&[("end", &end.to_string())]);
        }
        if let Some(match_expr) = &params.match_expr {
            req = req.query(&[("match[]", match_expr.as_str())]);
        }
        for m in &params.match_exprs {
            req = req.query(&[("match[]", m.as_str())]);
        }
        if let Some(limit) = params.limit {
            req = req.query(&[("limit", &limit.to_string())]);
        }

        let resp: PromqlResponse<Vec<BTreeMap<String, String>>> = self.send_json(req).await?;
        convert::convert_series_response(resp)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_provider() -> PromqlMetricProvider {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let client = crate::util::build_http_client(None).expect("HTTP client should build");
        PromqlMetricProvider::new(
            "http://example.com",
            client,
            None,
            None,
            String::new(),
            vec![],
            "Test",
            false,
        )
    }

    fn test_params(is_range: bool, step: Option<u64>) -> MetricQueryParams {
        MetricQueryParams {
            query: "up".to_string(),
            is_range,
            start: 1_700_000_000,
            end: 1_700_003_600,
            step,
            limit: None,
            timeout: None,
        }
    }

    #[test]
    fn range_query_without_step_uses_auto_step() {
        let provider = test_provider();
        let params = test_params(true, None);
        let expected_step = params.step_or_auto();
        let request = provider
            .build_query_request(&params)
            .build()
            .expect("request should build");

        assert_eq!(request.url().path(), "/api/v1/query_range");
        let expected_query =
            format!("query=up&start=1700000000&end=1700003600&step={expected_step}");
        assert_eq!(request.url().query(), Some(expected_query.as_str()));
    }

    #[test]
    fn instant_query_ignores_range_window_when_not_requested() {
        let provider = test_provider();
        let request = provider
            .build_query_request(&test_params(false, None))
            .build()
            .expect("request should build");

        assert_eq!(request.url().path(), "/api/v1/query");
        assert_eq!(request.url().query(), Some("query=up&time=1700003600"));
    }
}
