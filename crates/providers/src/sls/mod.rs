//! SLS (Alibaba Cloud Log Service) provider.
//!
//! Implements [`MetricProvider`], [`LogProvider`], and [`TraceProvider`]
//! traits against the SLS APIs:
//!
//! - **Metrics**: PromQL-compatible API at `/prometheus/{project}/{metricstore}/api/v1/...`
//!   using Basic Auth (`access-key-id`:`access-key-secret`).
//! - **Logs**: Native SLS `GetLogs` API at `/logstores/{logstore}` using V1 HMAC-SHA1 signing.
//! - **Traces**: Same `GetLogs` API against a trace-dedicated logstore.
//!
//! # API Endpoints
//!
//! | obz Command | Endpoint | Auth |
//! |-------------|----------|------|
//! | `metric query` (instant) | `GET /prometheus/{project}/{metricstore}/api/v1/query` | Basic |
//! | `metric query` (range) | `GET /prometheus/{project}/{metricstore}/api/v1/query_range` | Basic |
//! | `metric list` | `GET /prometheus/{project}/{metricstore}/api/v1/label/__name__/values` | Basic |
//! | `metric labels` | `GET /prometheus/{project}/{metricstore}/api/v1/labels` | Basic |
//! | `metric label-values` | `GET /prometheus/{project}/{metricstore}/api/v1/label/{name}/values` | Basic |
//! | `metric series` | `GET /prometheus/{project}/{metricstore}/api/v1/series` | Basic |
//! | `log search` | `GET /logstores/{logstore}` | HMAC-SHA1 |
//! | `trace search` | `GET /logstores/{logstore}` | HMAC-SHA1 |
//! | `trace get` | `GET /logstores/{logstore}` | HMAC-SHA1 |

pub(crate) mod convert;
pub(crate) mod response;
pub(crate) mod sign;

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use http::Method;
use reqwest::Client;

use crate::promql::response::{PromqlQueryData, PromqlResponse};
use crate::util::{
    apply_custom_headers, build_http_client, classify_reqwest_error, http_error,
    parse_timeout_config, send_and_parse_json, validate_endpoint,
};
use obz_core::descriptor::{FlagDescriptor, FlagType};
use obz_core::model::error::{ErrorCode, ObzError};
use obz_core::model::metric::MetricInfoDetail;
use obz_core::model::trace::TraceDetail;
use obz_core::provider::results::{ProviderResult, TraceSearchResult};
use obz_core::provider::traits::{LogProvider, MetricProvider, TraceProvider};
use obz_core::provider::{
    LabelValuesParams, LogSearchParams, LogSearchResult, MetricInfoParams, MetricMetadataParams,
    MetricQueryParams, MetricQueryResult, TraceGetParams, TraceSearchParams,
};
use obz_core::registry::{
    BuiltProvider, CheckResult, CheckScope, CheckSeverity, ProviderMeta, SupportedCommands,
};
use obz_core::StandardCommand;

/// Maximum number of retries for incomplete SLS log/trace queries.
const MAX_RETRY_COUNT: u32 = 10;

/// Maximum number of spans to fetch for a single trace in `trace get`.
///
/// A single distributed trace rarely exceeds 1000 spans. SLS `GetLogs`
/// returns at most `line` entries per request, so this serves as a
/// reasonable upper bound without requiring pagination.
const TRACE_GET_MAX_SPANS: usize = 1000;

/// SLS metric provider (PromQL-compatible API).
///
/// Handles metric queries via the `PromQL` gateway at
/// `/prometheus/{project}/{metricstore}/api/v1/...`.
/// Uses Basic Auth with `access-key-id` as username and `access-key-secret` as password.
pub(crate) struct SlsMetricProvider {
    /// Base URL of the SLS endpoint (e.g. `https://cn-hangzhou.log.aliyuncs.com`).
    base_url: String,
    /// HTTP client with connection pooling.
    client: Client,
    /// Basic auth credentials: (`access-key-id`, `access-key-secret`).
    basic_auth: (String, String),
    /// SLS project name.
    project: String,
    /// SLS metricstore name.
    metricstore: String,
    /// Custom HTTP headers from provider config.
    custom_headers: BTreeMap<String, String>,
    /// Whether to print HTTP request/response details to stderr.
    verbose: bool,
}

