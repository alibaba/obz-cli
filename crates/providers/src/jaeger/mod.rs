//! Jaeger trace provider.
//!
//! Implements the [`TraceProvider`] and [`ExtensionProvider`] traits against
//! the native Jaeger HTTP Query API.
//!
//! # API Endpoints
//!
//! | obz Command      | Endpoint                              | Response Format |
//! |-------------------|--------------------------------------|-----------------|
//! | `trace search`    | `GET /api/traces`                    | JSON            |
//! | `trace get`       | `GET /api/traces/{id}`               | JSON            |
//! | `ext services`    | `GET /api/services`                  | JSON            |
//! | `ext operations`  | `GET /api/services/{svc}/operations` | JSON            |

pub(crate) mod convert;
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
    apply_custom_headers, apply_standard_auth, build_http_client, http_error, parse_timeout_config,
    send_request, truncate_for_error, validate_endpoint, HttpResponse,
};
use response::{JaegerResponse, JaegerTrace};

/// Jaeger trace provider (native Jaeger HTTP Query API).
#[derive(Clone)]
pub(crate) struct JaegerProvider {
    /// Base URL of the Jaeger Query instance (e.g. `http://localhost:16686`).
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

impl JaegerProvider {
    /// Create a new `JaegerProvider`.
    ///
    /// # Arguments
    ///
    /// * `base_url` — Base URL of the Jaeger Query instance (no trailing slash).
    /// * `bearer_token` — Optional bearer token.
    /// * `basic_auth` — Optional (username, password) for basic auth.
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP client cannot be built.
    pub(crate) fn new(
        base_url: &str,
        bearer_token: Option<String>,
        basic_auth: Option<(String, String)>,
        custom_headers: std::collections::BTreeMap<String, String>,
        timeout: Option<std::time::Duration>,
        verbose: bool,
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

    /// Send a request and deserialize the Jaeger response envelope.
    async fn send_jaeger<T>(&self, req: reqwest::RequestBuilder) -> ProviderResult<T>
    where
        T: serde::de::DeserializeOwned + Default,
    {
        let HttpResponse { status, body } = send_request(&self.client, req, self.verbose).await?;

        if !status.is_success() {
            return Err(http_error(status, &body, "Jaeger"));
        }

        let envelope: JaegerResponse<T> =
            serde_json::from_str(&body).map_err(|e| ObzError::Provider {
                code: ErrorCode::BackendError,
                message: format!("failed to parse Jaeger response: {e}"),
                raw_error: Some(truncate_for_error(&body, 300)),
                recoverable: false,
                suggestion: None,
                doc_url: None,
            })?;

        // Jaeger returns null data only for missing resources (otherwise HTTP 404),
        // so unwrap_or_default() is safe here — null data ≡ empty result.
        Ok(envelope.data.unwrap_or_default())
    }
}

#[async_trait]
impl TraceProvider for JaegerProvider {
    async fn search(&self, params: &TraceSearchParams) -> ProviderResult<TraceSearchResult> {
        // Jaeger /api/traces requires a non-empty service name.
        if params.query.is_empty() {
            return Err(ObzError::InvalidArgument {
                code: ErrorCode::MissingRequired,
                message: "Jaeger requires a service name: use -q <service-name>".to_string(),
                suggestion: None,
            });
        }

        // Jaeger API expects microsecond timestamps for start/end.
        let req = self
            .get("/api/traces")?
            .query(&[
                ("start", &(params.start * 1_000_000).to_string()),
                ("end", &(params.end * 1_000_000).to_string()),
                ("limit", &params.limit.to_string()),
            ])
            .query(&[("service", &params.query)]);

        let traces: Vec<JaegerTrace> = self.send_jaeger(req).await?;
        Ok(convert::convert_search_result(traces))
    }

