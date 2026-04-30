//! `GreptimeDB` metric provider.
//!
//! `GreptimeDB` exposes a Prometheus-compatible HTTP API under the
//! `/v1/prometheus` path prefix. This module reuses the shared
//! [`PromqlMetricProvider`] — all six `MetricProvider` methods are
//! implemented in [`crate::promql::provider`].
//!
//! # Default database
//!
//! `GreptimeDB` routes queries to the `public` database by default.
//! To target a different database, append `?db=<name>` directly in
//! your queries or set a custom `db` query parameter via the `db`
//! config key (passed as an extra header workaround is not needed —
//! the default works for most deployments).
//!
//! # API Endpoints
//!
//! | Command                | Endpoint                                          |
//! |------------------------|---------------------------------------------------|
//! | `metric query` (instant)  | `GET /v1/prometheus/api/v1/query`              |
//! | `metric query` (range)    | `GET /v1/prometheus/api/v1/query_range`        |
//! | `metric list`             | `GET /v1/prometheus/api/v1/label/__name__/values` |
//! | `metric info`             | `GET /v1/prometheus/api/v1/metadata`           |
//! | `metric labels`           | `GET /v1/prometheus/api/v1/labels`             |
//! | `metric label-values`     | `GET /v1/prometheus/api/v1/label/{name}/values` |
//! | `metric series`           | `GET /v1/prometheus/api/v1/series`             |

/// `GreptimeDB` → obz model conversion functions (re-exported from `promql`).
pub(crate) mod convert;
/// `GreptimeDB` API response deserialization types (re-exported from `promql`).
pub(crate) mod response;

use crate::promql::provider::PromqlMetricProvider;
use crate::util::{
    build_http_client, parse_timeout_config, validate_custom_headers, validate_endpoint,
};
use obz_core::model::error::ObzError;
use obz_core::registry::{BuiltProvider, ProviderMeta, SupportedCommands};

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

    let provider = PromqlMetricProvider::new(
        endpoint,
        client,
        config.bearer_token(),
        basic,
        "/v1/prometheus".to_string(),
        extra_headers,
        "GreptimeDB",
        verbose,
    );
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
            metric_info: true,
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