impl SlsMetricProvider {
    /// Build the `PromQL` API path prefix for this provider.
    fn promql_prefix(&self) -> String {
        format!("/prometheus/{}/{}/api/v1", self.project, self.metricstore)
    }

    /// Build the full base URL with project-prefixed domain.
    ///
    /// SLS requires the domain to be `{project}.{region}.log.aliyuncs.com`.
    fn project_base_url(&self) -> String {
        let raw = self
            .base_url
            .trim_start_matches("https://")
            .trim_start_matches("http://");
        let scheme = if self.base_url.starts_with("https://") {
            "https"
        } else {
            "http"
        };
        format!("{scheme}://{}.{raw}", self.project)
    }

    /// Build a GET request with Basic Auth and custom headers.
    ///
    /// # Errors
    ///
    /// Returns an error if a reserved header is used in custom headers.
    fn get(&self, path: &str) -> Result<reqwest::RequestBuilder, ObzError> {
        let url = format!("{}{path}", self.project_base_url());
        let req = self
            .client
            .get(&url)
            .basic_auth(&self.basic_auth.0, Some(&self.basic_auth.1));
        apply_custom_headers(req, &self.custom_headers, &[], self.verbose)
    }

    /// Send a request and deserialize the JSON response.
    async fn send_json<T: serde::de::DeserializeOwned>(
        &self,
        req: reqwest::RequestBuilder,
    ) -> ProviderResult<T> {
        send_and_parse_json(&self.client, req, "SLS", self.verbose).await
    }

    fn build_query_request(
        &self,
        params: &MetricQueryParams,
    ) -> Result<reqwest::RequestBuilder, ObzError> {
        if params.is_range {
            let step = params.step_or_auto();
            let mut req = self.get(&format!("{}/query_range", self.promql_prefix()))?;
            req = req.query(&[
                ("query", params.query.as_str()),
                ("start", &params.start.to_string()),
                ("end", &params.end.to_string()),
                ("step", &step.to_string()),
            ]);
            if let Some(timeout) = &params.timeout {
                req = req.query(&[("timeout", &format!("{}ms", timeout.as_millis()))]);
            }
            Ok(req)
        } else {
            let mut req = self.get(&format!("{}/query", self.promql_prefix()))?;
            req = req.query(&[("query", params.query.as_str())]);
            req = req.query(&[("time", &params.end.to_string())]);
            if let Some(timeout) = &params.timeout {
                req = req.query(&[("timeout", &format!("{}ms", timeout.as_millis()))]);
            }
            Ok(req)
        }
    }
}

#[async_trait]
impl MetricProvider for SlsMetricProvider {
    async fn query(&self, params: &MetricQueryParams) -> ProviderResult<MetricQueryResult> {
        let req = self.build_query_request(params)?;
        let resp: PromqlResponse<PromqlQueryData> = self.send_json(req).await?;

        crate::promql::convert::convert_query_response(resp)
    }

    async fn list(&self, params: &MetricMetadataParams) -> ProviderResult<Vec<String>> {
        let mut req = self.get(&format!("{}/label/__name__/values", self.promql_prefix()))?;
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
        crate::promql::convert::convert_string_list_response(resp)
    }

    async fn info(&self, _params: &MetricInfoParams) -> ProviderResult<Vec<MetricInfoDetail>> {
        Err(ObzError::Unsupported {
            message: "SLS does not support metric info (metadata) queries".to_string(),
            provider: Some("sls".to_string()),
            suggestion: None,
        })
    }

    async fn labels(&self, params: &MetricMetadataParams) -> ProviderResult<Vec<String>> {
        let mut req = self.get(&format!("{}/labels", self.promql_prefix()))?;
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
        crate::promql::convert::convert_string_list_response(resp)
    }

    async fn label_values(&self, params: &LabelValuesParams) -> ProviderResult<Vec<String>> {
        let encoded_name = urlencoding::encode(&params.label_name);
        let path = format!("{}/label/{encoded_name}/values", self.promql_prefix());
        let mut req = self.get(&path)?;
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
        crate::promql::convert::convert_string_list_response(resp)
    }

