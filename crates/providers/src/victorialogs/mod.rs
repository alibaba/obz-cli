//! `VictoriaLogs` log provider.
//!
//! Implements the [`LogProvider`] trait against the `VictoriaLogs`
//! `LogsQL` query API. The primary endpoint is `/select/logsql/query`
//! which returns NDJSON (`application/stream+json`).
//!
//! # API Endpoints
//!
//! | obz Command | VL Endpoint | Response Format |
//! |-------------|-------------|-----------------|
//! | `log search` | `POST /select/logsql/query` | NDJSON |

/// VL → obz `LogEntry` conversion functions.
pub(crate) mod convert;
/// VL NDJSON response parsing.
pub(crate) mod response;

use async_trait::async_trait;
use reqwest::Client;

use crate::util::{
    apply_custom_headers, apply_standard_auth, build_http_client, http_error, parse_timeout_config,
    send_request, validate_endpoint, HttpResponse,
};
use obz_core::descriptor::{CommandDescriptor, FlagDescriptor, FlagType};
use obz_core::model::error::{ErrorCode, ObzError};
use obz_core::provider::params::ExtensionParams;
use obz_core::provider::results::{ExtensionResult, ProviderResult};
use obz_core::provider::traits::ExtensionProvider;
use obz_core::provider::{LogProvider, LogSearchParams, LogSearchResult};
use obz_core::StandardCommand;

/// `VictoriaLogs` log provider.
#[derive(Clone)]
pub(crate) struct VictoriaLogsProvider {
    /// Base URL (e.g., `http://localhost:9428`).
    base_url: String,
    /// HTTP client with connection pooling.
    client: Client,
    /// Optional bearer token for authentication.
    bearer_token: Option<String>,
    /// Optional basic auth credentials (`username`, `password`).
    basic_auth: Option<(String, String)>,
    /// Optional multi-tenant `AccountID`.
    account_id: Option<String>,
    /// Optional multi-tenant `ProjectID`.
    project_id: Option<String>,
    /// Custom HTTP headers from provider config.
    custom_headers: std::collections::BTreeMap<String, String>,
    /// Whether to print HTTP request/response details to stderr.
    verbose: bool,
}

