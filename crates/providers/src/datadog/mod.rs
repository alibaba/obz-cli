//! Datadog provider for metrics, logs, and traces.
//!
//! Implements [`MetricProvider`], [`LogProvider`], and [`TraceProvider`]
//! traits against the Datadog REST API.
//!
//! # API Endpoints
//!
//! | obz Command        | Endpoint                              | Method |
//! |--------------------|---------------------------------------|--------|
//! | `metric query`     | `/api/v1/query`                       | GET    |
//! | `metric list`      | `/api/v1/search?q=metrics:{prefix}`   | GET    |
//! | `metric info`      | `/api/v1/metrics/{name}`              | GET    |
//! | `log search`       | `/api/v2/logs/events/search`          | POST   |
//! | `trace search`     | `/api/v2/spans/events/search`         | POST   |
//! | `trace get`        | `/api/v2/spans/events/search`         | POST   |
//!
//! # Authentication
//!
//! All requests use `DD-API-KEY` and `DD-APPLICATION-KEY` HTTP headers.

/// Datadog response → obz model conversion functions.
pub(crate) mod convert;
/// Datadog API response deserialization types.
pub(crate) mod response;

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use jiff::Timestamp;
use reqwest::Client;

use crate::util::{
    apply_custom_headers, build_http_client, classify_reqwest_error, http_error,
    parse_timeout_config, send_request, truncate_for_error, validate_endpoint, HttpResponse,
};
use obz_core::descriptor::{CommandDescriptor, FlagDescriptor, FlagType};
use obz_core::model::error::{ErrorCode, ObzError};
use obz_core::model::metric::MetricInfoDetail;
use obz_core::model::trace::TraceDetail;
use obz_core::provider::params::ExtensionParams;
use obz_core::provider::results::{
    ExtensionResult, LogSearchResult, MetricQueryResult, ProviderResult, TraceSearchResult,
};
use obz_core::provider::traits::{ExtensionProvider, LogProvider, MetricProvider, TraceProvider};
use obz_core::provider::{
    LabelValuesParams, LogSearchParams, MetricInfoParams, MetricMetadataParams, MetricQueryParams,
    TraceGetParams, TraceSearchParams,
};
use obz_core::registry::{
    BuiltProvider, CheckResult, CheckScope, CheckSeverity, ProviderMeta, SupportedCommands,
};

use self::response::{
    DdErrorResponse, DdLogsResponse, DdMetricMetadata, DdMetricQueryResponse, DdSearchResponse,
    DdSpansResponse,
};

/// Maximum number of spans to fetch when retrieving a single trace.
const TRACE_GET_LIMIT: usize = 1000;

/// Datadog provider supporting metrics, logs, and traces.
pub(crate) struct DatadogProvider {
    /// Base URL of the Datadog API (e.g. `https://api.datadoghq.com`).
    base_url: String,
    /// HTTP client with connection pooling.
    client: Client,
    /// Datadog API key (`DD-API-KEY` header).
    api_key: String,
    /// Datadog Application key (`DD-APPLICATION-KEY` header).
    app_key: String,
    /// Custom HTTP headers from provider config.
    custom_headers: std::collections::BTreeMap<String, String>,
    /// Whether to print HTTP request/response details to stderr.
    verbose: bool,
}