    async fn series(
        &self,
        params: &MetricMetadataParams,
    ) -> ProviderResult<Vec<BTreeMap<String, String>>> {
        let mut req = self.get(&format!("{}/series", self.promql_prefix()))?;
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
        crate::promql::convert::convert_series_response(resp)
    }
}

// ---------------------------------------------------------------------------
// SLS Log/Trace Provider
// ---------------------------------------------------------------------------

/// SLS log and trace provider (native `GetLogs` API).
///
/// Handles log and trace queries via the SLS `GetLogs` API at
/// `/logstores/{logstore}`. Uses HMAC-SHA1 request signing.
pub(crate) struct SlsLogTraceProvider {
    /// Base URL of the SLS endpoint.
    base_url: String,
    /// HTTP client with connection pooling.
    client: Client,
    /// `AccessKey` ID for request signing.
    access_key_id: String,
    /// `AccessKey` Secret for request signing.
    access_key_secret: String,
    /// SLS project name.
    project: String,
    /// Logstore name for log queries.
    logstore: String,
    /// Custom HTTP headers from provider config.
    custom_headers: BTreeMap<String, String>,
    /// Whether to print HTTP request/response details to stderr.
    verbose: bool,
}

impl SlsLogTraceProvider {
    /// Send a signed SLS `GetLogs` request and return the response body and progress status.
    ///
    /// Automatically retries if `x-log-progress: Incomplete` is returned,
    /// up to `MAX_RETRY_COUNT` times with exponential backoff.
    async fn get_logs(
        &self,
        logstore: &str,
        query: &str,
        start: i64,
        end: i64,
        limit: usize,
    ) -> ProviderResult<(String, bool)> {
        let path = format!("/logstores/{logstore}");
        let host = format!(
            "{}.{}",
            self.project,
            self.base_url
                .trim_start_matches("https://")
                .trim_start_matches("http://")
        );
        let base = if self.base_url.starts_with("https://") {
            format!("https://{host}")
        } else {
            format!("http://{host}")
        };

        let mut retry_count = 0;

        loop {
            let query_params: Vec<(&str, String)> = vec![
                ("type", "log".to_string()),
                ("from", start.to_string()),
                ("to", end.to_string()),
                ("query", query.to_string()),
                ("line", limit.to_string()),
            ];

            // Build headers and sign the request.
            let mut headers = http::HeaderMap::new();
            let sign_params: Vec<(&str, &str)> =
                query_params.iter().map(|(k, v)| (*k, v.as_str())).collect();

            sign::sign_request(
                &self.access_key_id,
                &self.access_key_secret,
                http::Method::GET,
                &path,
                &mut headers,
                sign_params.into(),
                None,
            )?;

            // Build the reqwest request with signed headers.
            let url = format!("{base}{path}");
            let mut req = self.client.get(&url);

            // Apply signed headers.
            for (name, value) in &headers {
                req = req.header(name.as_str(), value.as_bytes());
            }

            // Apply query params.
            for (k, v) in &query_params {
                req = req.query(&[(k, v)]);
            }

            // Set Host header for SLS (project.endpoint).
            req = req.header("Host", &host);

            // Apply custom headers after SLS auth headers.
            req = apply_custom_headers(req, &self.custom_headers, &[], self.verbose)?;

            let request = req.build().map_err(|e| classify_reqwest_error(&e))?;
            let method = request.method().clone();
            let url = request.url().clone();
            if self.verbose {
                eprintln!("[verbose] → {method} {url}");
            }

            let start_time = Instant::now();
            let resp = self
                .client
                .execute(request)
                .await
                .map_err(|e| classify_reqwest_error(&e))?;
            let status = resp.status();
            if self.verbose {
                eprintln!(
                    "[verbose] ← {status} ({}ms)",
                    start_time.elapsed().as_millis()
                );
            }

            // Extract progress header before consuming body.
            let progress = resp
                .headers()
                .get("x-log-progress")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("Complete")
                .to_string();

            let body = resp.text().await.map_err(|e| ObzError::Network {
                code: ErrorCode::ConnectionError,
                message: format!("failed to read response body: {e}"),
                recoverable: false,
                source_chain: Some(crate::util::collect_error_chain(&e)),
            })?;

            if !status.is_success() {
                return Err(http_error(status, &body, "SLS"));
            }

            let is_complete = progress == "Complete";

            if is_complete || retry_count >= MAX_RETRY_COUNT {
                return Ok((body, is_complete));
            }

            // Exponential backoff: 200ms, 400ms, 800ms, ...
            let delay = Duration::from_millis(200 * 2u64.pow(retry_count.min(4)));
            tokio::time::sleep(delay).await;
            retry_count += 1;
        }
    }
}

