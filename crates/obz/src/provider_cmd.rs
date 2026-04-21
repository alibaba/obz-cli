//! Runtime logic for `obz provider` management commands.

use std::path::Path;
use std::process::ExitCode;
use std::time::Duration;

use comfy_table::presets::UTF8_FULL_CONDENSED;
use comfy_table::{ContentArrangement, Table};
use futures::future::join_all;
use obz_core::provider::ProviderConfig;
use obz_core::registry::{CheckScope, CheckSeverity, ProviderMeta, ProviderRegistry};

use crate::config::ObzConfig;

/// Summary severity used by `provider check` output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CheckReportSeverity {
    Pass,
    Warn,
    Fail,
    Skip,
}

impl CheckReportSeverity {
    fn symbol(self) -> &'static str {
        match self {
            Self::Pass => "✓",
            Self::Warn => "⚠",
            Self::Fail => "✗",
            Self::Skip => "-",
        }
    }
}

/// Structured output for checking a single configured provider.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ProviderCheckReport {
    configured_name: String,
    provider_display_name: String,
    severity: CheckReportSeverity,
    message: String,
    latency: Option<Duration>,
    endpoint: Option<String>,
    auth_detail: Option<String>,
}

fn supports_metric(meta: &ProviderMeta) -> bool {
    let sc = &meta.supported_commands;
    sc.metric_query
        || sc.metric_list
        || sc.metric_info
        || sc.metric_labels
        || sc.metric_label_values
        || sc.metric_series
}

fn supports_log(meta: &ProviderMeta) -> bool {
    meta.supported_commands.log_search
}

fn supports_trace(meta: &ProviderMeta) -> bool {
    let sc = &meta.supported_commands;
    sc.trace_search || sc.trace_get
}

fn render_check_summary(reports: &[ProviderCheckReport]) -> String {
    if reports.is_empty() {
        return String::new();
    }

    let name_width = reports
        .iter()
        .map(|report| report.configured_name.len())
        .max()
        .unwrap_or(10)
        .max(10);
    let display_width = reports
        .iter()
        .map(|report| report.provider_display_name.len())
        .max()
        .unwrap_or(17)
        .max(17);

    let mut lines = reports
        .iter()
        .map(|report| {
            let latency = report
                .latency
                .map(|duration| format!(" ({}ms)", duration.as_millis()))
                .unwrap_or_default();
            let auth_detail = report
                .auth_detail
                .as_deref()
                .map(|detail| format!(" [{detail}]"))
                .unwrap_or_default();
            format!(
                "{} {:<name_width$} {:<display_width$} {}{}{}",
                report.severity.symbol(),
                report.configured_name,
                report.provider_display_name,
                report.message,
                latency,
                auth_detail,
            )
        })
        .collect::<Vec<_>>();

    let failed_reports: Vec<_> = reports
        .iter()
        .filter(|report| matches!(report.severity, CheckReportSeverity::Fail))
        .collect();
    let failed = failed_reports.len();
    let warnings = reports
        .iter()
        .filter(|report| matches!(report.severity, CheckReportSeverity::Warn))
        .count();
    let passed = reports
        .iter()
        .filter(|report| matches!(report.severity, CheckReportSeverity::Pass))
        .count();

    let mut stats = Vec::new();
    if failed > 0 {
        stats.push(format!(
            "{failed} failure{}",
            if failed > 1 { "s" } else { "" }
        ));
    }
    if warnings > 0 {
        stats.push(format!(
            "{warnings} warning{}",
            if warnings > 1 { "s" } else { "" }
        ));
    }
    if passed > 0 && (failed > 0 || warnings > 0) {
        stats.push(format!("{passed} passed"));
    }

    if !stats.is_empty() {
        lines.push(String::new());
        lines.push(stats.join(", "));
    }

    if !failed_reports.is_empty() {
        lines.push(String::new());
        lines.push("Failures:".to_string());
        for report in failed_reports {
            lines.push(String::new());
            lines.push(format!(
                "  {} ({})",
                report.configured_name, report.provider_display_name
            ));
            if let Some(endpoint) = &report.endpoint {
                lines.push(format!("    Endpoint: {endpoint}"));
            }
            lines.push(format!("    Error:    {}", report.message));
            if let Some(auth_detail) = &report.auth_detail {
                lines.push(format!("    Auth:     {auth_detail}"));
            }
        }
    }

    format!("{}\n", lines.join("\n"))
}