    async fn get_trace(&self, params: &TraceGetParams) -> ProviderResult<TraceDetail> {
        // Jaeger /api/traces/{id} does not accept start/end parameters;
        // they are only used by /api/traces (search).
        let req = self.get(&format!("/api/traces/{}", params.trace_id))?;

        let mut traces: Vec<JaegerTrace> = self.send_jaeger(req).await?;

        let trace = traces.pop().ok_or_else(|| ObzError::Provider {
            code: ErrorCode::NotFound,
            message: format!("trace '{}' not found", params.trace_id),
            raw_error: None,
            recoverable: false,
            suggestion: Some("Check the trace ID. The default time range is 1 hour; if the trace is older, use --from (e.g. --from now-6h)".to_string()),
            doc_url: None,
        })?;

        Ok(convert::convert_trace_detail(&trace))
    }
}

#[async_trait]
impl ExtensionProvider for JaegerProvider {
    async fn execute(
        &self,
        command: &str,
        params: &ExtensionParams,
    ) -> ProviderResult<ExtensionResult> {
        match command {
            "services" => {
                let req = self.get("/api/services")?;
                let items: Vec<String> = self.send_jaeger(req).await?;
                Ok(ExtensionResult::from_strings(items))
            }
            "operations" => {
                let service = params
                    .get("service")
                    .ok_or_else(|| ObzError::InvalidArgument {
                        code: ErrorCode::MissingRequired,
                        message: "--service is required for the operations command".to_string(),
                        suggestion: None,
                    })?;
                let encoded = urlencoding::encode(service);
                let req = self.get(&format!("/api/services/{encoded}/operations"))?;
                let items: Vec<String> = self.send_jaeger(req).await?;
                Ok(ExtensionResult::from_strings(items))
            }
            _ => Err(ObzError::Unsupported {
                message: format!("unknown extension command: {command}"),
                provider: Some("jaeger".to_string()),
                suggestion: None,
            }),
        }
    }
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

/// Extension commands for Jaeger.
static EXTENSION_COMMANDS: &[(&str, CommandDescriptor)] = &[
    (
        "trace",
        CommandDescriptor {
            name: "services",
            description: "List available service names",
            flags: &[],
        },
    ),
    (
        "trace",
        CommandDescriptor {
            name: "operations",
            description: "List operations for a service",
            flags: &[FlagDescriptor {
                name: "service",
                flag_type: FlagType::String,
                required: true,
                default: None,
                description: "Service name to list operations for",
                repeatable: false,
                short: None,
            }],
        },
    ),
];

/// Factory function: build a [`BuiltProvider`] for Jaeger.
fn build(config: &obz_core::provider::ProviderConfig) -> Result<BuiltProvider, ObzError> {
    let endpoint = config.require_config("endpoint")?;
    validate_endpoint(endpoint)?;
    let basic = config.basic_auth();
    let timeout = parse_timeout_config(config);
    let verbose = config.verbose();
    let provider = JaegerProvider::new(
        endpoint,
        config.bearer_token(),
        basic,
        config.custom_headers().clone(),
        timeout,
        verbose,
    )?;
    let ext_provider = provider.clone();
    Ok(BuiltProvider {
        name: "jaeger",
        metric_query_language: None,
        log_query_language: None,
        metric: None,
        log: None,
        trace: Some(Box::new(provider)),
        extension: Some(Box::new(ext_provider)),
    })
}

/// Return the [`ProviderMeta`] for Jaeger.
///
/// Called by [`obz_providers::register_all`] at startup.
pub(crate) fn meta() -> ProviderMeta {
    ProviderMeta {
        name: "jaeger",
        display_name: "Jaeger",
        aliases: &["jg", "jaeger"],
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
        check: Some(|config| Box::pin(crate::probe::http_get_probe(config, "/"))),
        command_flags: &[],
        extension_commands: EXTENSION_COMMANDS,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use obz_core::model::error::ErrorCode;

    /// Create a Jaeger provider pointed at a dummy URL (for testing error paths
    /// that return before making HTTP calls).
    fn dummy_provider() -> JaegerProvider {
        let _ = rustls::crypto::ring::default_provider().install_default();
        JaegerProvider::new(
            "http://localhost:0",
            None,
            None,
            std::collections::BTreeMap::new(),
            None,
            false,
        )
        .unwrap()
    }

    #[tokio::test]
    async fn extension_operations_missing_service() {
        let provider = dummy_provider();
        let params = ExtensionParams {
            start: None,
            end: None,
            signal: "trace".to_string(),
            args: Vec::new(),
        };

        let result = provider.execute("operations", &params).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            ObzError::InvalidArgument { code, message, .. } => {
                assert_eq!(code, ErrorCode::MissingRequired);
                assert!(message.contains("--service"));
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
                assert_eq!(provider.as_deref(), Some("jaeger"));
            }
            other => panic!("expected Unsupported, got {other:?}"),
        }
    }
}
