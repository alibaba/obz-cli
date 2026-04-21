//! Grafana Tempo trace provider.
//!
//! Implements the [`TraceProvider`] and [`ExtensionProvider`] traits against
//! the Tempo HTTP API. Tempo uses `TraceQL` as its query language and returns
//! trace data in OTLP protobuf-JSON format.
//!
//! # API Endpoints
//!
//! | obz Command       | Tempo Endpoint                          | Response Format |
//! |--------------------|-----------------------------------------|-----------------|
//! | `trace search`     | `GET /api/search`                      | JSON            |
//! | `trace get`        | `GET /api/traces/{id}`                 | OTLP JSON       |
//! | `ext tags`         | `GET /api/v2/search/tags`              | JSON            |
//! | `ext tag-values`   | `GET /api/v2/search/tag/{name}/values` | JSON            |

/// Tempo → obz trace model conversion functions.
pub(crate) mod convert;
/// Tempo API response deserialization types.
pub(crate) mod response;

use async_trait::async_trait;
use reqwest::Client;

use obz_core::descriptor::{CommandDescriptor, FlagDescriptor, FlagType};
use obz_core::model::error::{ErrorCode, ObzError};
use obz_core::model::trace::TraceDetail;
use obz_core::provider::params::ExtensionParams;
use obz_core::provider::results::{ExtensionResult, ProviderResult, TraceSearchResult};
use obz_core::provider::traits::{ExtensionProvider, TraceProvider};
use obz_core::provider::{TraceGetParams, TraceSearchParams};
use obz_core::registry::{BuiltProvider, ProviderMeta, SupportedCommands};

use crate::util::{
    apply_custom_headers, apply_standard_auth, build_http_client, parse_timeout_config,
    send_and_parse_json, validate_endpoint,
};
use response::{OtlpTraceResponse, TempoSearchResponse, TempoTagValuesResponse, TempoTagsResponse};

/// Grafana Tempo trace provider.
// Clone is needed because `BuiltProvider` requires separate boxed trait
// objects for `trace` and `extension`, so we clone the provider instance
// during registration.
#[derive(Clone)]
pub(crate) struct TempoProvider {
    /// Base URL of the Tempo instance (e.g., `http://localhost:3200`).
    base_url: String,
    /// HTTP client with connection pooling.
    client: Client,
    /// Optional bearer token for authentication.
    bearer_token: Option<String>,
    /// Optional basic auth credentials.
    basic_auth: Option<(String, String)>,
    /// Custom HTTP headers from provider config.
    custom_headers: std::collections::BTreeMap<String, String>,
    /// Whether to print HTTP request/response details to stderr.
    verbose: bool,
}

impl TempoProvider {
    /// Create a new `TempoProvider`.
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP client cannot be built.
    pub(crate) fn new(
        base_url: &str,
        bearer_token: Option<String>,
        basic_auth: Option<(String, String)>,
        custom_headers: std::collections::BTreeMap<String, String>,
        verbose: bool,
        timeout: Option<std::time::Duration>,
    ) -> Result<Self, ObzError> {
        let client = build_http_client(timeout)?;
        Ok(Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            client,
            bearer_token,
            basic_auth,
            custom_headers,
            verbose,
        })
    }

    /// Build a GET request with authentication and custom headers.
    ///
    /// # Errors
    ///
    /// Returns an error if a reserved header is used in custom headers.
    fn get(&self, path: &str) -> Result<reqwest::RequestBuilder, ObzError> {
        let url = format!("{}{path}", self.base_url);
        let req = self.client.get(&url);
        let req = apply_standard_auth(req, &self.bearer_token, &self.basic_auth);
        apply_custom_headers(req, &self.custom_headers, &[], self.verbose)
    }

    /// Send a request and deserialize the JSON response.
    async fn send_json<T: serde::de::DeserializeOwned>(
        &self,
        req: reqwest::RequestBuilder,
    ) -> ProviderResult<T> {
        send_and_parse_json(&self.client, req, "Tempo", self.verbose).await
    }
}

#[async_trait]
impl TraceProvider for TempoProvider {
    async fn search(&self, params: &TraceSearchParams) -> ProviderResult<TraceSearchResult> {
        let mut req = self.get("/api/search")?.query(&[
            ("start", &params.start.to_string()),
            ("end", &params.end.to_string()),
            ("limit", &params.limit.to_string()),
        ]);

        // Tempo uses "q" for TraceQL queries. An empty query is valid (returns all).
        if !params.query.is_empty() {
            req = req.query(&[("q", &params.query)]);
        }

        let resp: TempoSearchResponse = self.send_json(req).await?;
        Ok(convert::convert_search_result(resp))
    }

    async fn get_trace(&self, params: &TraceGetParams) -> ProviderResult<TraceDetail> {
        let req = self.get(&format!("/api/traces/{}", params.trace_id))?;

        let resp: OtlpTraceResponse = self.send_json(req).await?;

        if resp.batches.is_empty() {
            return Err(ObzError::Provider {
                code: ErrorCode::NotFound,
                message: format!("trace '{}' not found", params.trace_id),
                raw_error: None,
                recoverable: false,
                suggestion: Some("Check the trace ID. The default time range is 1 hour; if the trace is older, use --from (e.g. --from now-6h)".to_string()),
                doc_url: None,
            });
        }

        Ok(convert::convert_trace_detail(&resp))
    }
}

