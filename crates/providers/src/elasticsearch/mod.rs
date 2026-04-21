//! Elasticsearch log and trace provider.
//!
//! Implements the [`LogProvider`] and [`TraceProvider`] traits against the
//! Elasticsearch `_search` API with `OTel` (OpenTelemetry) data models via
//! the native Elasticsearch `OTel` integration.
//!
//! # API Endpoints
//!
//! | obz Command    | Endpoint                        | Method |
//! |----------------|----------------------------------|--------|
//! | `log search`   | `POST /{index}/_search`          | POST   |
//! | `trace search` | `POST /{index}/_search`          | POST   |
//! | `trace get`    | `POST /{index}/_search` (by ID)  | POST   |
//!
//! # Index Configuration
//!
//! The target index (or data stream) is specified via the `--index` provider flag.
//! Elasticsearch native `OTel` integration uses data streams like
//! `logs-generic.otel-default` and `traces-generic.otel-default`.
//!
//! # Key differences from `OpenSearch`
//!
//! - Timestamps are `epoch_millis` strings (not RFC 3339).
//! - Field names use `snake_case` (`trace_id`, not `traceId`).
//! - Log body is nested as `body.text` (not a plain string).
//! - Severity is top-level (`severity_text`, not `severity.text`).
//! - Resource is nested as `resource.attributes` (not a flat map).
//! - Span duration is an explicit field in nanoseconds (not computed from start/end).
//! - Span status Unset is represented as `{}` (not `{"code": "Unset"}`).

pub(crate) mod convert;
pub(crate) mod response;

use async_trait::async_trait;
use reqwest::Client;

use obz_core::descriptor::{FlagDescriptor, FlagType};
use obz_core::model::error::{ErrorCode, ObzError};
use obz_core::model::trace::TraceDetail;
use obz_core::provider::results::{LogSearchResult, ProviderResult, TraceSearchResult};
use obz_core::provider::traits::{LogProvider, TraceProvider};
use obz_core::provider::{LogSearchParams, TraceGetParams, TraceSearchParams};
use obz_core::registry::{BuiltProvider, ProviderMeta, SupportedCommands};
use obz_core::StandardCommand;

use crate::util::{
    apply_custom_headers, apply_standard_auth, build_http_client, parse_timeout_config,
    send_request, truncate_for_error, validate_endpoint, HttpResponse,
};
use response::{EsErrorResponse, EsSearchResponse};

/// Maximum number of spans returned when fetching a single trace by ID.
///
/// Traces with more spans will be truncated. A future improvement could
/// use `search_after` pagination to lift this limit.
const MAX_TRACE_SPANS: u64 = 1000;

/// Elasticsearch log and trace provider.
#[derive(Clone)]
pub(crate) struct ElasticsearchProvider {
    /// Base URL of the Elasticsearch cluster (e.g., `http://localhost:9200`).
    base_url: String,
    /// Index or data stream name (e.g., `logs-generic.otel-default`).
    index: Option<String>,
    /// HTTP client with connection pooling.
    client: Client,
    /// Optional basic auth credentials.
    basic_auth: Option<(String, String)>,
    /// Optional bearer token for authentication.
    bearer_token: Option<String>,
    /// Optional Elasticsearch API key (base64-encoded `id:api_key`).
    api_key: Option<String>,
    /// Custom HTTP headers from provider config.
    custom_headers: std::collections::BTreeMap<String, String>,
    /// Whether to print HTTP request/response details to stderr.
    verbose: bool,
}

