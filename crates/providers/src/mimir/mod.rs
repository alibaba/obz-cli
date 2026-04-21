//! Grafana Mimir metric provider.
//!
//! Uses the shared [`PromqlMetricProvider`] for Grafana Mimir's
//! Prometheus-compatible HTTP API. This module only contains the
//! registration logic — all six `MetricProvider` methods are implemented
//! in [`crate::promql::provider`].
//!
//! The Grafana `X-Scope-OrgID` multi-tenancy header is supported via
//! `PromqlMetricProvider`'s `extra_headers` mechanism.
//!
//! # API Endpoints
//!
//! | Command | Endpoint |
//! |---------|----------|
//! | `metric query` (instant) | `GET /api/v1/query` |
//! | `metric query` (range) | `GET /api/v1/query_range` |
//! | `metric list` | `GET /api/v1/label/__name__/values` |
//! | `metric info` | `GET /api/v1/metadata` |
//! | `metric labels` | `GET /api/v1/labels` |
//! | `metric label-values` | `GET /api/v1/label/{name}/values` |
//! | `metric series` | `GET /api/v1/series` |

/// Mimir → obz model conversion functions (re-exported from `promql`).
pub(crate) mod convert;
/// Mimir API response deserialization types (re-exported from `promql`).
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

/// Factory function: build a [`BuiltProvider`] for Mimir.
fn build(config: &obz_core::provider::ProviderConfig) -> Result<BuiltProvider, ObzError> {
    let endpoint = config.require_config("endpoint")?;
    validate_endpoint(endpoint)?;
    let basic = config.basic_auth();
    let verbose = config.verbose();
    let timeout = parse_timeout_config(config);

    validate_custom_headers(config.custom_headers())?;
    let extra_headers: Vec<(String, String)> = config
        .custom_headers()
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();

    let client = build_http_client(timeout)?;
    let provider = PromqlMetricProvider::new(
        endpoint,
        client,
        config.bearer_token(),
        basic,
        String::new(),
        extra_headers,
        "Mimir",
        verbose,
    );
    Ok(BuiltProvider {
        name: "mimir",
        metric_query_language: Some("PromQL"),
        log_query_language: None,
        metric: Some(Box::new(provider)),
        log: None,
        trace: None,
        extension: None,
    })
}

/// Return the [`ProviderMeta`] for Mimir.
///
/// Called by [`obz_providers::register_all`] at startup.
pub(crate) fn meta() -> ProviderMeta {
    ProviderMeta {
        name: "mimir",
        display_name: "Grafana Mimir",
        aliases: &["mimir"],
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
        check: Some(|config| {
            Box::pin(crate::probe::http_get_probe(
                config,
                "/api/v1/status/buildinfo",
            ))
        }),
        command_flags: &[],
        extension_commands: &[],
    }
}
