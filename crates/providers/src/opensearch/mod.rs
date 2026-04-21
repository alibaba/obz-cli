//! `OpenSearch` log and trace provider.
//!
//! Implements the [`LogProvider`] and [`TraceProvider`] traits against the
//! `OpenSearch` `_search` API with `OTel` (OpenTelemetry) data models.
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
//! The target index name is specified via the `--index` provider flag.

pub(crate) mod convert;
pub(crate) mod response;

use async_trait::async_trait;
use jiff::Timestamp;
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
use response::{OsErrorResponse, OsSearchResponse};

/// Maximum number of spans returned when fetching a single trace by ID.
///
/// Traces with more spans will be truncated. A future improvement could
/// use `search_after` pagination to lift this limit.
const MAX_TRACE_SPANS: u64 = 1000;

/// `OpenSearch` log and trace provider.
#[derive(Clone)]
pub(crate) struct OpenSearchProvider {
    /// Base URL of the `OpenSearch` cluster (e.g., `http://localhost:9200`).
    base_url: String,
    /// Index name or pattern for the signal (e.g., `otel-logs-*`).
    index: Option<String>,
    /// HTTP client with connection pooling.
    client: Client,
    /// Optional basic auth credentials.
    basic_auth: Option<(String, String)>,
    /// Optional bearer token for authentication.
    bearer_token: Option<String>,
    /// Custom HTTP headers from provider config.
    custom_headers: std::collections::BTreeMap<String, String>,
    /// Whether to print HTTP request/response details to stderr.
    verbose: bool,
}

impl OpenSearchProvider {
    /// Create a new `OpenSearchProvider`.
    ///
    /// # Arguments
    ///
    /// * `base_url` — Base URL of the `OpenSearch` cluster (without trailing slash).
    /// * `index` — Index name or pattern to query.
    /// * `bearer_token` — Optional bearer token for authentication.
    /// * `basic_auth` — Optional (username, password) for basic auth.
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP client cannot be built.
    pub(crate) fn new(
        base_url: &str,
        index: Option<String>,
        bearer_token: Option<String>,
        basic_auth: Option<(String, String)>,
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
        let req = apply_standard_auth(req, &self.bearer_token, &self.basic_auth);
        apply_custom_headers(req, &self.custom_headers, &[], self.verbose)
    }