impl ElasticsearchProvider {
    /// Create a new `ElasticsearchProvider`.
    ///
    /// # Arguments
    ///
    /// * `base_url` — Base URL of the Elasticsearch cluster (without trailing slash).
    /// * `index` — Index or data stream name to query.
    /// * `bearer_token` — Optional bearer token for authentication.
    /// * `basic_auth` — Optional (username, password) for basic auth.
    /// * `api_key` — Optional Elasticsearch API key (base64-encoded `id:api_key`).
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP client cannot be built.
    #[expect(
        clippy::too_many_arguments,
        reason = "custom_headers added for auth migration"
    )]
    pub(crate) fn new(
        base_url: &str,
        index: Option<String>,
        bearer_token: Option<String>,
        basic_auth: Option<(String, String)>,
        api_key: Option<String>,
        custom_headers: std::collections::BTreeMap<String, String>,
        timeout: Option<std::time::Duration>,
        verbose: bool,
    ) -> Result<Self, ObzError> {
        let client = build_http_client(timeout)?;

        Ok(Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            index,
            client,
            basic_auth,
            bearer_token,
            api_key,
            custom_headers,
            verbose,
        })
    }

    /// Build a POST request with authentication and custom headers.
    ///
    /// # Errors
    ///
    /// Returns an error if a reserved header is used in custom headers.
    fn post(&self, path: &str) -> Result<reqwest::RequestBuilder, ObzError> {
        let url = format!("{}{path}", self.base_url);
        let req = self.client.post(&url);
        let req = self.apply_auth(req);
        apply_custom_headers(req, &self.custom_headers, &[], self.verbose)
    }

    /// Apply authentication to a request builder.
    ///
    /// Priority: API key > bearer token > basic auth.
    fn apply_auth(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        if let Some(api_key) = &self.api_key {
            req.header("Authorization", format!("ApiKey {api_key}"))
        } else {
            apply_standard_auth(req, &self.bearer_token, &self.basic_auth)
        }
    }

    /// Send a POST request with a JSON body and deserialize the response.
    async fn send_search(
        &self,
        index: &str,
        body: serde_json::Value,
    ) -> ProviderResult<EsSearchResponse> {
        let encoded_index = crate::util::encode_index_path(index);
        let path = format!("/{encoded_index}/_search");
        let req = self.post(&path)?.json(&body);

        let HttpResponse { status, body: raw } =
            send_request(&self.client, req, self.verbose).await?;

        if !status.is_success() {
            // Try to parse as Elasticsearch error response.
            if let Ok(err_resp) = serde_json::from_str::<EsErrorResponse>(&raw) {
                let code = match err_resp.error.error_type.as_str() {
                    "index_not_found_exception" => ErrorCode::NotFound,
                    "parsing_exception" => ErrorCode::QuerySyntax,
                    "query_shard_exception" => {
                        if err_resp.error.reason.contains("parse")
                            || err_resp.error.reason.contains("syntax")
                        {
                            ErrorCode::QuerySyntax
                        } else {
                            ErrorCode::BackendError
                        }
                    }
                    _ if err_resp.error.reason.contains("parse")
                        || err_resp.error.reason.contains("syntax") =>
                    {
                        ErrorCode::QuerySyntax
                    }
                    _ => ErrorCode::BackendError,
                };
                return Err(ObzError::Provider {
                    code,
                    message: err_resp.error.reason,
                    raw_error: Some(err_resp.error.error_type),
                    recoverable: false,
                    suggestion: None,
                    doc_url: None,
                });
            }

            // Fallback: generic HTTP error.
            return Err(ObzError::Provider {
                code: ErrorCode::BackendError,
                message: format!("Elasticsearch returned HTTP {status}"),
                raw_error: Some(truncate_for_error(&raw, 500)),
                recoverable: status.as_u16() == 429 || status.as_u16() >= 500,
                suggestion: None,
                doc_url: None,
            });
        }

        serde_json::from_str::<EsSearchResponse>(&raw).map_err(|e| ObzError::Provider {
            code: ErrorCode::BackendError,
            message: format!("failed to parse Elasticsearch response (HTTP {status}): {e}"),
            raw_error: Some(truncate_for_error(&raw, 500)),
            recoverable: false,
            suggestion: None,
            doc_url: None,
        })
    }

    /// Build an Elasticsearch query body for log search.
    fn build_log_query(&self, params: &LogSearchParams) -> serde_json::Value {
        self.build_search_query(&params.query, params.start, params.end, params.limit)
    }

    /// Build an Elasticsearch query body for trace search.
    fn build_trace_query(&self, params: &TraceSearchParams) -> serde_json::Value {
        self.build_search_query(&params.query, params.start, params.end, params.limit)
    }

    /// Common query builder for log and trace search.
    ///
    /// Constructs a `bool` query with a `query_string` (or `match_all`) clause
    /// and an `@timestamp` range filter using `epoch_millis` format. Results are
    /// sorted by `@timestamp` descending and capped at `limit`.
    fn build_search_query(
        &self,
        query: &str,
        start: i64,
        end: i64,
        limit: usize,
    ) -> serde_json::Value {
        let query_clause = if query.is_empty() || query == "*" {
            serde_json::json!({ "match_all": {} })
        } else {
            serde_json::json!({
                "query_string": {
                    "query": query
                }
            })
        };

        let range_filter = serde_json::json!({
            "range": {
                "@timestamp": {
                    "gte": start * 1_000,
                    "lte": end * 1_000,
                    "format": "epoch_millis"
                }
            }
        });

        serde_json::json!({
            "query": {
                "bool": {
                    "must": [query_clause],
                    "filter": [range_filter]
                }
            },
            "size": limit,
            "sort": [{ "@timestamp": { "order": "desc" } }]
        })
    }

    /// Build an Elasticsearch query body for trace get by ID.
    ///
    /// Uses `trace_id` (`snake_case`, not `traceId`) and includes an `@timestamp`
    /// range filter to avoid full-index scans.
    ///
    /// # Limitations
    ///
    /// Results are capped at 1000 spans per trace. Traces with more spans will
    /// be truncated. A future improvement could use `search_after` pagination.
    fn build_trace_get_query(&self, params: &TraceGetParams) -> serde_json::Value {
        let range_filter = serde_json::json!({
            "range": {
                "@timestamp": {
                    "gte": params.start * 1_000,
                    "lte": params.end * 1_000,
                    "format": "epoch_millis"
                }
            }
        });

        serde_json::json!({
            "query": {
                "bool": {
                    "must": [{ "term": { "trace_id": params.trace_id } }],
                    "filter": [range_filter]
                }
            },
            "size": MAX_TRACE_SPANS
        })
    }

    fn require_index(&self) -> ProviderResult<&str> {
        self.index
            .as_deref()
            .ok_or_else(|| ObzError::InvalidArgument {
                code: ErrorCode::MissingRequired,
                message: "--index is required for Elasticsearch queries".to_string(),
                suggestion: None,
            })
    }
}