impl VictoriaLogsProvider {
    /// Create a new `VictoriaLogsProvider`.
    ///
    /// # Arguments
    ///
    /// * `base_url` — Base URL of the `VictoriaLogs` instance.
    /// * `bearer_token` — Optional bearer token for authentication.
    /// * `basic_auth` — Optional basic auth credentials.
    /// * `account_id` — Optional multi-tenant account ID.
    /// * `project_id` — Optional multi-tenant project ID.
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
        bearer_token: Option<String>,
        basic_auth: Option<(String, String)>,
        account_id: Option<String>,
        project_id: Option<String>,
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
            account_id,
            project_id,
            custom_headers,
            verbose,
        })
    }

    /// Build the query URL path, accounting for multi-tenant routing.
    ///
    /// Single-tenant: `/select/logsql/query`
    /// With `AccountID`: `/select/{AccountID}/logsql/query`
    /// With both: `/select/{AccountID}:{ProjectID}/logsql/query`
    fn query_path(&self) -> String {
        match (&self.account_id, &self.project_id) {
            (Some(acc), Some(proj)) => format!("/select/{acc}:{proj}/logsql/query"),
            (Some(acc), None) => format!("/select/{acc}/logsql/query"),
            _ => "/select/logsql/query".to_string(),
        }
    }

    /// Apply authentication to a request builder.
    ///
    /// Bearer token takes precedence over basic auth.
    fn apply_auth(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        apply_standard_auth(req, &self.bearer_token, &self.basic_auth)
    }

    /// Build a POST request for a `LogsQL` endpoint with auth and custom headers.
    fn post_form(&self, path: &str) -> Result<reqwest::RequestBuilder, ObzError> {
        let url = format!("{}{}", self.base_url, self.select_path(path));
        let req = self.client.post(&url);
        let req = self.apply_auth(req);
        apply_custom_headers(req, &self.custom_headers, &[], self.verbose)
    }

    /// Build a select API path with multi-tenant routing preserved.
    fn select_path(&self, suffix: &str) -> String {
        match (&self.account_id, &self.project_id) {
            (Some(acc), Some(proj)) => format!("/select/{acc}:{proj}/logsql/{suffix}"),
            (Some(acc), None) => format!("/select/{acc}/logsql/{suffix}"),
            _ => format!("/select/logsql/{suffix}"),
        }
    }

    /// Send a request and parse a JSON response body.
    async fn send_json<T: serde::de::DeserializeOwned>(
        &self,
        req: reqwest::RequestBuilder,
    ) -> ProviderResult<T> {
        let HttpResponse { status, body } = send_request(&self.client, req, self.verbose).await?;

        if !status.is_success() {
            return Err(http_error(status, &body, "VictoriaLogs"));
        }

        serde_json::from_str(&body).map_err(|e| ObzError::Provider {
            code: ErrorCode::BackendError,
            message: format!("failed to parse VictoriaLogs response: {e}"),
            raw_error: Some(crate::util::truncate_for_error(&body, 500)),
            recoverable: false,
            suggestion: None,
            doc_url: None,
        })
    }

    /// Build common form parameters for extension commands.
    fn extension_form<'a>(
        &'a self,
        params: &'a ExtensionParams,
        query: &'a str,
    ) -> Vec<(&'a str, String)> {
        let mut form = vec![("query", query.to_string())];
        if let Some(start) = params.start {
            form.push(("start", start.to_string()));
        }
        if let Some(end) = params.end {
            form.push(("end", end.to_string()));
        }
        form
    }

    /// List field names.
    async fn list_field_names(&self, params: &ExtensionParams) -> ProviderResult<ExtensionResult> {
        let query = params.get("query").unwrap_or("*");
        let form = self.extension_form(params, query);
        let req = self.post_form("field_names")?.form(&form);
        let resp: response::VlValuesResponse = self.send_json(req).await?;
        let total_count = resp.values.len();
        Ok(ExtensionResult {
            data: serde_json::to_value(resp.values).map_err(|e| ObzError::Provider {
                code: ErrorCode::BackendError,
                message: format!("failed to serialize VictoriaLogs field names: {e}"),
                raw_error: None,
                recoverable: false,
                suggestion: None,
                doc_url: None,
            })?,
            total_count: Some(total_count),
        })
    }

    /// List values for a specific field.
    async fn list_field_values(&self, params: &ExtensionParams) -> ProviderResult<ExtensionResult> {
        let query = params.get("query").unwrap_or("*");
        let field = params.require("field")?;
        let mut form = self.extension_form(params, query);
        form.push(("field", field.to_string()));
        let req = self.post_form("field_values")?.form(&form);
        let resp: response::VlValuesResponse = self.send_json(req).await?;
        let total_count = resp.values.len();
        Ok(ExtensionResult {
            data: serde_json::to_value(resp.values).map_err(|e| ObzError::Provider {
                code: ErrorCode::BackendError,
                message: format!("failed to serialize VictoriaLogs field values: {e}"),
                raw_error: None,
                recoverable: false,
                suggestion: None,
                doc_url: None,
            })?,
            total_count: Some(total_count),
        })
    }

    /// Show log volume distribution over time.
    async fn get_hits(&self, params: &ExtensionParams) -> ProviderResult<ExtensionResult> {
        let query = params.get("query").unwrap_or("*");
        let mut form = self.extension_form(params, query);
        let step = params
            .get("step")
            .filter(|step| !step.is_empty())
            .unwrap_or("1h");
        form.push(("step", step.to_string()));
        let req = self.post_form("hits")?.form(&form);
        let resp: response::VlHitsResponse = self.send_json(req).await?;
        let total_count = resp.hits.len();
        Ok(ExtensionResult {
            data: serde_json::to_value(resp.hits).map_err(|e| ObzError::Provider {
                code: ErrorCode::BackendError,
                message: format!("failed to serialize VictoriaLogs hits response: {e}"),
                raw_error: None,
                recoverable: false,
                suggestion: None,
                doc_url: None,
            })?,
            total_count: Some(total_count),
        })
    }

    /// Run a stats aggregation query.
    async fn get_stats(&self, params: &ExtensionParams) -> ProviderResult<ExtensionResult> {
        let query = params.require("query")?;
        let mut form = self.extension_form(params, query);
        if let Some(step) = params.get("step").filter(|step| !step.is_empty()) {
            form.push(("step", step.to_string()));
        }
        let req = self.post_form("stats_query")?.form(&form);
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
impl LogProvider for VictoriaLogsProvider {
    async fn search(&self, params: &LogSearchParams) -> ProviderResult<LogSearchResult> {
        let url = format!("{}{}", self.base_url, self.query_path());

        let form = vec![
            ("query", params.query.clone()),
            ("start", params.start.to_string()),
            ("end", params.end.to_string()),
            ("limit", params.limit.to_string()),
        ];

        let req = self.client.post(&url).form(&form);
        let req = self.apply_auth(req);
        let req = apply_custom_headers(req, &self.custom_headers, &[], self.verbose)?;

        let HttpResponse { status, body } = send_request(&self.client, req, self.verbose).await?;

        if !status.is_success() {
            return Err(http_error(status, &body, "VictoriaLogs"));
        }

        // Response is NDJSON (application/stream+json).
        let entries = response::parse_ndjson(&body)?;

        Ok(convert::convert_entries(entries))
    }
}

#[async_trait]
impl ExtensionProvider for VictoriaLogsProvider {
    async fn execute(
        &self,
        command: &str,
        params: &ExtensionParams,
    ) -> ProviderResult<ExtensionResult> {
        match command {
            "field-names" => self.list_field_names(params).await,
            "field-values" => self.list_field_values(params).await,
            "hits" => self.get_hits(params).await,
            "stats" => self.get_stats(params).await,
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

use obz_core::registry::{BuiltProvider, ProviderMeta, SupportedCommands};

/// Shared optional query flag for `VictoriaLogs` extension commands.
const VL_QUERY_FLAG: FlagDescriptor = FlagDescriptor {
    name: "query",
    flag_type: FlagType::String,
    required: false,
    default: None,
    description: "LogsQL query filter (defaults to *)",
    repeatable: false,
    short: Some('q'),
};

/// Shared required field flag.
const VL_FIELD_FLAG: FlagDescriptor = FlagDescriptor {
    name: "field",
    flag_type: FlagType::String,
    required: true,
    default: None,
    description: "Field name to get values for",
    repeatable: false,
    short: None,
};

/// Shared optional step flag.
const VL_STEP_FLAG: FlagDescriptor = FlagDescriptor {
    name: "step",
    flag_type: FlagType::String,
    required: false,
    default: None,
    description: "Bucket step duration (e.g. 5m, 1h)",
    repeatable: false,
    short: None,
};

/// Extension commands for `VictoriaLogs`.
static EXTENSION_COMMANDS: &[(&str, CommandDescriptor)] = &[
    (
        "log",
        CommandDescriptor {
            name: "field-names",
            description: "List available field names",
            flags: &[VL_QUERY_FLAG],
        },
    ),
    (
        "log",
        CommandDescriptor {
            name: "field-values",
            description: "List values for a specific field",
            flags: &[VL_QUERY_FLAG, VL_FIELD_FLAG],
        },
    ),
    (
        "log",
        CommandDescriptor {
            name: "hits",
            description: "Show log volume distribution over time",
            flags: &[VL_QUERY_FLAG, VL_STEP_FLAG],
        },
    ),
    (
        "log",
        CommandDescriptor {
            name: "stats",
            description: "Run stats aggregation query",
            flags: &[
                FlagDescriptor {
                    name: "query",
                    flag_type: FlagType::String,
                    required: true,
                    default: None,
                    description: "LogsQL stats query",
                    repeatable: false,
                    short: Some('q'),
                },
                VL_STEP_FLAG,
            ],
        },
    ),
];

/// Factory function: build a [`BuiltProvider`] for `VictoriaLogs`.
fn build(config: &obz_core::provider::ProviderConfig) -> Result<BuiltProvider, ObzError> {
    let endpoint = config.require_config("endpoint")?;
    validate_endpoint(endpoint)?;
    let basic = config.basic_auth();
    let timeout = parse_timeout_config(config);
    let verbose = config.verbose();
    let provider = VictoriaLogsProvider::new(
        endpoint,
        config.bearer_token(),
        basic,
        config.get_owned("account-id"),
        config.get_owned("project-id"),
        config.custom_headers().clone(),
        timeout,
        verbose,
    )?;
    let ext_provider = provider.clone();
    Ok(BuiltProvider {
        name: "victorialogs",
        metric_query_language: None,
        log_query_language: Some("LogsQL"),
        metric: None,
        log: Some(Box::new(provider)),
        trace: None,
        extension: Some(Box::new(ext_provider)),
    })
}

/// Return the [`ProviderMeta`] for `VictoriaLogs`.
///
/// Called by [`obz_providers::register_all`] at startup.
pub(crate) fn meta() -> ProviderMeta {
    ProviderMeta {
        name: "victorialogs",
        display_name: "VictoriaLogs",
        aliases: &["vl", "victorialogs"],
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
        check: Some(|config| Box::pin(crate::probe::http_get_probe(config, "/health"))),
        command_flags: &[(
            StandardCommand::LogSearch,
            &[
                FlagDescriptor {
                    name: "account-id",
                    flag_type: FlagType::String,
                    required: false,
                    default: None,
                    description: "Multi-tenant AccountID",
                    repeatable: false,
                    short: None,
                },
                FlagDescriptor {
                    name: "project-id",
                    flag_type: FlagType::String,
                    required: false,
                    default: None,
                    description: "Multi-tenant ProjectID",
                    repeatable: false,
                    short: None,
                },
            ],
        )],
        extension_commands: EXTENSION_COMMANDS,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_provider() -> VictoriaLogsProvider {
        let _ = rustls::crypto::ring::default_provider().install_default();
        VictoriaLogsProvider::new(
            "http://localhost:0",
            None,
            None,
            None,
            None,
            std::collections::BTreeMap::new(),
            None,
            false,
        )
        .unwrap()
    }

    #[tokio::test]
    async fn extension_field_values_missing_field() {
        let provider = dummy_provider();
        let params = ExtensionParams {
            start: None,
            end: None,
            signal: "log".to_string(),
            args: Vec::new(),
        };

        let result = provider.execute("field-values", &params).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            ObzError::InvalidArgument { code, message, .. } => {
                assert_eq!(code, ErrorCode::MissingRequired);
                assert!(message.contains("--field"));
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

    #[test]
    fn skips_empty_step_for_hits_form() {
        let provider = dummy_provider();
        let params = ExtensionParams {
            start: None,
            end: None,
            signal: "log".to_string(),
            args: vec![
                ("query".to_string(), "*".to_string()),
                ("step".to_string(), String::new()),
            ],
        };

        let mut form = provider.extension_form(&params, "*");
        if let Some(step) = params.get("step").filter(|step| !step.is_empty()) {
            form.push(("step", step.to_string()));
        }

        assert!(!form.iter().any(|(key, _)| *key == "step"));
    }
}