fn auth_description(config: &ProviderConfig) -> &'static str {
    if config.auth_get("api-key").is_some() || config.auth_get("app-key").is_some() {
        "api key headers"
    } else if config.auth_get("token").is_some() {
        "bearer token"
    } else if config.basic_auth().is_some() {
        "basic auth"
    } else if config.auth_get("access-key-id").is_some()
        || config.auth_get("access-key-secret").is_some()
    {
        "provider-specific credentials"
    } else {
        "none"
    }
}

async fn check_provider(
    registry: &ProviderRegistry,
    config: &ObzConfig,
    name: &str,
    config_dir: &Path,
) -> ProviderCheckReport {
    let provider_type = config.provider_type(name).to_string();

    let meta = match registry.get(&provider_type) {
        Ok(meta) => meta,
        Err(error) => {
            return ProviderCheckReport {
                configured_name: name.to_string(),
                provider_display_name: provider_type,
                severity: CheckReportSeverity::Fail,
                message: error.to_string(),
                latency: None,
                endpoint: None,
                auth_detail: None,
            };
        }
    };

    let provider_config = match config.resolve_with_dir(name, Some(config_dir)) {
        Ok(Some(provider_config)) => provider_config,
        Ok(None) => {
            return ProviderCheckReport {
                configured_name: name.to_string(),
                provider_display_name: meta.display_name.to_string(),
                severity: CheckReportSeverity::Fail,
                message: "provider is not configured".to_string(),
                latency: None,
                endpoint: None,
                auth_detail: None,
            };
        }
        Err(error) => {
            return ProviderCheckReport {
                configured_name: name.to_string(),
                provider_display_name: meta.display_name.to_string(),
                severity: CheckReportSeverity::Fail,
                message: error.to_string(),
                latency: None,
                endpoint: None,
                auth_detail: None,
            };
        }
    };

    let endpoint = provider_config.get("endpoint").map(str::to_string);

    if let Err(error) = (meta.build)(&provider_config) {
        return ProviderCheckReport {
            configured_name: name.to_string(),
            provider_display_name: meta.display_name.to_string(),
            severity: CheckReportSeverity::Fail,
            message: error.to_string(),
            latency: None,
            endpoint,
            auth_detail: None,
        };
    }

    match meta.check {
        Some(check_fn) => {
            let result = check_fn(&provider_config).await;
            let severity = match result.severity {
                CheckSeverity::Ok => CheckReportSeverity::Pass,
                CheckSeverity::Warn => CheckReportSeverity::Warn,
                CheckSeverity::Fail => CheckReportSeverity::Fail,
            };

            let auth = auth_description(&provider_config);
            let auth_detail = match (&result.scope, &result.severity) {
                (CheckScope::ConnectivityAndAuth, CheckSeverity::Ok) => None,
                (CheckScope::ConnectivityAndAuth, _) => {
                    if auth == "none" {
                        Some("authentication check failed".to_string())
                    } else {
                        Some(format!("{auth} rejected"))
                    }
                }
                (CheckScope::ConfiguredNotVerifiable, _) => {
                    Some(format!("{auth} configured (not verified by check)"))
                }
                (CheckScope::Connectivity, _) => None,
            };

            ProviderCheckReport {
                configured_name: name.to_string(),
                provider_display_name: meta.display_name.to_string(),
                severity,
                message: result.message,
                latency: result.latency,
                endpoint,
                auth_detail,
            }
        }
        None => ProviderCheckReport {
            configured_name: name.to_string(),
            provider_display_name: meta.display_name.to_string(),
            severity: CheckReportSeverity::Skip,
            message: "check not available".to_string(),
            latency: None,
            endpoint,
            auth_detail: None,
        },
    }
}