#[async_trait]
impl LogProvider for ElasticsearchProvider {
    async fn search(&self, params: &LogSearchParams) -> ProviderResult<LogSearchResult> {
        let index = self.require_index()?;
        let body = self.build_log_query(params);
        let resp = self.send_search(index, body).await?;
        Ok(convert::convert_log_search_result(resp))
    }
}

#[async_trait]
impl TraceProvider for ElasticsearchProvider {
    async fn search(&self, params: &TraceSearchParams) -> ProviderResult<TraceSearchResult> {
        let index = self.require_index()?;
        let body = self.build_trace_query(params);
        let resp = self.send_search(index, body).await?;
        Ok(convert::convert_trace_search_result(resp))
    }

    async fn get_trace(&self, params: &TraceGetParams) -> ProviderResult<TraceDetail> {
        let index = self.require_index()?;
        let body = self.build_trace_get_query(params);
        let resp = self.send_search(index, body).await?;

        convert::convert_trace_detail(resp, &params.trace_id).ok_or_else(|| ObzError::Provider {
            code: ErrorCode::NotFound,
            message: format!("trace '{}' not found", params.trace_id),
            raw_error: None,
            recoverable: false,
            suggestion: Some(
                "Check the trace ID and ensure the correct index is specified (--index)"
                    .to_string(),
            ),
            doc_url: None,
        })
    }
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

/// Provider-specific flags for Elasticsearch.
///
/// `required` is declared `false` in the descriptor to avoid erroring when a
/// different provider is selected. Enforcement happens at runtime in `build()`
/// after the provider is resolved.
///
/// Note: `OpenSearch` also declares `--index` on the same commands. When
/// multiple providers share a flag name, the first registered provider's
/// description is shown in `--help`. This declaration is still needed so
/// that the flag value can be extracted when `--provider elasticsearch`
/// is active.
static ES_FLAGS: &[FlagDescriptor] = &[FlagDescriptor {
    name: "index",
    flag_type: FlagType::String,
    required: false,
    default: None,
    description: "Elasticsearch index or data stream name (e.g. logs-generic.otel-default)",
    repeatable: false,
    short: None,
}];

/// Command-specific flag bindings.
static COMMAND_FLAGS: &[(StandardCommand, &[FlagDescriptor])] = &[
    (StandardCommand::LogSearch, ES_FLAGS),
    (StandardCommand::TraceSearch, ES_FLAGS),
    (StandardCommand::TraceGet, ES_FLAGS),
];

/// Factory function: build a [`BuiltProvider`] for Elasticsearch.
fn build(config: &obz_core::provider::ProviderConfig) -> Result<BuiltProvider, ObzError> {
    let endpoint = config.require_config("endpoint")?;
    validate_endpoint(endpoint)?;
    let index = config.get("index").map(str::to_string);

    let basic = config.basic_auth();
    let timeout = parse_timeout_config(config);
    let verbose = config.verbose();
    let provider = ElasticsearchProvider::new(
        endpoint,
        index,
        config.bearer_token(),
        basic,
        config.auth_get_owned("api-key"),
        config.custom_headers().clone(),
        timeout,
        verbose,
    )?;
    let trace_provider = provider.clone();
    Ok(BuiltProvider {
        name: "elasticsearch",
        metric_query_language: None,
        log_query_language: Some("Elasticsearch Query DSL"),
        metric: None,
        log: Some(Box::new(provider)),
        trace: Some(Box::new(trace_provider)),
        extension: None,
    })
}

/// Return the [`ProviderMeta`] for Elasticsearch.
///
/// Called by [`obz_providers::register_all`] at startup.
pub(crate) fn meta() -> ProviderMeta {
    ProviderMeta {
        name: "elasticsearch",
        display_name: "Elasticsearch",
        aliases: &["es", "elasticsearch"],
        supported_commands: SupportedCommands {
            metric_query: false,
            metric_list: false,
            metric_info: false,
            metric_labels: false,
            metric_label_values: false,
            metric_series: false,
            log_search: true,
            trace_search: true,
            trace_get: true,
        },
        build,
        check: Some(|config| Box::pin(crate::probe::http_get_probe(config, "/"))),
        command_flags: COMMAND_FLAGS,
        extension_commands: &[],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use obz_core::provider::{LogSearchParams, TraceGetParams};

    #[test]
    fn build_without_index_succeeds() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let mut config = obz_core::provider::ProviderConfig::new();
        config.set("endpoint", "http://localhost:9200");
        match build(&config) {
            Ok(built) => {
                assert!(built.log.is_some());
                assert!(built.trace.is_some());
            }
            Err(e) => panic!("expected Ok, got {e:?}"),
        }
    }

    #[test]
    fn build_with_index_succeeds() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let mut config = obz_core::provider::ProviderConfig::new();
        config.set("endpoint", "http://localhost:9200");
        config.set("index", "logs-generic.otel-default");
        match build(&config) {
            Ok(built) => {
                assert!(built.log.is_some());
                assert!(built.trace.is_some());
            }
            Err(e) => panic!("expected Ok, got {e:?}"),
        }
    }