    /// Send a POST request with a JSON body and deserialize the response.
    async fn send_search(
        &self,
        index: &str,
        body: serde_json::Value,
    ) -> ProviderResult<OsSearchResponse> {
        let encoded_index = crate::util::encode_index_path(index);
        let path = format!("/{encoded_index}/_search");
        let req = self.post(&path)?.json(&body);

        let HttpResponse { status, body: raw } =
            send_request(&self.client, req, self.verbose).await?;

        if !status.is_success() {
            // Try to parse as OpenSearch error response.
            if let Ok(err_resp) = serde_json::from_str::<OsErrorResponse>(&raw) {
                let code = match err_resp.error.error_type.as_str() {
                    "index_not_found_exception" => ErrorCode::NotFound,
                    "parsing_exception" => ErrorCode::QuerySyntax,
                    "query_shard_exception" => {
                        // query_shard_exception can be a syntax error or a shard failure;
                        // check the reason string to distinguish.
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
                message: format!("OpenSearch returned HTTP {status}"),
                raw_error: Some(truncate_for_error(&raw, 500)),
                recoverable: status.as_u16() == 429 || status.as_u16() >= 500,
                suggestion: None,
                doc_url: None,
            });
        }

        serde_json::from_str::<OsSearchResponse>(&raw).map_err(|e| ObzError::Provider {
            code: ErrorCode::BackendError,
            message: format!("failed to parse OpenSearch response (HTTP {status}): {e}"),
            raw_error: Some(truncate_for_error(&raw, 500)),
            recoverable: false,
            suggestion: None,
            doc_url: None,
        })
    }

    /// Build an `OpenSearch` query body for log search.
    fn build_log_query(&self, params: &LogSearchParams) -> serde_json::Value {
        let query = if params.query.is_empty() || params.query == "*" {
            serde_json::json!({ "match_all": {} })
        } else {
            serde_json::json!({
                "query_string": {
                    "query": params.query
                }
            })
        };

        // Wrap with time range filter using ISO 8601 timestamps.
        let range_filter = serde_json::json!({
            "range": {
                "@timestamp": {
                    "gte": epoch_to_iso8601(params.start),
                    "lte": epoch_to_iso8601(params.end)
                }
            }
        });

        let bool_query = serde_json::json!({
            "bool": {
                "must": [query],
                "filter": [range_filter]
            }
        });

        serde_json::json!({
            "query": bool_query,
            "size": params.limit,
            "sort": [{ "@timestamp": { "order": "desc" } }]
        })
    }

    /// Build an `OpenSearch` query body for trace search.
    fn build_trace_query(&self, params: &TraceSearchParams) -> serde_json::Value {
        let query = if params.query.is_empty() || params.query == "*" {
            serde_json::json!({ "match_all": {} })
        } else {
            serde_json::json!({
                "query_string": {
                    "query": params.query
                }
            })
        };

        // Use ISO 8601 timestamps — `startTime` is an ISO 8601 string field.
        let range_filter = serde_json::json!({
            "range": {
                "startTime": {
                    "gte": epoch_to_iso8601(params.start),
                    "lte": epoch_to_iso8601(params.end)
                }
            }
        });

        let bool_query = serde_json::json!({
            "bool": {
                "must": [query],
                "filter": [range_filter]
            }
        });

        serde_json::json!({
            "query": bool_query,
            "size": params.limit,
            "sort": [{ "startTime": { "order": "desc" } }]
        })
    }

    /// Build an `OpenSearch` query body for trace get by ID.
    ///
    /// Includes a `startTime` range filter to avoid full-index scans on
    /// large clusters when the time range is available.
    fn build_trace_get_query(&self, params: &TraceGetParams) -> serde_json::Value {
        let range_filter = serde_json::json!({
            "range": {
                "startTime": {
                    "gte": epoch_to_iso8601(params.start),
                    "lte": epoch_to_iso8601(params.end)
                }
            }
        });

        serde_json::json!({
            "query": {
                "bool": {
                    "must": [{ "term": { "traceId": params.trace_id } }],
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
                message: "--index is required for OpenSearch queries".to_string(),
                suggestion: None,
            })
    }
}

/// Convert a Unix epoch (seconds) to an ISO 8601 string for `OpenSearch` range queries.
///
/// `unwrap_or(UNIX_EPOCH)` is safe because epoch values come from
/// `obz_core::time::parse_time()` which validates the range. The only way
/// `from_second()` fails is for timestamps outside ~year ±9999, which the
/// time module rejects before we reach here.
fn epoch_to_iso8601(epoch_secs: i64) -> String {
    Timestamp::from_second(epoch_secs)
        .unwrap_or(Timestamp::UNIX_EPOCH)
        .to_string()
}

#[async_trait]
impl LogProvider for OpenSearchProvider {
    async fn search(&self, params: &LogSearchParams) -> ProviderResult<LogSearchResult> {
        let index = self.require_index()?;
        let body = self.build_log_query(params);
        let resp = self.send_search(index, body).await?;
        Ok(convert::convert_log_search_result(resp))
    }
}

#[async_trait]
impl TraceProvider for OpenSearchProvider {
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

/// Provider-specific flags for `OpenSearch`.
///
/// `required` is declared `false` in the descriptor to avoid erroring when a
/// different provider is selected. Enforcement happens at runtime in `build()`
/// after the provider is resolved.
static INDEX_FLAGS: &[FlagDescriptor] = &[FlagDescriptor {
    name: "index",
    flag_type: FlagType::String,
    required: false,
    default: None,
    description:
        "Index name, pattern, or data stream (e.g. otel-logs-*, logs-generic.otel-default)",
    repeatable: false,
    short: None,
}];

/// Command-specific flag bindings.
static COMMAND_FLAGS: &[(StandardCommand, &[FlagDescriptor])] = &[
    (StandardCommand::LogSearch, INDEX_FLAGS),
    (StandardCommand::TraceSearch, INDEX_FLAGS),
    (StandardCommand::TraceGet, INDEX_FLAGS),
];

/// Factory function: build a [`BuiltProvider`] for `OpenSearch`.
fn build(config: &obz_core::provider::ProviderConfig) -> Result<BuiltProvider, ObzError> {
    let endpoint = config.require_config("endpoint")?;
    validate_endpoint(endpoint)?;
    let index = config.get("index").map(str::to_string);

    let basic = config.basic_auth();
    let timeout = parse_timeout_config(config);
    let verbose = config.verbose();
    let provider = OpenSearchProvider::new(
        endpoint,
        index,
        config.bearer_token(),
        basic,
        config.custom_headers().clone(),
        timeout,
        verbose,
    )?;
    let trace_provider = provider.clone();
    Ok(BuiltProvider {
        name: "opensearch",
        metric_query_language: None,
        log_query_language: Some("OpenSearch DSL"),
        metric: None,
        log: Some(Box::new(provider)),
        trace: Some(Box::new(trace_provider)),
        extension: None,
    })
}

/// Return the [`ProviderMeta`] for `OpenSearch`.
///
/// Called by [`obz_providers::register_all`] at startup.
pub(crate) fn meta() -> ProviderMeta {
    ProviderMeta {
        name: "opensearch",
        display_name: "OpenSearch",
        aliases: &["os", "opensearch"],
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
        config.set("index", "otel-logs-*");
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
            ("otel-logs-*", "/otel-logs-*/_search"),
            ("index-a,index-b", "/index-a,index-b/_search"),
            ("simple-index", "/simple-index/_search"),
        ];
        for (index, expected_path) in cases {
            let path = format!("/{}/_search", encode_index_path(index));
            assert_eq!(path, expected_path, "index: {index}");
        }
    }

    fn test_provider_without_index() -> OpenSearchProvider {
        let _ = rustls::crypto::ring::default_provider().install_default();
        OpenSearchProvider::new(
            "http://localhost:9200",
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
