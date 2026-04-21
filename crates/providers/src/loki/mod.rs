//! Grafana Loki log provider.
//!
//! Implements the [`LogProvider`] trait against the Loki HTTP API.
//! Loki uses `LogQL` as its query language and returns log data in a
//! `streams` format with nanosecond timestamps.
//!
//! # API Endpoints
//!
//! | obz Command | Loki Endpoint | Response Format |
//! |-------------|---------------|-----------------|
//! | `log search` | `GET /loki/api/v1/query_range` | JSON (streams) |

/// Loki → obz `LogEntry` conversion functions.
pub(crate) mod convert;
/// Loki API response deserialization types.
pub(crate) mod response;

use async_trait::async_trait;
use reqwest::Client;

use crate::util::{
    apply_custom_headers, apply_standard_auth, build_http_client, parse_timeout_config,
    send_and_parse_json, validate_endpoint,
};
use obz_core::descriptor::{CommandDescriptor, FlagDescriptor, FlagType};
use obz_core::model::error::{ErrorCode, ObzError};
use obz_core::provider::params::ExtensionParams;
use obz_core::provider::results::{ExtensionResult, ProviderResult};
use obz_core::provider::traits::ExtensionProvider;
use obz_core::provider::{LogProvider, LogSearchParams, LogSearchResult};
use obz_core::registry::{BuiltProvider, ProviderMeta, SupportedCommands};

use self::response::{LokiDetectedFieldsResponse, LokiQueryData, LokiResponse};

/// Grafana Loki log provider.
#[derive(Clone)]
pub(crate) struct LokiProvider {
    /// Base URL of the Loki instance (e.g., `http://localhost:3100`).
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

impl LokiProvider {
    /// Convert Unix seconds to Loki nanosecond query timestamps.
    fn to_loki_timestamp(ts: i64) -> String {
        ts.saturating_mul(1_000_000_000).to_string()
    }

    /// Create a new `LokiProvider`.
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
        send_and_parse_json(&self.client, req, "Loki", self.verbose).await
    }

    /// List available label names.
    async fn list_labels(&self, params: &ExtensionParams) -> ProviderResult<ExtensionResult> {
        let mut req = self.get("/loki/api/v1/labels")?;
        if let Some(start) = params.start {
            req = req.query(&[("start", Self::to_loki_timestamp(start))]);
        }
        if let Some(end) = params.end {
            req = req.query(&[("end", Self::to_loki_timestamp(end))]);
        }

        let resp: LokiResponse<Vec<String>> = self.send_json(req).await?;
        Ok(ExtensionResult::from_strings(resp.data.unwrap_or_default()))
    }

    /// List values for a specific label.
    async fn list_label_values(&self, params: &ExtensionParams) -> ProviderResult<ExtensionResult> {
        let label = params.require("label")?;
        let path = format!("/loki/api/v1/label/{}/values", urlencoding::encode(label));
        let mut req = self.get(&path)?;
        if let Some(start) = params.start {
            req = req.query(&[("start", Self::to_loki_timestamp(start))]);
        }
        if let Some(end) = params.end {
            req = req.query(&[("end", Self::to_loki_timestamp(end))]);
        }

        let resp: LokiResponse<Vec<String>> = self.send_json(req).await?;
        Ok(ExtensionResult::from_strings(resp.data.unwrap_or_default()))
    }

    /// List detected fields in log content.
    async fn list_fields(&self, params: &ExtensionParams) -> ProviderResult<ExtensionResult> {
        let query = params.get("query").unwrap_or("{__name__=~\".+\"}");
        let mut req = self
            .get("/loki/api/v1/detected_fields")?
            .query(&[("query", query)]);
        if let Some(start) = params.start {
            req = req.query(&[("start", Self::to_loki_timestamp(start))]);
        }
        if let Some(end) = params.end {
            req = req.query(&[("end", Self::to_loki_timestamp(end))]);
        }

        let resp: LokiDetectedFieldsResponse = self.send_json(req).await?;
        let fields = resp.fields;
        let total_count = fields.len();
        Ok(ExtensionResult {
            data: serde_json::to_value(fields).map_err(|e| ObzError::Provider {
                code: ErrorCode::BackendError,
                message: format!("failed to serialize Loki detected fields: {e}"),
                raw_error: None,
                recoverable: false,
                suggestion: None,
                doc_url: None,
            })?,
            total_count: Some(total_count),
        })
    }

    /// Run an instant `LogQL` metric query for log aggregation.
    async fn get_stats(&self, params: &ExtensionParams) -> ProviderResult<ExtensionResult> {
        let query = params.require("query")?;
        let mut req = self.get("/loki/api/v1/query")?.query(&[("query", query)]);
        if let Some(end) = params.end {
            req = req.query(&[("time", Self::to_loki_timestamp(end))]);
        }

        let data: serde_json::Value = self.send_json(req).await?;
        let total_count = data
            .get("data")
            .and_then(|v| v.get("result"))
            .and_then(serde_json::Value::as_array)
            .map(std::vec::Vec::len);
        Ok(ExtensionResult { data, total_count })
    }
}