impl DatadogProvider {
    /// Create a new `DatadogProvider`.
    ///
    /// The `client` is passed in so that multiple provider instances can
    /// share the same connection pool (`reqwest::Client` is `Arc`-based).
    ///
    /// # Arguments
    ///
    /// * `base_url` — Base URL of the Datadog API (without trailing slash).
    /// * `client` — Shared HTTP client.
    /// * `api_key` — Datadog API key.
    /// * `app_key` — Datadog Application key.
    /// * `verbose` — Whether to print HTTP request/response details to stderr.
    pub(crate) fn new(
        base_url: &str,
        client: Client,
        api_key: String,
        app_key: String,
        custom_headers: std::collections::BTreeMap<String, String>,
        verbose: bool,
    ) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            client,
            api_key,
            app_key,
            custom_headers,
            verbose,
        }
    }

    /// Datadog-managed headers that users cannot override via custom headers.
    const PROVIDER_MANAGED: &[&str] = &["dd-api-key", "dd-application-key", "accept"];

    /// Build a GET request with Datadog authentication and custom headers.
    ///
    /// # Errors
    ///
    /// Returns an error if a reserved header is used in custom headers.
    fn get(&self, path: &str) -> Result<reqwest::RequestBuilder, ObzError> {
        let url = format!("{}{path}", self.base_url);
        let req = self
            .client
            .get(&url)
            .header("DD-API-KEY", &self.api_key)
            .header("DD-APPLICATION-KEY", &self.app_key)
            .header("Accept", "application/json");
        apply_custom_headers(
            req,
            &self.custom_headers,
            Self::PROVIDER_MANAGED,
            self.verbose,
        )
    }

    /// Build a POST request with Datadog authentication and custom headers.
    ///
    /// # Errors
    ///
    /// Returns an error if a reserved header is used in custom headers.
    fn post(&self, path: &str) -> Result<reqwest::RequestBuilder, ObzError> {
        let url = format!("{}{path}", self.base_url);
        let req = self
            .client
            .post(&url)
            .header("DD-API-KEY", &self.api_key)
            .header("DD-APPLICATION-KEY", &self.app_key)
            .header("Accept", "application/json");
        apply_custom_headers(
            req,
            &self.custom_headers,
            Self::PROVIDER_MANAGED,
            self.verbose,
        )
    }

    /// Send a request and deserialize the JSON response.
    ///
    /// On non-success HTTP status, attempts to parse the Datadog error
    /// format (`{"errors": [...]}`) for a better error message, then
    /// falls back to the generic `http_error()` helper.
    async fn send_json<T: serde::de::DeserializeOwned>(
        &self,
        req: reqwest::RequestBuilder,
    ) -> ProviderResult<T> {
        let HttpResponse { status, body } = send_request(&self.client, req, self.verbose).await?;

        if !status.is_success() {
            // Try to extract Datadog error message.
            if let Ok(dd_err) = serde_json::from_str::<DdErrorResponse>(&body) {
                let msg = dd_err.errors.join("; ");
                return Err(match status.as_u16() {
                    401 => ObzError::Auth {
                        code: ErrorCode::AuthMissing,
                        message: format!("Datadog returned HTTP 401: {msg}"),
                        recoverable: false,
                        suggestion: Some(
                            "Check api-key and app-key in config.yaml auth section".to_string(),
                        ),
                    },
                    403 => ObzError::Auth {
                        code: ErrorCode::AccessDenied,
                        message: format!("Datadog returned HTTP 403: {msg}"),
                        recoverable: false,
                        suggestion: Some(
                            "Check api-key and app-key permissions in config.yaml auth section"
                                .to_string(),
                        ),
                    },
                    _ => http_error(status, &body, "Datadog"),
                });
            }
            return Err(http_error(status, &body, "Datadog"));
        }

        serde_json::from_str::<T>(&body).map_err(|e| ObzError::Provider {
            code: ErrorCode::BackendError,
            message: format!("failed to parse Datadog response (HTTP {status}): {e}"),
            raw_error: Some(truncate_for_error(&body, 500)),
            recoverable: false,
            suggestion: None,
            doc_url: None,
        })
    }

    /// Parse a compute spec such as `count` or `avg:@duration`.
    fn parse_compute(value: &str) -> serde_json::Value {
        if let Some((aggregation, metric)) = value.split_once(':') {
            serde_json::json!({"aggregation": aggregation, "metric": metric})
        } else {
            serde_json::json!({"aggregation": value})
        }
    }

    /// Parse a group-by facet.
    ///
    /// The `limit` is currently fixed at 10 groups per facet. A future
    /// `--group-by-limit` flag may expose this to users.
    fn parse_group_by(value: &str) -> serde_json::Value {
        serde_json::json!({"facet": value, "limit": 10})
    }

    /// Build the common aggregate request attributes.
    fn build_aggregate_attributes(params: &ExtensionParams) -> ProviderResult<serde_json::Value> {
        let computes = params.get_all("compute");
        let group_bys = params.get_all("group-by");

        let compute = if computes.is_empty() {
            vec![serde_json::json!({"aggregation": "count"})]
        } else {
            computes.into_iter().map(Self::parse_compute).collect()
        };

        let group_by = group_bys
            .into_iter()
            .map(Self::parse_group_by)
            .collect::<Vec<_>>();

        let now = Timestamp::now().as_second();
        let start = params.start.unwrap_or(now - 3600);
        let end = params.end.unwrap_or(now);

        Ok(serde_json::json!({
            "filter": {
                "query": params.require("query")?,
                "from": format_unix_to_iso8601(start),
                "to": format_unix_to_iso8601(end),
            },
            "compute": compute,
            "group_by": group_by,
        }))
    }

    /// Execute log aggregate.
    async fn aggregate_logs(&self, params: &ExtensionParams) -> ProviderResult<ExtensionResult> {
        let body = Self::build_aggregate_attributes(params)?;
        let data: serde_json::Value = self
            .send_json(self.post("/api/v2/logs/analytics/aggregate")?.json(&body))
            .await?;
        let total_count = data
            .get("data")
            .and_then(|v| v.get("buckets"))
            .and_then(serde_json::Value::as_array)
            .map(std::vec::Vec::len);
        Ok(ExtensionResult { data, total_count })
    }

    /// Execute trace aggregate.
    async fn aggregate_traces(&self, params: &ExtensionParams) -> ProviderResult<ExtensionResult> {
        let body = serde_json::json!({
            "data": {
                "attributes": Self::build_aggregate_attributes(params)?,
                "type": "aggregate_request",
            }
        });
        let data: serde_json::Value = self
            .send_json(self.post("/api/v2/spans/analytics/aggregate")?.json(&body))
            .await?;
        let total_count = data
            .get("data")
            .and_then(|v| v.get("buckets"))
            .and_then(serde_json::Value::as_array)
            .map(std::vec::Vec::len);
        Ok(ExtensionResult { data, total_count })
    }
}