    #[test]
    fn build_with_api_key_succeeds() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let mut config = obz_core::provider::ProviderConfig::new();
        config.set("endpoint", "http://localhost:9200");
        config.set("index", "logs-generic.otel-default");
        config.set_auth("api-key", "VnVhQ2calDNjd0RlYR0tONZge==");
        match build(&config) {
            Ok(built) => {
                assert!(built.log.is_some());
                assert!(built.trace.is_some());
            }
            Err(e) => panic!("expected Ok, got {e:?}"),
        }
    }

    #[test]
    fn encode_index_preserves_wildcard_and_comma() {
        use crate::util::encode_index_path;

        let cases = [
            ("traces-*", "/traces-*/_search"),
            ("logs-generic.otel-*", "/logs-generic.otel-*/_search"),
            ("index-a,index-b", "/index-a,index-b/_search"),
            ("simple-index", "/simple-index/_search"),
            ("index with spaces", "/index%20with%20spaces/_search"),
        ];
        for (index, expected_path) in cases {
            let path = format!("/{}/_search", encode_index_path(index));
            assert_eq!(path, expected_path, "index: {index}");
        }
    }

    fn test_provider_without_index() -> ElasticsearchProvider {
        let _ = rustls::crypto::ring::default_provider().install_default();
        ElasticsearchProvider::new(
            "http://localhost:9200",
            None,
            None,
            None,
            None,
            std::collections::BTreeMap::new(),
            None,
            false,
        )
        .unwrap_or_else(|error| panic!("expected test provider, got {error:?}"))
    }

    #[tokio::test]
    async fn search_without_index_returns_error() {
        let provider = test_provider_without_index();
        let params = LogSearchParams {
            query: "*".to_string(),
            start: 0,
            end: 1,
            limit: 10,
        };

        match LogProvider::search(&provider, &params).await {
            Err(ObzError::InvalidArgument { code, message, .. }) => {
                assert_eq!(code, ErrorCode::MissingRequired);
                assert!(message.contains("--index"));
            }
            other => panic!("expected InvalidArgument, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn get_trace_without_index_returns_error() {
        let provider = test_provider_without_index();
        let params = TraceGetParams {
            trace_id: "trace-1".to_string(),
            start: 0,
            end: 1,
        };

        match provider.get_trace(&params).await {
            Err(ObzError::InvalidArgument { code, message, .. }) => {
                assert_eq!(code, ErrorCode::MissingRequired);
                assert!(message.contains("--index"));
            }
            other => panic!("expected InvalidArgument, got {other:?}"),
        }
    }
}