#[async_trait]
impl ExtensionProvider for TempoProvider {
    async fn execute(
        &self,
        command: &str,
        params: &ExtensionParams,
    ) -> ProviderResult<ExtensionResult> {
        // NOTE: `start`/`end` from `params` are not forwarded to the Tempo
        // tags/tag-values APIs. These endpoints support optional time-range
        // filtering, which can be added as a follow-up.
        match command {
            "tags" => {
                let req = self.get("/api/v2/search/tags")?;
                let resp: TempoTagsResponse = self.send_json(req).await?;
                let items = convert::convert_tags_response(resp);
                Ok(ExtensionResult::from_strings(items))
            }
            "tag-values" => {
                let tag = params.get("tag").ok_or_else(|| ObzError::InvalidArgument {
                    code: ErrorCode::MissingRequired,
                    message: "--tag is required for the tag-values command".to_string(),
                    suggestion: None,
                })?;
                let encoded = urlencoding::encode(tag);
                let req = self.get(&format!("/api/v2/search/tag/{encoded}/values"))?;
                let resp: TempoTagValuesResponse = self.send_json(req).await?;
                let items = convert::convert_tag_values_response(resp);
                Ok(ExtensionResult::from_strings(items))
            }
            _ => Err(ObzError::Unsupported {
                message: format!("unknown extension command: {command}"),
                provider: Some("tempo".to_string()),
                suggestion: None,
            }),
        }
    }
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

/// Extension commands for Tempo.
static EXTENSION_COMMANDS: &[(&str, CommandDescriptor)] = &[
    (
        "trace",
        CommandDescriptor {
            name: "tags",
            description: "List available tag names (grouped by scope)",
            flags: &[],
        },
    ),
    (
        "trace",
        CommandDescriptor {
            name: "tag-values",
            description: "List values for a specific tag",
            flags: &[FlagDescriptor {
                name: "tag",
                flag_type: FlagType::String,
                required: true,
                default: None,
                description: "Tag name to list values for (e.g., resource.service.name)",
                repeatable: false,
                short: None,
            }],
        },
    ),
];

/// Factory function: build a [`BuiltProvider`] for Tempo.
fn build(config: &obz_core::provider::ProviderConfig) -> Result<BuiltProvider, ObzError> {
    let endpoint = config.require_config("endpoint")?;
    validate_endpoint(endpoint)?;
    let basic = config.basic_auth();
    let verbose = config.verbose();
    let timeout = parse_timeout_config(config);
    let provider = TempoProvider::new(
        endpoint,
        config.bearer_token(),
        basic,
        config.custom_headers().clone(),
        verbose,
        timeout,
    )?;
    let ext_provider = provider.clone();
    Ok(BuiltProvider {
        name: "tempo",
        metric_query_language: None,
        log_query_language: None,
        metric: None,
        log: None,
        trace: Some(Box::new(provider)),
        extension: Some(Box::new(ext_provider)),
    })
}

/// Return the [`ProviderMeta`] for Tempo.
///
/// Called by [`obz_providers::register_all`] at startup.
pub(crate) fn meta() -> ProviderMeta {
    ProviderMeta {
        name: "tempo",
        display_name: "Grafana Tempo",
        aliases: &["tempo"],
        supported_commands: SupportedCommands {
            metric_query: false,
            metric_list: false,
            metric_info: false,
            metric_labels: false,
            metric_label_values: false,
            metric_series: false,
            log_search: false,
            trace_search: true,
            trace_get: true,
        },
        build,
        check: Some(|config| Box::pin(crate::probe::http_get_probe(config, "/ready"))),
        command_flags: &[],
        extension_commands: EXTENSION_COMMANDS,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use obz_core::model::error::ErrorCode;

    /// Create a Tempo provider pointed at a dummy URL (for testing error paths).
    fn dummy_provider() -> TempoProvider {
        let _ = rustls::crypto::ring::default_provider().install_default();
        TempoProvider::new(
            "http://localhost:0",
            None,
            None,
            std::collections::BTreeMap::new(),
            false,
            None,
        )
        .unwrap()
    }

    #[tokio::test]
    async fn extension_tag_values_missing_tag() {
        let provider = dummy_provider();
        let params = ExtensionParams {
            start: None,
            end: None,
            signal: "trace".to_string(),
            args: Vec::new(),
        };

        let result = provider.execute("tag-values", &params).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            ObzError::InvalidArgument { code, message, .. } => {
                assert_eq!(code, ErrorCode::MissingRequired);
                assert!(message.contains("--tag"));
            }
            other => panic!("expected InvalidArgument, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn extension_unknown_command() {
        let provider = dummy_provider();
        let params = ExtensionParams {
            start: None,
            end: None,
            signal: "trace".to_string(),
            args: Vec::new(),
        };

        let result = provider.execute("nonexistent", &params).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            ObzError::Unsupported {
                message, provider, ..
            } => {
                assert!(message.contains("nonexistent"));
                assert_eq!(provider.as_deref(), Some("tempo"));
            }
            other => panic!("expected Unsupported, got {other:?}"),
        }
    }
}