// ---------------------------------------------------------------------------
// MetricProvider
// ---------------------------------------------------------------------------

#[async_trait]
impl MetricProvider for DatadogProvider {
    async fn query(&self, params: &MetricQueryParams) -> ProviderResult<MetricQueryResult> {
        let req = self.get("/api/v1/query")?.query(&[
            ("from", &params.start.to_string()),
            ("to", &params.end.to_string()),
            ("query", &params.query),
        ]);

        let resp: DdMetricQueryResponse = self.send_json(req).await?;

        // Datadog returns HTTP 200 with status:"error" for invalid queries.
        if resp.status == "error" {
            let msg = resp
                .error
                .unwrap_or_else(|| "unknown query error".to_string());
            return Err(ObzError::Provider {
                code: ErrorCode::QuerySyntax,
                message: format!("Datadog query error: {msg}"),
                raw_error: None,
                recoverable: false,
                suggestion: Some("Check the query syntax".to_string()),
                doc_url: None,
            });
        }

        Ok(convert::convert_metric_query(resp))
    }

    async fn list(&self, params: &MetricMetadataParams) -> ProviderResult<Vec<String>> {
        // Use the Datadog search API: GET /api/v1/search?q=metrics:{prefix}
        // This API does not support server-side limit or pagination.
        let query = match &params.match_expr {
            Some(expr) => format!("metrics:{expr}"),
            None => "metrics:".to_string(),
        };

        let req = self.get("/api/v1/search")?.query(&[("q", &query)]);
        let resp: DdSearchResponse = self.send_json(req).await?;

        let mut items = convert::convert_metric_search(resp);
        if let Some(limit) = params.limit {
            items.truncate(limit);
        }
        Ok(items)
    }

    async fn info(&self, params: &MetricInfoParams) -> ProviderResult<Vec<MetricInfoDetail>> {
        let path = format!(
            "/api/v1/metrics/{}",
            urlencoding::encode(&params.metric_name)
        );
        let req = self.get(&path)?;
        let resp: DdMetricMetadata = self.send_json(req).await?;

        Ok(convert::convert_metric_metadata(&params.metric_name, resp))
    }

    async fn labels(&self, _params: &MetricMetadataParams) -> ProviderResult<Vec<String>> {
        Err(ObzError::Unsupported {
            message: "Datadog does not support listing metric labels".to_string(),
            provider: Some("datadog".to_string()),
            suggestion: None,
        })
    }

    async fn label_values(&self, _params: &LabelValuesParams) -> ProviderResult<Vec<String>> {
        Err(ObzError::Unsupported {
            message: "Datadog does not support listing label values".to_string(),
            provider: Some("datadog".to_string()),
            suggestion: None,
        })
    }