/// Check configured providers with a lightweight live probe.
pub(crate) async fn check_providers(
    registry: &ProviderRegistry,
    config: &ObzConfig,
    name: Option<&str>,
    config_dir: &Path,
) -> ExitCode {
    let provider_names = match name {
        Some(name) => {
            if config.provider_names().contains(&name) {
                vec![name]
            } else {
                let report = ProviderCheckReport {
                    configured_name: name.to_string(),
                    provider_display_name: "unknown provider".to_string(),
                    severity: CheckReportSeverity::Fail,
                    message: "provider is not configured".to_string(),
                    latency: None,
                    endpoint: None,
                    auth_detail: None,
                };
                eprint!("{}", render_check_summary(&[report]));
                return ExitCode::from(1);
            }
        }
        None => {
            let names = config.provider_names();
            if names.is_empty() {
                eprintln!("No configured providers found.");
                eprintln!();
                eprintln!(
                    "To get started, add a provider to {}/config.yaml:",
                    config_dir.display()
                );
                eprintln!();
                eprintln!("  providers:");
                eprintln!("    vm:");
                eprintln!("      endpoint: http://localhost:8428");
                eprintln!();
                eprintln!("Run 'obz provider list' to see all available providers.");
                eprintln!("Run 'obz skills list' to find configuration guides for each provider.");
                return ExitCode::from(1);
            }
            names
        }
    };

    let reports = join_all(
        provider_names
            .into_iter()
            .map(|provider_name| check_provider(registry, config, provider_name, config_dir)),
    )
    .await;

    print!("{}", render_check_summary(&reports));

    if reports
        .iter()
        .all(|report| !matches!(report.severity, CheckReportSeverity::Fail))
    {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

/// Print a table of all registered providers to stdout.
///
/// Called from `main.rs` when `obz provider list` is matched, before
/// provider resolution — no `--provider` flag is needed.
pub(crate) fn list_providers(registry: &ProviderRegistry, config: &ObzConfig, config_dir: &Path) {
    print!("{}", render_provider_table(registry, config, config_dir));
}

/// Render the provider table as a `String`.
///
/// Extracted from [`list_providers`] so the output can be asserted in tests
/// without capturing stdout.
///
/// Uses [`comfy_table`] with the `UTF8_FULL_CONDENSED` preset so column
/// widths adapt to content length automatically, matching the table style
/// used by `obz ... -o table` output.
///
/// ```text
/// ╭──────┬─────────────────┬────────────────────────┬─────────┬────────┬─────┬───────╮
/// │ Name │ Provider        │ Endpoint               │ Default │ Metric │ Log │ Trace │
/// ╞══════╪═════════════════╪════════════════════════╪═════════╪════════╪═════╪═══════╡
/// │ vm   │ VictoriaMetrics │ http://localhost:8428  │ metric  │ ✓      │     │       │
/// ╰──────┴─────────────────┴────────────────────────┴─────────┴────────┴─────┴───────╯
/// ```
fn render_provider_table(
    registry: &ProviderRegistry,
    config: &ObzConfig,
    config_dir: &Path,
) -> String {
    fn default_signals(config: &ObzConfig, names: &[&str]) -> String {
        let mut signals = Vec::new();
        if let Some(dm) = config.default_metric() {
            if names.contains(&dm) {
                signals.push("metric");
            }
        }
        if let Some(dl) = config.default_log() {
            if names.contains(&dl) {
                signals.push("log");
            }
        }
        if let Some(dt) = config.default_trace() {
            if names.contains(&dt) {
                signals.push("trace");
            }
        }
        signals.join(",")
    }

    fn capability_cell(supported: bool, available: Option<bool>) -> &'static str {
        match (supported, available) {
            (_, Some(true)) => "✓",
            (true, Some(false)) => "!",
            (false, Some(false)) => "✗",
            _ => "",
        }
    }

    fn config_footer(config_dir: &Path) -> String {
        if config_dir.is_dir() {
            format!("Config: {}", config_dir.display())
        } else {
            "Config: (no config directory found)".to_string()
        }
    }

    fn defaults_footer(config: &ObzConfig) -> String {
        let mut parts = Vec::new();
        if let Some(provider) = config.default_metric() {
            parts.push(format!("metric={provider}"));
        }
        if let Some(provider) = config.default_log() {
            parts.push(format!("log={provider}"));
        }
        if let Some(provider) = config.default_trace() {
            parts.push(format!("trace={provider}"));
        }

        if parts.is_empty() {
            "Defaults: (none)".to_string()
        } else {
            format!("Defaults: {}", parts.join(", "))
        }
    }

    let mut table = Table::new();
    table
        .load_preset(UTF8_FULL_CONDENSED)
        .set_content_arrangement(ContentArrangement::Dynamic)
        .set_header([
            "Name", "Provider", "Endpoint", "Default", "Metric", "Log", "Trace",
        ]);

    for name in config.provider_names() {
        let provider_type = config.provider_type(name);
        let default = default_signals(config, &[name]);

        match registry.get(provider_type) {
            Ok(meta) => match config.resolve_with_dir(name, Some(config_dir)) {
                Ok(Some(provider_config)) => match (meta.build)(&provider_config) {
                    Ok(built) => {
                        let endpoint = provider_config.get("endpoint").unwrap_or("(configured)");
                        let metric =
                            capability_cell(supports_metric(meta), Some(built.metric.is_some()));
                        let log = capability_cell(supports_log(meta), Some(built.log.is_some()));
                        let trace =
                            capability_cell(supports_trace(meta), Some(built.trace.is_some()));

                        table.add_row([
                            name,
                            meta.display_name,
                            endpoint,
                            &default,
                            metric,
                            log,
                            trace,
                        ]);
                    }
                    Err(error) => {
                        let error = error.to_string();
                        let metric = capability_cell(supports_metric(meta), Some(false));
                        let log = capability_cell(supports_log(meta), Some(false));
                        let trace = capability_cell(supports_trace(meta), Some(false));

                        table.add_row([
                            name,
                            meta.display_name,
                            error.as_str(),
                            &default,
                            metric,
                            log,
                            trace,
                        ]);
                    }
                },
                Ok(None) => {
                    table.add_row([
                        name,
                        meta.display_name,
                        "(not configured)",
                        &default,
                        "",
                        "",
                        "",
                    ]);
                }
                Err(error) => {
                    let error = error.to_string();
                    let metric = capability_cell(supports_metric(meta), Some(false));
                    let log = capability_cell(supports_log(meta), Some(false));
                    let trace = capability_cell(supports_trace(meta), Some(false));

                    table.add_row([
                        name,
                        meta.display_name,
                        error.as_str(),
                        &default,
                        metric,
                        log,
                        trace,
                    ]);
                }
            },
            Err(error) => {
                let error = error.to_string();
                table.add_row([name, provider_type, error.as_str(), &default, "", "", ""]);
            }
        }
    }

    let configured_types: Vec<&str> = config
        .provider_names()
        .iter()
        .map(|name| config.provider_type(name))
        .collect();

    for meta in registry.all() {
        let dominated = meta
            .aliases
            .iter()
            .chain(std::iter::once(&meta.name))
            .any(|a| configured_types.contains(a) || config.provider_names().contains(a));
        if dominated {
            continue;
        }

        let metric = capability_cell(supports_metric(meta), None);
        let log = capability_cell(supports_log(meta), None);
        let trace = capability_cell(supports_trace(meta), None);
        let name = meta.aliases.first().copied().unwrap_or(meta.name);
        let mut all_names: Vec<&str> = meta.aliases.to_vec();
        if !all_names.contains(&meta.name) {
            all_names.push(meta.name);
        }
        let default = default_signals(config, &all_names);

        table.add_row([
            name,
            meta.display_name,
            "(not configured)",
            &default,
            metric,
            log,
            trace,
        ]);
    }

    let has_configured = !config.provider_names().is_empty();
    let legend = if has_configured {
        "\n✓ configured  ! misconfigured  ✗ not supported\n"
    } else {
        "\n"
    };

    let mut footer = format!(
        "{table}{legend}{}\n{}\n",
        defaults_footer(config),
        config_footer(config_dir)
    );

    if !has_configured {
        footer.push_str(
            "Tip: run 'obz skills show <skill-name>' for configuration examples \
             (e.g. 'obz skills show obz-vm')\n",
        );
    }

    footer
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config;
    use obz_core::registry::{CheckResult, SupportedCommands};
    use obz_core::BuiltProvider;
    use obz_core::{ErrorCode, ObzError};

    fn metric_provider() -> Box<dyn obz_core::provider::MetricProvider> {
        struct TestMetricProvider;

        #[async_trait::async_trait]
        impl obz_core::provider::MetricProvider for TestMetricProvider {
            async fn query(
                &self,
                _: &obz_core::provider::MetricQueryParams,
            ) -> obz_core::provider::ProviderResult<obz_core::provider::MetricQueryResult>
            {
                unreachable!("not used in tests")
            }

            async fn list(
                &self,
                _: &obz_core::provider::MetricMetadataParams,
            ) -> obz_core::provider::ProviderResult<Vec<String>> {
                unreachable!("not used in tests")
            }

            async fn info(
                &self,
                _: &obz_core::provider::MetricInfoParams,
            ) -> obz_core::provider::ProviderResult<Vec<obz_core::model::metric::MetricInfoDetail>>
            {
                unreachable!("not used in tests")
            }

            async fn labels(
                &self,
                _: &obz_core::provider::MetricMetadataParams,
            ) -> obz_core::provider::ProviderResult<Vec<String>> {
                unreachable!("not used in tests")
            }

            async fn label_values(
                &self,
                _: &obz_core::provider::LabelValuesParams,
            ) -> obz_core::provider::ProviderResult<Vec<String>> {
                unreachable!("not used in tests")
            }

            async fn series(
                &self,
                _: &obz_core::provider::MetricMetadataParams,
            ) -> obz_core::provider::ProviderResult<Vec<std::collections::BTreeMap<String, String>>>
            {
                unreachable!("not used in tests")
            }
        }

        Box::new(TestMetricProvider)
    }

    fn log_provider() -> Box<dyn obz_core::provider::LogProvider> {
        struct TestLogProvider;

        #[async_trait::async_trait]
        impl obz_core::provider::LogProvider for TestLogProvider {
            async fn search(
                &self,
                _: &obz_core::provider::LogSearchParams,
            ) -> obz_core::provider::ProviderResult<obz_core::provider::LogSearchResult>
            {
                unreachable!("not used in tests")
            }
        }

        Box::new(TestLogProvider)
    }

    fn trace_provider() -> Box<dyn obz_core::provider::TraceProvider> {
        struct TestTraceProvider;

        #[async_trait::async_trait]
        impl obz_core::provider::TraceProvider for TestTraceProvider {
            async fn search(
                &self,
                _: &obz_core::provider::TraceSearchParams,
            ) -> obz_core::provider::ProviderResult<obz_core::provider::TraceSearchResult>
            {
                unreachable!("not used in tests")
            }

            async fn get_trace(
                &self,
                _: &obz_core::provider::TraceGetParams,
            ) -> obz_core::provider::ProviderResult<obz_core::model::trace::TraceDetail>
            {
                unreachable!("not used in tests")
            }
        }

        Box::new(TestTraceProvider)
    }

    fn load_test_config(config_yaml: &str) -> (tempfile::TempDir, config::ObzConfig) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.yaml"), config_yaml).unwrap();
        let cfg = config::load(dir.path()).unwrap();
        (dir, cfg)
    }

    #[test]
    fn test_render_provider_table_contains_all_providers() {
        fn dummy_build(_: &ProviderConfig) -> Result<BuiltProvider, ObzError> {
            Ok(BuiltProvider {
                name: "dummy",
                metric_query_language: None,
                log_query_language: None,
                metric: None,
                log: None,
                trace: None,
                extension: None,
            })
        }

        let mut registry = ProviderRegistry::new();
        registry.register(ProviderMeta {
            name: "myprovider",
            display_name: "My Provider",
            aliases: &["mp", "myprovider"],
            supported_commands: SupportedCommands {
                metric_query: true,
                log_search: true,
                trace_search: false,
                trace_get: false,
                ..SupportedCommands::default()
            },
            build: dummy_build,
            check: None,
            command_flags: &[],
            extension_commands: &[],
        });
        registry.register(ProviderMeta {
            name: "trace-only",
            display_name: "Trace Only",
            aliases: &["to"],
            supported_commands: SupportedCommands {
                trace_search: true,
                trace_get: true,
                ..SupportedCommands::default()
            },
            build: dummy_build,
            check: None,
            command_flags: &[],
            extension_commands: &[],
        });

        let config = config::ObzConfig::empty();
        let config_dir = Path::new("/missing-config");
        let output = render_provider_table(&registry, &config, config_dir);

        assert!(output.contains("Name"), "missing header 'Name'");
        assert!(output.contains("Provider"), "missing header 'Provider'");
        assert!(output.contains("Endpoint"), "missing header 'Endpoint'");
        assert!(output.contains("Default"), "missing header 'Default'");
        assert!(output.contains("Metric"), "missing header 'Metric'");
        assert!(output.contains("Log"), "missing header 'Log'");
        assert!(output.contains("Trace"), "missing header 'Trace'");

        assert!(output.contains("My Provider"), "missing 'My Provider'");
        assert!(output.contains("Trace Only"), "missing 'Trace Only'");

        assert!(
            output.contains("mp"),
            "missing alias row name for myprovider"
        );
        assert!(
            output.contains("to"),
            "missing alias row name for trace-only"
        );
        assert!(output.contains("(not configured)"));
        assert!(output.contains("Defaults: (none)"));
        assert!(output.contains("Config: (no config directory found)"));

        assert!(
            !output.contains("✓ configured"),
            "legend should not appear when no providers are configured"
        );
        assert!(
            output.contains("obz skills show"),
            "should show skills tip when no providers are configured"
        );
    }

    #[test]
    fn test_render_provider_table_shows_configured_provider_build_success() {
        fn build_vm(config: &ProviderConfig) -> Result<BuiltProvider, ObzError> {
            let _ = config.require_config("endpoint")?;
            Ok(BuiltProvider {
                name: "vm",
                metric_query_language: None,
                log_query_language: None,
                metric: Some(metric_provider()),
                log: None,
                trace: None,
                extension: None,
            })
        }

        let mut registry = ProviderRegistry::new();
        registry.register(ProviderMeta {
            name: "vm",
            display_name: "VictoriaMetrics",
            aliases: &["vm"],
            supported_commands: SupportedCommands {
                metric_query: true,
                ..SupportedCommands::default()
            },
            build: build_vm,
            check: None,
            command_flags: &[],
            extension_commands: &[],
        });

        let (config_dir, config) = load_test_config(
            r#"
defaults:
  metric: vm
providers:
  vm:
    endpoint: http://localhost:8428
"#,
        );

        let output = render_provider_table(&registry, &config, config_dir.path());

        assert!(output.contains("VictoriaMetrics"));
        assert!(output.contains("http://localhost:8428"));
        assert!(output.contains("metric"));
        assert!(output.contains('✓'));
        assert!(output.contains("Defaults: metric=vm"));
        assert!(output.contains(&format!("Config: {}", config_dir.path().display())));
        assert!(
            output.contains("✓ configured"),
            "legend should appear when providers are configured"
        );
        assert!(
            !output.contains("obz skills show"),
            "skills tip should not appear when providers are configured"
        );
    }

    #[test]
    fn test_render_provider_table_shows_build_failure_reason() {
        fn build_requires_endpoint(config: &ProviderConfig) -> Result<BuiltProvider, ObzError> {
            let _ = config.require_config("endpoint")?;
            Ok(BuiltProvider {
                name: "broken",
                metric_query_language: None,
                log_query_language: None,
                metric: Some(metric_provider()),
                log: Some(log_provider()),
                trace: None,
                extension: None,
            })
        }

        let mut registry = ProviderRegistry::new();
        registry.register(ProviderMeta {
            name: "broken",
            display_name: "Broken Provider",
            aliases: &["broken"],
            supported_commands: SupportedCommands {
                metric_query: true,
                log_search: true,
                ..SupportedCommands::default()
            },
            build: build_requires_endpoint,
            check: None,
            command_flags: &[],
            extension_commands: &[],
        });

        let (config_dir, config) = load_test_config(
            r#"
providers:
  broken:
    token: secret
"#,
        );

        let output = render_provider_table(&registry, &config, config_dir.path());

        assert!(output.contains("Broken Provider"));
        assert!(output.contains("--endpoint is required"));
        assert!(output.contains('!'), "misconfigured signals should show !");
    }

    #[test]
    fn test_render_provider_table_mixes_configured_and_unconfigured_providers() {
        fn build_sls(config: &ProviderConfig) -> Result<BuiltProvider, ObzError> {
            let _ = config.require_config("endpoint")?;
            Ok(BuiltProvider {
                name: "sls",
                metric_query_language: None,
                log_query_language: None,
                metric: None,
                log: Some(log_provider()),
                trace: Some(trace_provider()),
                extension: None,
            })
        }

        fn build_vm(config: &ProviderConfig) -> Result<BuiltProvider, ObzError> {
            let _ = config.require_config("endpoint")?;
            Ok(BuiltProvider {
                name: "vm",
                metric_query_language: None,
                log_query_language: None,
                metric: Some(metric_provider()),
                log: None,
                trace: None,
                extension: None,
            })
        }

        let mut registry = ProviderRegistry::new();
        registry.register(ProviderMeta {
            name: "sls",
            display_name: "Simple Log Service",
            aliases: &["sls"],
            supported_commands: SupportedCommands {
                log_search: true,
                trace_search: true,
                ..SupportedCommands::default()
            },
            build: build_sls,
            check: None,
            command_flags: &[],
            extension_commands: &[],
        });
        registry.register(ProviderMeta {
            name: "vm",
            display_name: "VictoriaMetrics",
            aliases: &["vm"],
            supported_commands: SupportedCommands {
                metric_query: true,
                ..SupportedCommands::default()
            },
            build: build_vm,
            check: None,
            command_flags: &[],
            extension_commands: &[],
        });

        let (config_dir, config) = load_test_config(
            r#"
defaults:
  log: app-logs
  trace: app-logs
providers:
  app-logs:
    provider: sls
    endpoint: http://sls.example.com
"#,
        );

        let output = render_provider_table(&registry, &config, config_dir.path());

        assert!(output.contains("app-logs"));
        assert!(output.contains("Simple Log Service"));
        assert!(output.contains("log,trace"));
        assert!(output.contains("VictoriaMetrics"));
        assert!(output.contains("(not configured)"));
        assert!(output.contains("Defaults: log=app-logs, trace=app-logs"));
    }

    #[test]
    fn test_render_provider_table_shows_resolve_failure_reason() {
        fn build_trace_fail(config: &ProviderConfig) -> Result<BuiltProvider, ObzError> {
            config.require_config("endpoint")?;
            Err(ObzError::InvalidArgument {
                code: ErrorCode::MissingRequired,
                message: "build failed".to_string(),
                suggestion: None,
            })
        }

        let mut registry = ProviderRegistry::new();
        registry.register(ProviderMeta {
            name: "tempo",
            display_name: "Tempo",
            aliases: &["tempo"],
            supported_commands: SupportedCommands {
                trace_search: true,
                ..SupportedCommands::default()
            },
            build: build_trace_fail,
            check: None,
            command_flags: &[],
            extension_commands: &[],
        });

        let (config_dir, config) = load_test_config(
            r#"
providers:
  prod-trace:
    provider: tempo
    endpoint: http://tempo.example.com
"#,
        );

        let output = render_provider_table(&registry, &config, config_dir.path());

        assert!(output.contains("prod-trace"));
        assert!(output.contains("build failed"));
        assert!(
            output.contains('!'),
            "supported but failed signals should show !"
        );
    }

    #[test]
    fn test_render_check_reports_shows_build_failure() {
        let mut registry = ProviderRegistry::new();
        registry.register(ProviderMeta {
            name: "broken",
            display_name: "Broken Provider",
            aliases: &["broken"],
            supported_commands: SupportedCommands {
                metric_query: true,
                ..SupportedCommands::default()
            },
            build: |config: &ProviderConfig| {
                let _ = config.require_config("endpoint")?;
                Err(ObzError::InvalidArgument {
                    code: ErrorCode::MissingRequired,
                    message: "--api-key is required".to_string(),
                    suggestion: None,
                })
            },
            check: None,
            command_flags: &[],
            extension_commands: &[],
        });

        let (config_dir, config) = load_test_config(
            r#"
providers:
  broken:
    endpoint: http://example.com
"#,
        );

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let report = runtime.block_on(check_provider(
            &registry,
            &config,
            "broken",
            config_dir.path(),
        ));
        let output = render_check_summary(std::slice::from_ref(&report));

        assert_eq!(report.severity, CheckReportSeverity::Fail);
        assert!(output.contains("✗ broken"));
        assert!(output.contains("Broken Provider"));
        assert!(output.contains("1 failure"));
        assert!(output.contains("Failures:"));
        assert!(output.contains("  broken (Broken Provider)"));
        assert!(output.contains("    Endpoint: http://example.com"));
        assert!(output.contains("    Error:"));
        assert!(output.contains("--api-key is required"));
    }

    #[test]
    fn test_render_check_reports_shows_configured_single_provider() {
        let report = ProviderCheckReport {
            configured_name: "vm".to_string(),
            provider_display_name: "VictoriaMetrics".to_string(),
            severity: CheckReportSeverity::Pass,
            message: "reachable".to_string(),
            latency: Some(Duration::from_millis(12)),
            endpoint: Some("http://localhost:8428".to_string()),
            auth_detail: None,
        };
        let output = render_check_summary(&[report]);

        assert!(output.contains("✓ vm"));
        assert!(output.contains("VictoriaMetrics"));
        assert!(output.contains("reachable (12ms)"));
        assert!(!output.contains("Failures:"));
        assert!(!output.contains("failed"));
        assert!(!output.contains("warning"));
    }

    #[test]
    fn test_render_check_reports_shows_failed_auth_detail() {
        let report = ProviderCheckReport {
            configured_name: "dd".to_string(),
            provider_display_name: "Datadog".to_string(),
            severity: CheckReportSeverity::Fail,
            message: "invalid api key".to_string(),
            latency: Some(Duration::from_millis(367)),
            endpoint: Some("https://api.ap1.datadoghq.com".to_string()),
            auth_detail: Some("api key headers rejected".to_string()),
        };

        let output = render_check_summary(&[report]);

        assert!(output.contains("✗ dd"));
        assert!(output.contains("Datadog"));
        assert!(output.contains("invalid api key (367ms)"));
        assert!(output.contains("Failures:"));
        assert!(output.contains("    Endpoint: https://api.ap1.datadoghq.com"));
        assert!(output.contains("    Error:    invalid api key"));
        assert!(output.contains("    Auth:     api key headers rejected"));
    }

    #[test]
    fn test_check_provider_shows_not_verifiable_auth_detail() {
        let mut registry = ProviderRegistry::new();
        registry.register(ProviderMeta {
            name: "sls",
            display_name: "Simple Log Service",
            aliases: &["sls"],
            supported_commands: SupportedCommands::default(),
            build: |config: &ProviderConfig| {
                let _ = config.require_config("endpoint")?;
                Ok(BuiltProvider {
                    name: "sls",
                    metric_query_language: None,
                    log_query_language: None,
                    metric: None,
                    log: None,
                    trace: None,
                    extension: None,
                })
            },
            check: Some(|_: &ProviderConfig| {
                Box::pin(async {
                    CheckResult {
                        severity: CheckSeverity::Warn,
                        scope: CheckScope::ConfiguredNotVerifiable,
                        message: "reachable".to_string(),
                        latency: None,
                    }
                })
            }),
            command_flags: &[],
            extension_commands: &[],
        });

        let (config_dir, config) = load_test_config(
            r#"
providers:
  sls:
    endpoint: http://sls.example.com
    auth:
      access-key-id: ak
      access-key-secret: sk
"#,
        );

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let report = runtime.block_on(check_provider(&registry, &config, "sls", config_dir.path()));

        assert_eq!(report.severity, CheckReportSeverity::Warn);
        assert_eq!(
            report.auth_detail.as_deref(),
            Some("provider-specific credentials configured (not verified by check)")
        );
    }
}