#[async_trait]
impl LogProvider for SlsLogTraceProvider {
    async fn search(&self, params: &LogSearchParams) -> ProviderResult<LogSearchResult> {
        let (body, is_complete) = self
            .get_logs(
                &self.logstore,
                &params.query,
                params.start,
                params.end,
                params.limit,
            )
            .await?;

        let entries = response::parse_sls_response(&body)?;
        Ok(convert::convert_log_entries(&entries, is_complete))
    }
}

#[async_trait]
impl TraceProvider for SlsLogTraceProvider {
    async fn search(&self, params: &TraceSearchParams) -> ProviderResult<TraceSearchResult> {
        let (body, is_complete) = self
            .get_logs(
                &self.logstore,
                &params.query,
                params.start,
                params.end,
                params.limit,
            )
            .await?;

        let entries = response::parse_sls_response(&body)?;
        Ok(convert::convert_trace_entries(&entries, is_complete))
    }

    async fn get_trace(&self, params: &TraceGetParams) -> ProviderResult<TraceDetail> {
        // Query by traceID in the trace logstore.
        let query = format!("traceID:{}", params.trace_id);
        let (body, _is_complete) = self
            .get_logs(
                &self.logstore,
                &query,
                params.start,
                params.end,
                TRACE_GET_MAX_SPANS,
            )
            .await?;

        let entries = response::parse_sls_response(&body)?;

        if entries.is_empty() {
            return Err(ObzError::Provider {
                code: ErrorCode::NotFound,
                message: format!("trace '{}' not found", params.trace_id),
                raw_error: None,
                recoverable: false,
                suggestion: Some("Check the trace ID and time range (--from / --to)".to_string()),
                doc_url: None,
            });
        }

        Ok(convert::convert_trace_detail(&entries, &params.trace_id))
    }
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

const SLS_FLAG_PROJECT: FlagDescriptor = FlagDescriptor {
    name: "project",
    flag_type: FlagType::String,
    required: true,
    default: None,
    description: "SLS project name",
    repeatable: false,
    short: None,
};

static METRIC_FLAGS: &[FlagDescriptor] = &[
    SLS_FLAG_PROJECT,
    FlagDescriptor {
        name: "metricstore",
        flag_type: FlagType::String,
        required: true,
        default: None,
        description: "SLS metricstore name",
        repeatable: false,
        short: None,
    },
];

static LOG_FLAGS: &[FlagDescriptor] = &[
    SLS_FLAG_PROJECT,
    FlagDescriptor {
        name: "logstore",
        flag_type: FlagType::String,
        required: true,
        default: None,
        description: "SLS logstore name",
        repeatable: false,
        short: None,
    },
];

static TRACE_FLAGS: &[FlagDescriptor] = &[
    SLS_FLAG_PROJECT,
    FlagDescriptor {
        name: "trace-logstore",
        flag_type: FlagType::String,
        required: true,
        default: None,
        description: "SLS trace logstore name",
        repeatable: false,
        short: None,
    },
];

/// Health check for the SLS provider.
///
/// When `project` is configured, sends a signed `GET /logstores` request
/// to `{project}.{endpoint}`. Otherwise, sends a signed `GET /` to list
/// projects.
async fn sls_check(config: &obz_core::provider::ProviderConfig) -> CheckResult {
    let endpoint = match config.get("endpoint") {
        Some(endpoint) => endpoint.trim_end_matches('/'),
        None => {
            return CheckResult {
                severity: CheckSeverity::Fail,
                message: "endpoint not configured".to_string(),
                scope: CheckScope::Connectivity,
                latency: None,
            };
        }
    };
    let Some(access_key_id) = config.auth_get("access-key-id") else {
        return CheckResult {
            severity: CheckSeverity::Fail,
            message: "access-key-id not configured".to_string(),
            scope: CheckScope::ConnectivityAndAuth,
            latency: None,
        };
    };
    let Some(access_key_secret) = config.auth_get("access-key-secret") else {
        return CheckResult {
            severity: CheckSeverity::Fail,
            message: "access-key-secret not configured".to_string(),
            scope: CheckScope::ConnectivityAndAuth,
            latency: None,
        };
    };

    let client = match build_http_client(Some(Duration::from_secs(5))) {
        Ok(client) => client,
        Err(error) => {
            return CheckResult {
                severity: CheckSeverity::Fail,
                message: error.to_string(),
                scope: CheckScope::Connectivity,
                latency: None,
            };
        }
    };

    let raw_domain = endpoint
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    let host = config
        .get("project")
        .map(|project| format!("{project}.{raw_domain}"));

    let (path, url) = if config.get("project").is_some() {
        let host = host
            .as_deref()
            .unwrap_or_else(|| unreachable!("project implies host is present"));
        let scheme = if endpoint.starts_with("https://") {
            "https"
        } else {
            "http"
        };
        (
            "/logstores".to_string(),
            format!("{scheme}://{host}/logstores"),
        )
    } else {
        ("/".to_string(), endpoint.to_string())
    };

    let mut headers = http::HeaderMap::new();
    if sign::sign_request(
        access_key_id,
        access_key_secret,
        Method::GET,
        &path,
        &mut headers,
        Vec::<(&str, &str)>::new().into(),
        None,
    )
    .is_err()
    {
        return CheckResult {
            severity: CheckSeverity::Fail,
            message: "failed to sign SLS request".to_string(),
            scope: CheckScope::ConnectivityAndAuth,
            latency: None,
        };
    }

    let mut request = client.get(&url);
    for (name, value) in &headers {
        request = request.header(name.as_str(), value.as_bytes());
    }
    if let Some(host) = &host {
        request = request.header("Host", host);
    }
    request = match apply_custom_headers(request, config.custom_headers(), &["host"], false) {
        Ok(request) => request,
        Err(error) => {
            return CheckResult {
                severity: CheckSeverity::Fail,
                message: error.to_string(),
                scope: CheckScope::Connectivity,
                latency: None,
            };
        }
    };

    let start = Instant::now();

    match request.send().await {
        Ok(response) => {
            let latency = Some(start.elapsed());
            let status = response.status();
            if status.is_success() {
                CheckResult {
                    severity: CheckSeverity::Ok,
                    message: "SLS endpoint accessible".to_string(),
                    scope: CheckScope::ConnectivityAndAuth,
                    latency,
                }
            } else if status == reqwest::StatusCode::UNAUTHORIZED
                || status == reqwest::StatusCode::FORBIDDEN
            {
                CheckResult {
                    severity: CheckSeverity::Fail,
                    message: format!("authentication failed (HTTP {status})"),
                    scope: CheckScope::ConnectivityAndAuth,
                    latency,
                }
            } else {
                CheckResult {
                    severity: CheckSeverity::Fail,
                    message: format!("unexpected response (HTTP {status})"),
                    scope: CheckScope::ConnectivityAndAuth,
                    latency,
                }
            }
        }
        Err(error) => {
            let latency = Some(start.elapsed());
            let classified = classify_reqwest_error(&error);
            let message = if error.is_timeout() {
                "connection timed out after 5s".to_string()
            } else {
                classified.to_string()
            };

            CheckResult {
                severity: CheckSeverity::Fail,
                message,
                scope: CheckScope::Connectivity,
                latency,
            }
        }
    }
}

/// Factory function: build a [`BuiltProvider`] for SLS.
fn build(config: &obz_core::provider::ProviderConfig) -> Result<BuiltProvider, ObzError> {
    let endpoint = config.require_config("endpoint")?;
    validate_endpoint(endpoint)?;
    let base_url = endpoint.trim_end_matches('/').to_string();
    let access_key_id = config.auth_get_owned("access-key-id");
    let access_key_secret = config.auth_get_owned("access-key-secret");

    // Determine which capabilities to build based on available config.
    let project = config.get_owned("project");
    let metricstore = config.get_owned("metricstore");
    let logstore = config.get_owned("logstore");
    let trace_logstore = config.get_owned("trace-logstore");
    let verbose = config.verbose();
    let timeout = parse_timeout_config(config);

    // Shared HTTP client across all signal types.
    let client = build_http_client(timeout)?;

    // Resolve auth credentials (AccessKeyID, AccessKeySecret).
    let auth = match (&access_key_id, &access_key_secret) {
        (Some(u), Some(p)) => Some((u.clone(), p.clone())),
        _ => None,
    };

    // Build metric provider if project and metricstore are specified.
    let metric: Option<Box<dyn MetricProvider>> = if let (Some(ref proj), Some(ref ms)) =
        (&project, &metricstore)
    {
        let basic_auth = auth.clone().ok_or_else(|| ObzError::Auth {
            code: ErrorCode::AuthMissing,
            message: "SLS metric queries require access-key-id and access-key-secret in config.yaml auth section"
                .to_string(),
            recoverable: false,
            suggestion: Some(
                "Set access-key-id and access-key-secret under providers.<name>.auth in config.yaml"
                    .to_string(),
            ),
        })?;

        Some(Box::new(SlsMetricProvider {
            base_url: base_url.clone(),
            client: client.clone(),
            basic_auth,
            project: proj.clone(),
            metricstore: ms.clone(),
            custom_headers: config.custom_headers().clone(),
            verbose,
        }))
    } else {
        None
    };

    // Build log provider if project and logstore are specified.
    let mut log: Option<Box<dyn LogProvider>> = None;
    if let (Some(ref proj), Some(ref ls)) = (&project, &logstore) {
        let (ak_id, ak_secret) = auth.clone().ok_or_else(|| ObzError::Auth {
            code: ErrorCode::AuthMissing,
            message: "SLS log queries require access-key-id and access-key-secret in config.yaml auth section".to_string(),
            recoverable: false,
            suggestion: Some(
                "Set access-key-id and access-key-secret under providers.<name>.auth in config.yaml"
                    .to_string(),
            ),
        })?;

        log = Some(Box::new(SlsLogTraceProvider {
            base_url: base_url.clone(),
            client: client.clone(),
            access_key_id: ak_id,
            access_key_secret: ak_secret,
            project: proj.clone(),
            logstore: ls.clone(),
            custom_headers: config.custom_headers().clone(),
            verbose,
        }));
    }

    // Build trace provider if project and trace-logstore are specified.
    let mut trace: Option<Box<dyn TraceProvider>> = None;
    if let (Some(ref proj), Some(ref tls)) = (&project, &trace_logstore) {
        let (ak_id, ak_secret) = auth.ok_or_else(|| ObzError::Auth {
            code: ErrorCode::AuthMissing,
            message: "SLS trace queries require access-key-id and access-key-secret in config.yaml auth section"
                .to_string(),
            recoverable: false,
            suggestion: Some(
                "Set access-key-id and access-key-secret under providers.<name>.auth in config.yaml"
                    .to_string(),
            ),
        })?;

        trace = Some(Box::new(SlsLogTraceProvider {
            base_url: base_url.clone(),
            client,
            access_key_id: ak_id,
            access_key_secret: ak_secret,
            project: proj.clone(),
            logstore: tls.clone(),
            custom_headers: config.custom_headers().clone(),
            verbose,
        }));
    }

    Ok(BuiltProvider {
        name: "sls",
        metric_query_language: if metric.is_some() {
            Some("PromQL")
        } else {
            None
        },
        log_query_language: if log.is_some() {
            Some("SLS Query")
        } else {
            None
        },
        metric,
        log,
        trace,
        extension: None,
    })
}

/// Command flags mapping for SLS — all metric commands share the same flags.
static COMMAND_FLAGS: &[(StandardCommand, &[FlagDescriptor])] = &[
    (StandardCommand::MetricQuery, METRIC_FLAGS),
    (StandardCommand::MetricList, METRIC_FLAGS),
    (StandardCommand::MetricLabels, METRIC_FLAGS),
    (StandardCommand::MetricLabelValues, METRIC_FLAGS),
    (StandardCommand::MetricSeries, METRIC_FLAGS),
    (StandardCommand::LogSearch, LOG_FLAGS),
    (StandardCommand::TraceSearch, TRACE_FLAGS),
    (StandardCommand::TraceGet, TRACE_FLAGS),
];

/// Return the [`ProviderMeta`] for SLS.
///
/// Called by [`obz_providers::register_all`] at startup.
pub(crate) fn meta() -> ProviderMeta {
    ProviderMeta {
        name: "sls",
        display_name: "SLS",
        aliases: &["sls"],
        supported_commands: SupportedCommands {
            metric_query: true,
            metric_list: true,
            metric_info: false, // SLS does not support /api/v1/metadata.
            metric_labels: true,
            metric_label_values: true,
            metric_series: true,
            log_search: true,
            trace_search: true,
            trace_get: true,
        },
        build,
        check: Some(|config| Box::pin(sls_check(config))),
        command_flags: COMMAND_FLAGS,
        extension_commands: &[],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use obz_core::model::error::ErrorCode;
    use obz_core::provider::ProviderConfig;

    fn init_crypto() {
        let _ = rustls::crypto::ring::default_provider().install_default();
    }

    #[test]
    fn build_with_new_auth_fields_succeeds() {
        init_crypto();
        let mut config = ProviderConfig::new();
        config.set("endpoint", "https://cn-hangzhou.log.aliyuncs.com");
        config.set_auth("access-key-id", "test-ak-id");
        config.set_auth("access-key-secret", "test-ak-secret");
        config.set("project", "test-project");
        config.set("logstore", "test-logstore");
        match build(&config) {
            Ok(built) => assert!(built.log.is_some()),
            Err(e) => panic!("expected Ok, got {e:?}"),
        }
    }

    #[test]
    fn build_old_username_password_no_longer_works() {
        init_crypto();
        let mut config = ProviderConfig::new();
        config.set("endpoint", "https://cn-hangzhou.log.aliyuncs.com");
        // Old field names should NOT be recognized for auth.
        config.set_auth("username", "test-ak-id");
        config.set_auth("password", "test-ak-secret");
        config.set("project", "test-project");
        config.set("logstore", "test-logstore");
        match build(&config) {
            Err(ObzError::Auth { code, .. }) => {
                assert_eq!(code, ErrorCode::AuthMissing);
            }
            Ok(_) => panic!("expected AuthMissing error, but build succeeded"),
            Err(e) => panic!("expected Auth error, got {e:?}"),
        }
    }

    fn test_metric_params(is_range: bool, step: Option<u64>) -> MetricQueryParams {
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

    fn test_metric_provider() -> SlsMetricProvider {
        init_crypto();
        let client = crate::util::build_http_client(None).expect("HTTP client should build");
        SlsMetricProvider {
            base_url: "https://cn-hangzhou.log.aliyuncs.com".to_string(),
            client,
            basic_auth: ("test-ak-id".to_string(), "test-ak-secret".to_string()),
            project: "test-project".to_string(),
            metricstore: "test-store".to_string(),
            custom_headers: BTreeMap::new(),
            verbose: false,
        }
    }

    #[test]
    fn metric_range_query_without_step_uses_auto_step() {
        let provider = test_metric_provider();
        let params = test_metric_params(true, None);
        let expected_step = params.step_or_auto();
        let request = provider
            .build_query_request(&params)
            .expect("build_query_request should succeed")
            .build()
            .expect("request should build");

        assert_eq!(
            request.url().path(),
            "/prometheus/test-project/test-store/api/v1/query_range"
        );
        let expected_query =
            format!("query=up&start=1700000000&end=1700003600&step={expected_step}");
        assert_eq!(request.url().query(), Some(expected_query.as_str()));
    }

    #[test]
    fn metric_instant_query_uses_time_parameter_only() {
        let provider = test_metric_provider();
        let request = provider
            .build_query_request(&test_metric_params(false, None))
            .expect("build_query_request should succeed")
            .build()
            .expect("request should build");

        assert_eq!(
            request.url().path(),
            "/prometheus/test-project/test-store/api/v1/query"
        );
        assert_eq!(request.url().query(), Some("query=up&time=1700003600"));
    }
}