    async fn series(
        &self,
        _params: &MetricMetadataParams,
    ) -> ProviderResult<Vec<BTreeMap<String, String>>> {
        Err(ObzError::Unsupported {
            message: "Datadog does not support listing metric series".to_string(),
            provider: Some("datadog".to_string()),
            suggestion: None,
        })
    }
}

// ---------------------------------------------------------------------------
// LogProvider
// ---------------------------------------------------------------------------

#[async_trait]
impl LogProvider for DatadogProvider {
    async fn search(&self, params: &LogSearchParams) -> ProviderResult<LogSearchResult> {
        let body = serde_json::json!({
            "filter": {
                "from": format_unix_to_iso8601(params.start),
                "to": format_unix_to_iso8601(params.end),
                "query": params.query,
            },
            "sort": "timestamp",
            "page": {
                "limit": params.limit,
            }
        });

        let req = self.post("/api/v2/logs/events/search")?.json(&body);
        let resp: DdLogsResponse = self.send_json(req).await?;

        Ok(convert::convert_logs_search(resp))
    }
}

// ---------------------------------------------------------------------------
// TraceProvider
// ---------------------------------------------------------------------------

#[async_trait]
impl TraceProvider for DatadogProvider {
    async fn search(&self, params: &TraceSearchParams) -> ProviderResult<TraceSearchResult> {
        let body = serde_json::json!({
            "data": {
                "attributes": {
                    "filter": {
                        "from": format_unix_to_iso8601(params.start),
                        "to": format_unix_to_iso8601(params.end),
                        "query": params.query,
                    },
                    "sort": "timestamp",
                    "page": {
                        "limit": params.limit,
                    }
                },
                "type": "search_request"
            }
        });

        let req = self.post("/api/v2/spans/events/search")?.json(&body);
        let resp: DdSpansResponse = self.send_json(req).await?;

        Ok(convert::convert_spans_search(resp))
    }

    async fn get_trace(&self, params: &TraceGetParams) -> ProviderResult<TraceDetail> {
        // Datadog has no dedicated "get trace" endpoint. We use the spans
        // search API with a trace_id filter and a higher limit to retrieve
        // all spans belonging to this trace.
        let body = serde_json::json!({
            "data": {
                "attributes": {
                    "filter": {
                        "from": format_unix_to_iso8601(params.start),
                        "to": format_unix_to_iso8601(params.end),
                        "query": format!("trace_id:{}", params.trace_id),
                    },
                    "sort": "timestamp",
                    "page": {
                        "limit": TRACE_GET_LIMIT,
                    }
                },
                "type": "search_request"
            }
        });

        let req = self.post("/api/v2/spans/events/search")?.json(&body);
        let resp: DdSpansResponse = self.send_json(req).await?;

        if resp.data.is_empty() {
            return Err(ObzError::Provider {
                code: ErrorCode::NotFound,
                message: format!("trace '{}' not found", params.trace_id),
                raw_error: None,
                recoverable: false,
                suggestion: Some("Check the trace ID. The default time range is 1 hour; if the trace is older, use --from (e.g. --from now-6h)".to_string()),
                doc_url: None,
            });
        }

        Ok(convert::convert_trace_detail(&params.trace_id, &resp))
    }
}