#[async_trait]
impl LogProvider for LokiProvider {
    async fn search(&self, params: &LogSearchParams) -> ProviderResult<LogSearchResult> {
        // Loki expects nanosecond timestamps for start/end.
        let start_ns = params.start.saturating_mul(1_000_000_000);
        let end_ns = params.end.saturating_mul(1_000_000_000);

        let req = self.get("/loki/api/v1/query_range")?.query(&[
            ("query", params.query.as_str()),
            ("start", &start_ns.to_string()),
            ("end", &end_ns.to_string()),
            ("limit", &params.limit.to_string()),
        ]);

        let resp: LokiResponse<LokiQueryData> = self.send_json(req).await?;
        convert::convert_query_response(resp)
    }
}

#[async_trait]
impl ExtensionProvider for LokiProvider {
    async fn execute(
        &self,
        command: &str,
        params: &ExtensionParams,
    ) -> ProviderResult<ExtensionResult> {
        match command {
            "labels" => self.list_labels(params).await,
            "label-values" => self.list_label_values(params).await,
            "fields" => self.list_fields(params).await,
            "stats" => self.get_stats(params).await,
            _ => Err(ObzError::InvalidArgument {
                code: ErrorCode::InvalidFlag,
                message: format!("unknown command: {command}"),
                suggestion: None,
            }),
        }
    }
}

/// Extension commands for Loki.
static EXTENSION_COMMANDS: &[(&str, CommandDescriptor)] = &[
    (
        "log",
        CommandDescriptor {
            name: "labels",
            description: "List available label names",
            flags: &[],
        },
    ),
    (
        "log",
        CommandDescriptor {
            name: "label-values",
            description: "List values for a specific label",
            flags: &[FlagDescriptor {
                name: "label",
                flag_type: FlagType::String,
                required: true,
                default: None,
                description: "Label name to get values for",
                repeatable: false,
                short: None,
            }],
        },
    ),
    (
        "log",
        CommandDescriptor {
            name: "fields",
            description: "List detected fields in log content",
            flags: &[FlagDescriptor {
                name: "query",
                flag_type: FlagType::String,
                required: false,
                default: None,
                description: "Log stream selector (default: all streams; specify to narrow scope on large clusters)",
                repeatable: false,
                short: Some('q'),
            }],
        },
    ),
    (
        "log",
        CommandDescriptor {
            name: "stats",
            description: "Run instant query aggregation on logs",
            flags: &[FlagDescriptor {
                name: "query",
                flag_type: FlagType::String,
                required: true,
                default: None,
                description: "LogQL metric query (e.g. count_over_time({job=\"nginx\"}[5m]))",
                repeatable: false,
                short: Some('q'),
            }],
        },
    ),
];

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

/// Factory function: build a [`BuiltProvider`] for Loki.
fn build(config: &obz_core::provider::ProviderConfig) -> Result<BuiltProvider, ObzError> {
    let endpoint = config.require_config("endpoint")?;
    validate_endpoint(endpoint)?;
    let basic = config.basic_auth();
    let verbose = config.verbose();
    let timeout = parse_timeout_config(config);
    let provider = LokiProvider::new(
        endpoint,
        config.bearer_token(),
        basic,
        config.custom_headers().clone(),
        verbose,
        timeout,
    )?;
    let ext_provider = provider.clone();
    Ok(BuiltProvider {
        name: "loki",
        metric_query_language: None,
        log_query_language: Some("LogQL"),
        metric: None,
        log: Some(Box::new(provider)),
        trace: None,
        extension: Some(Box::new(ext_provider)),
    })
}

/// Return the [`ProviderMeta`] for Loki.
///
/// Called by [`obz_providers::register_all`] at startup.
pub(crate) fn meta() -> ProviderMeta {
    ProviderMeta {
        name: "loki",
        display_name: "Grafana Loki",
        aliases: &["loki"],
        supported_commands: SupportedCommands {
            metric_query: false,
            metric_list: false,
            metric_info: false,
            metric_labels: false,
            metric_label_values: false,
            metric_series: false,
            log_search: true,
            trace_search: false,
            trace_get: false,
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

    fn dummy_provider() -> LokiProvider {
        let _ = rustls::crypto::ring::default_provider().install_default();
        LokiProvider::new(
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
    async fn extension_label_values_missing_label() {
        let provider = dummy_provider();
        let params = ExtensionParams {
            start: None,
            end: None,
            signal: "log".to_string(),
            args: Vec::new(),
        };

        let result = provider.execute("label-values", &params).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            ObzError::InvalidArgument { code, message, .. } => {
                assert_eq!(code, ErrorCode::MissingRequired);
                assert!(message.contains("--label"));
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
            signal: "log".to_string(),
            args: Vec::new(),
        };

        let result = provider.execute("nonexistent", &params).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            ObzError::InvalidArgument { code, message, .. } => {
                assert_eq!(code, ErrorCode::InvalidFlag);
                assert!(message.contains("nonexistent"));
            }
            other => panic!("expected InvalidArgument, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn extension_stats_missing_query() {
        let provider = dummy_provider();
        let params = ExtensionParams {
            start: None,
            end: None,
            signal: "log".to_string(),
            args: Vec::new(),
        };

        let result = provider.execute("stats", &params).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            ObzError::InvalidArgument { code, message, .. } => {
                assert_eq!(code, ErrorCode::MissingRequired);
                assert!(message.contains("--query"));
            }
            other => panic!("expected InvalidArgument, got {other:?}"),
        }
    }
}