#[async_trait]
impl ExtensionProvider for DatadogProvider {
    async fn execute(
        &self,
        command: &str,
        params: &ExtensionParams,
    ) -> ProviderResult<ExtensionResult> {
        match command {
            "aggregate" => match params.signal.as_str() {
                "log" => self.aggregate_logs(params).await,
                "trace" => self.aggregate_traces(params).await,
                _ => Err(ObzError::InvalidArgument {
                    code: ErrorCode::InvalidFlag,
                    message: format!(
                        "aggregate command is not supported under the '{}' signal group",
                        params.signal,
                    ),
                    suggestion: None,
                }),
            },
            _ => Err(ObzError::InvalidArgument {
                code: ErrorCode::InvalidFlag,
                message: format!("unknown command: {command}"),
                suggestion: None,
            }),
        }
    }
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

/// Health check for the Datadog provider.
///
/// Validates API key by calling the `GET /api/v1/validate` endpoint
/// with the configured `DD-API-KEY` header.
async fn datadog_check(config: &obz_core::provider::ProviderConfig) -> CheckResult {
    let endpoint = config
        .get("endpoint")
        .unwrap_or("https://api.datadoghq.com")
        .trim_end_matches('/');
    let Some(api_key) = config.auth_get("api-key") else {
        return CheckResult {
            severity: CheckSeverity::Fail,
            message: "api-key not configured".to_string(),
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

    let url = format!("{endpoint}/api/v1/validate");
    let mut request = client
        .get(&url)
        .header("DD-API-KEY", api_key)
        .header("Accept", "application/json");

    if let Some(app_key) = config.auth_get("app-key") {
        request = request.header("DD-APPLICATION-KEY", app_key);
    }

    request = match apply_custom_headers(
        request,
        config.custom_headers(),
        &["dd-api-key", "dd-application-key", "accept"],
        false,
    ) {
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
                    message: "API key validated".to_string(),
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
                    message: format!("validation endpoint returned (HTTP {status})"),
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

/// Factory function: build a [`BuiltProvider`] for Datadog.
///
/// Creates provider instances sharing a single `reqwest::Client` connection pool.
fn build(config: &obz_core::provider::ProviderConfig) -> Result<BuiltProvider, ObzError> {
    let endpoint = config.require_config("endpoint")?;
    validate_endpoint(endpoint)?;
    let api_key = config
        .auth_get("api-key")
        .ok_or_else(|| obz_core::auth_missing_error("api-key", "datadog"))?;
    let app_key = config
        .auth_get("app-key")
        .ok_or_else(|| obz_core::auth_missing_error("app-key", "datadog"))?;
    let verbose = config.verbose();
    let custom_headers = config.custom_headers().clone();

    let timeout = parse_timeout_config(config);
    let client = build_http_client(timeout)?;

    let metric_provider = DatadogProvider::new(
        endpoint,
        client.clone(),
        api_key.to_string(),
        app_key.to_string(),
        custom_headers.clone(),
        verbose,
    );
    let log_provider = DatadogProvider::new(
        endpoint,
        client.clone(),
        api_key.to_string(),
        app_key.to_string(),
        custom_headers.clone(),
        verbose,
    );
    let trace_provider = DatadogProvider::new(
        endpoint,
        client.clone(),
        api_key.to_string(),
        app_key.to_string(),
        custom_headers.clone(),
        verbose,
    );
    let extension_provider = DatadogProvider::new(
        endpoint,
        client,
        api_key.to_string(),
        app_key.to_string(),
        custom_headers,
        verbose,
    );

    Ok(BuiltProvider {
        name: "datadog",
        metric_query_language: Some("Datadog Query"),
        log_query_language: Some("Datadog Log Query"),
        metric: Some(Box::new(metric_provider)),
        log: Some(Box::new(log_provider)),
        trace: Some(Box::new(trace_provider)),
        extension: Some(Box::new(extension_provider)),
    })
}

const DD_QUERY_FLAG: FlagDescriptor = FlagDescriptor {
    name: "query",
    flag_type: FlagType::String,
    required: true,
    default: None,
    description: "Search query filter",
    repeatable: false,
    short: Some('q'),
};

const DD_COMPUTE_FLAG: FlagDescriptor = FlagDescriptor {
    name: "compute",
    flag_type: FlagType::String,
    required: false,
    default: None,
    description: "Aggregation to compute (e.g. count, avg:@duration). Repeatable.",
    repeatable: true,
    short: None,
};

const DD_GROUP_BY_FLAG: FlagDescriptor = FlagDescriptor {
    name: "group-by",
    flag_type: FlagType::String,
    required: false,
    default: None,
    description: "Facet to group by. Repeatable.",
    repeatable: true,
    short: None,
};

static EXTENSION_COMMANDS: &[(&str, CommandDescriptor)] = &[
    (
        "log",
        CommandDescriptor {
            name: "aggregate",
            description: "Aggregate log events with server-side computation",
            flags: &[DD_QUERY_FLAG, DD_COMPUTE_FLAG, DD_GROUP_BY_FLAG],
        },
    ),
    (
        "trace",
        CommandDescriptor {
            name: "aggregate",
            description: "Aggregate trace spans with server-side computation",
            flags: &[DD_QUERY_FLAG, DD_COMPUTE_FLAG, DD_GROUP_BY_FLAG],
        },
    ),
];

/// Return the [`ProviderMeta`] for Datadog.
///
/// Called by [`obz_providers::register_all`] at startup.
pub(crate) fn meta() -> ProviderMeta {
    ProviderMeta {
        name: "datadog",
        display_name: "Datadog",
        aliases: &["dd", "datadog"],
        supported_commands: SupportedCommands {
            metric_query: true,
            metric_list: true,
            metric_info: true,
            metric_labels: false,
            metric_label_values: false,
            metric_series: false,
            log_search: true,
            trace_search: true,
            trace_get: true,
        },
        build,
        check: Some(|config| Box::pin(datadog_check(config))),
        command_flags: &[],
        extension_commands: EXTENSION_COMMANDS,
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Convert a Unix timestamp (seconds) to an ISO 8601 string for Datadog v2 APIs.
fn format_unix_to_iso8601(ts: i64) -> String {
    jiff::Timestamp::from_second(ts)
        .map(|t| t.strftime("%Y-%m-%dT%H:%M:%SZ").to_string())
        .unwrap_or_else(|_| ts.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_provider() -> DatadogProvider {
        let _ = rustls::crypto::ring::default_provider().install_default();
        DatadogProvider::new(
            "http://localhost:0",
            build_http_client(None).unwrap(),
            "api-key".to_string(),
            "app-key".to_string(),
            std::collections::BTreeMap::new(),
            false,
        )
    }

    #[test]
    fn parse_compute_count() {
        assert_eq!(
            DatadogProvider::parse_compute("count"),
            serde_json::json!({"aggregation": "count"})
        );
    }

    #[test]
    fn parse_compute_metric() {
        assert_eq!(
            DatadogProvider::parse_compute("avg:@duration"),
            serde_json::json!({"aggregation": "avg", "metric": "@duration"})
        );
    }

    #[tokio::test]
    async fn aggregate_missing_query() {
        let provider = dummy_provider();
        let params = ExtensionParams {
            start: None,
            end: None,
            signal: "log".to_string(),
            args: Vec::new(),
        };

        let result = provider.execute("aggregate", &params).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            ObzError::InvalidArgument { code, message, .. } => {
                assert_eq!(code, ErrorCode::MissingRequired);
                assert!(message.contains("--query"));
            }
            other => panic!("expected InvalidArgument, got {other:?}"),
        }
    }

    #[test]
    fn build_aggregate_attributes_defaults_to_last_hour() {
        let before = Timestamp::now().as_second();
        let params = ExtensionParams {
            start: None,
            end: None,
            signal: String::new(),
            args: vec![("query".to_string(), "service:web".to_string())],
        };

        let body = DatadogProvider::build_aggregate_attributes(&params).unwrap();

        let filter = body
            .get("filter")
            .and_then(serde_json::Value::as_object)
            .unwrap();
        let from = filter
            .get("from")
            .and_then(serde_json::Value::as_str)
            .unwrap();
        let to = filter
            .get("to")
            .and_then(serde_json::Value::as_str)
            .unwrap();
        let after = Timestamp::now().as_second();

        let expected_from_candidates = [
            format_unix_to_iso8601(before - 3600),
            format_unix_to_iso8601(after - 3600),
        ];
        let expected_to_candidates = [
            format_unix_to_iso8601(before),
            format_unix_to_iso8601(after),
        ];

        assert!(expected_from_candidates.contains(&from.to_string()));
        assert!(expected_to_candidates.contains(&to.to_string()));
    }

    #[test]
    fn aggregate_traces_wraps_attributes_in_data_request() {
        let params = ExtensionParams {
            start: Some(1),
            end: Some(2),
            signal: "trace".to_string(),
            args: vec![("query".to_string(), "service:web".to_string())],
        };

        let body = serde_json::json!({
            "data": {
                "attributes": DatadogProvider::build_aggregate_attributes(&params).unwrap(),
                "type": "aggregate_request",
            }
        });

        assert_eq!(body["data"]["type"], "aggregate_request");
        assert_eq!(body["data"]["attributes"]["filter"]["query"], "service:web");
        assert_eq!(
            body["data"]["attributes"]["filter"]["from"],
            "1970-01-01T00:00:01Z"
        );
        assert_eq!(
            body["data"]["attributes"]["filter"]["to"],
            "1970-01-01T00:00:02Z"
        );
    }
}
