//! Dispatch and argument extraction for the obz CLI.
//!
//! After clap parses the command line, this module resolves the selected
//! provider, collects global and provider-specific flags into a
//! [`ProviderConfig`], and routes to the appropriate core execute function.

use std::path::Path;

use clap::ArgMatches;
use obz_core::descriptor::FlagType;
use obz_core::execute::ExecuteError;
use obz_core::output::OutputFormat;
use obz_core::provider::{
    ExtensionParams, LabelValuesParams, LogSearchParams, MetricInfoParams, MetricMetadataParams,
    MetricQueryParams, ProviderConfig, TraceGetParams, TraceSearchParams,
};
use obz_core::registry::{BuiltProvider, ProviderMeta, ProviderRegistry};
use obz_core::{ErrorCode, ObzError};

use crate::cli::CliOutputFormat;
use crate::config;
use crate::credential_cache;
use crate::skills;

/// Global flag names stored into [`ProviderConfig`] values.
///
/// Only `endpoint` remains as a CLI-overridable value.
const GLOBAL_CONFIG_KEYS: &[&str] = &["endpoint"];

/// Default `--from` value for all query commands (1 hour ago).
const DEFAULT_FROM: &str = "now-1h";

/// Resolve global flags, instantiate the selected provider, and dispatch.
///
/// Returns the resolved provider name alongside the result so the caller
/// can inject it into [`ErrorDetail`] for structured JSON error output.
pub(crate) async fn run(
    registry: &ProviderRegistry,
    matches: ArgMatches,
    config_dir: &Path,
) -> (Option<String>, Result<(), ExecuteError>) {
    // Load config.yaml. Missing file/dir is not an error — we get an empty ObzConfig.
    let obz_config = match config::load(config_dir) {
        Ok(config) => config,
        Err(error) => return (None, Err(error.into())),
    };

    let Some((signal, signal_matches)) = matches.subcommand() else {
        unreachable!("clap enforces subcommand_required");
    };

    // Skills command does not carry query-global flags (output, provider,
    // etc.), so dispatch it before accessing those flags.
    if signal == "skills" {
        return (
            None,
            dispatch_skill(signal_matches, config_dir, registry).map_err(ExecuteError::Obz),
        );
    }

    let output: OutputFormat = signal_matches
        .get_one::<CliOutputFormat>("output")
        .copied()
        .unwrap_or(CliOutputFormat::Json)
        .into();
    let fields = parse_fields(signal_matches);
    let truncate = signal_matches.get_one::<usize>("truncate").copied();

    // Resolve provider name: -p flag > per-signal default > global default.
    let cli_provider = signal_matches
        .get_one::<String>("provider")
        .map(String::as_str);
    let from_defaults = cli_provider.is_none();
    let Some(provider_name) = cli_provider.or_else(|| obz_config.default_provider(signal)) else {
        return (
            None,
            Err(ExecuteError::Obz(ObzError::InvalidArgument {
                code: ErrorCode::MissingRequired,
                message: "--provider is required (e.g. -p vm). \
                          Tip: set a default in ~/.config/obz/config.yaml under 'defaults:'. \
                          Run 'obz provider list' to see available providers, \
                          or 'obz skills list' to find setup guides"
                    .to_string(),
                suggestion: Some(
                    "Specify --provider <name>, or set default_provider in config.yaml".to_string(),
                ),
            })),
        );
    };
    let provider_name = provider_name.to_string();

    let result = async {
        // Seed ProviderConfig from config files (if the -p name matches).
        let resolved = obz_config.resolve_with_dir(&provider_name, Some(config_dir))?;
        if resolved.is_none() && from_defaults {
            // The default provider name doesn't match any entry in config.yaml.
            // Give a specific error rather than a generic "missing endpoint" later.
            let signal_key = match signal {
                "metric" => "defaults.metric",
                "log" => "defaults.log",
                "trace" => "defaults.trace",
                _ => "defaults.provider",
            };
            return Err(ExecuteError::Obz(ObzError::InvalidArgument {
                code: ErrorCode::ConfigError,
                message: format!(
                    "default provider \"{provider_name}\" (from {signal_key} in config.yaml) \
                     not found in providers section"
                ),
                suggestion: None,
            }));
        }
        let mut config = resolved.unwrap_or_default();

        // Determine the provider type early — needed for env var resolution.
        // If the config entry has an explicit `provider` field, use it;
        // otherwise the -p name itself is the provider type alias.
        let provider_type = obz_config.provider_type(&provider_name);

        let verbose = signal_matches.get_flag("verbose");

        // CLI flags overwrite config file values.
        for &key in GLOBAL_CONFIG_KEYS {
            if let Some(v) = signal_matches.get_one::<String>(key) {
                config.set(key, v);
            }
        }

        // CLI timeout overrides config timeout.
        if let Some(t) = signal_matches.get_one::<String>("timeout") {
            let secs = obz_core::time::parse_step(t).map_err(|e| {
                ExecuteError::Obz(ObzError::InvalidArgument {
                    code: ErrorCode::InvalidFlag,
                    message: format!("invalid --timeout: {e}"),
                    suggestion: None,
                })
            })?;
            config.set_timeout(std::time::Duration::from_secs(secs));
        }

        if verbose {
            config.set_verbose(true);
        }

        // Look up provider metadata using the inferred type.
        let meta = registry.get(provider_type)?;

        // Priority: CLI flags > credential-process > ${env:}/${file:} resolved > inline config.
        if let Some(cp_config) = obz_config.credential_process(&provider_name) {
            if verbose {
                eprintln!(
                    "[verbose] credential-process configured: {} {}",
                    cp_config.command,
                    cp_config.args.join(" ")
                );
            }
            let output = credential_cache::get_or_refresh(&provider_name, cp_config, verbose)
                .map_err(ExecuteError::Obz)?;
            if verbose {
                let auth_keys: Vec<&str> = output.auth.keys().map(String::as_str).collect();
                let header_keys: Vec<&str> = output.headers.keys().map(String::as_str).collect();
                eprintln!(
                    "[verbose] credential-process auth keys: [{}], header keys: [{}]",
                    auth_keys.join(", "),
                    header_keys.join(", ")
                );
            }
            for (k, v) in &output.auth {
                config.set_auth(k, v);
            }
            for (k, v) in &output.headers {
                config.set_header(k, v);
            }
        }

        // Collect provider-specific flags from the subcommand matches.
        // CLI flags override credential-process output.
        collect_provider_flags(&mut config, meta, &matches);

        // Instantiate exactly the one selected provider.
        // Provider-specific required flags (e.g. --endpoint) are validated
        // inside the provider's build() via config.require().
        let provider = (meta.build)(&config)?;

        let result = match (signal, signal_matches) {
            ("metric", m) => {
                dispatch_metric(&provider, m, output, fields.as_deref(), truncate).await
            }
            ("log", m) => dispatch_log(&provider, m, output, fields.as_deref(), truncate).await,
            ("trace", m) => {
                dispatch_trace(&provider, m, output, fields.as_deref(), truncate).await
            }
            _ => unreachable!("clap enforces subcommand_required"),
        };

        if let Err(ExecuteError::Obz(ObzError::Auth {
            code,
            message: original_msg,
            ..
        })) = &result
        {
            if matches!(code, ErrorCode::AuthMissing | ErrorCode::AuthExpired)
                && obz_config.credential_process(&provider_name).is_some()
            {
                let Some(cp_config) = obz_config.credential_process(&provider_name) else {
                    unreachable!("credential_process existence checked above");
                };
                match credential_cache::refresh(&provider_name, cp_config, verbose) {
                    Ok(()) => {
                        return Err(ExecuteError::Obz(ObzError::Auth {
                            code: *code,
                            message: format!(
                                "authentication failed from provider \"{provider_name}\": {original_msg}"
                            ),
                            recoverable: true,
                            suggestion: Some(
                                "Cached credentials were expired. Fresh credentials have been \
                                 obtained from credential-process. Retry the same command."
                                    .to_string(),
                            ),
                        }));
                    }
                    Err(refresh_err) => {
                        return Err(ExecuteError::Obz(ObzError::Auth {
                            code: *code,
                            message: format!(
                                "authentication failed from provider \"{provider_name}\": {original_msg}"
                            ),
                            recoverable: false,
                            suggestion: Some(format!(
                                "Cached credentials were expired. Credential refresh also \
                                 failed: {refresh_err}"
                            )),
                        }));
                    }
                }
            }
        }

        // Enrich Unsupported errors with CLI navigation hints.
        match result {
            Err(ExecuteError::Obz(ObzError::Unsupported {
                message,
                ref provider,
                suggestion,
            })) => {
                let cli_hint = provider
                    .as_deref()
                    .and_then(skills::skill_hint)
                    .unwrap_or_else(|| {
                        "Run 'obz provider check' to verify your configuration".to_string()
                    });
                let enriched = match suggestion {
                    Some(base) => {
                        let sep = if base.ends_with(['.', '!', '?', ':']) {
                            " "
                        } else {
                            ". "
                        };
                        format!("{base}{sep}{cli_hint}")
                    }
                    None => cli_hint,
                };
                Err(ExecuteError::Obz(ObzError::Unsupported {
                    message,
                    provider: provider.clone(),
                    suggestion: Some(enriched),
                }))
            }
            other => other,
        }
    }
    .await;

    (Some(provider_name), result)
}

/// Extract provider-specific flag values from `ArgMatches` and store them
/// into `config`.
///
/// Walks through the subcommand hierarchy (e.g. `metric` → `query`) to find
/// the leaf `ArgMatches`, then reads any flags declared in the provider's
/// `command_flags` for the matching [`obz_core::StandardCommand`].
fn collect_provider_flags(config: &mut ProviderConfig, meta: &ProviderMeta, matches: &ArgMatches) {
    // Determine the signal and subcommand (e.g. "metric" + "query").
    let Some((signal, signal_matches)) = matches.subcommand() else {
        return;
    };
    let Some((subcmd, subcmd_matches)) = signal_matches.subcommand() else {
        return;
    };

    for &(cmd, flags) in meta.command_flags {
        if cmd.signal() != signal || cmd.subcommand() != subcmd {
            continue;
        }
        extract_flags(config, flags, subcmd_matches);
    }

    // Extension command flags are not stored in ProviderConfig — they are
    // handled directly by dispatch_extension_command. No action needed here.
}

/// Extract flag values from `ArgMatches` into `ProviderConfig`.
fn extract_flags(
    config: &mut ProviderConfig,
    flags: &[obz_core::descriptor::FlagDescriptor],
    m: &ArgMatches,
) {
    for fd in flags {
        match fd.flag_type {
            FlagType::String | FlagType::Duration => {
                if let Some(v) = m.get_one::<String>(fd.name) {
                    config.set(fd.name, v);
                }
            }
            FlagType::Int => {
                if let Some(v) = m.get_one::<i64>(fd.name) {
                    config.set(fd.name, v.to_string());
                }
            }
            FlagType::Bool => {
                if m.get_flag(fd.name) {
                    config.set(fd.name, "true");
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Arg-extraction helpers (reduce dispatch boilerplate)
// ---------------------------------------------------------------------------

/// Parse an optional time argument from `ArgMatches`.
fn parse_optional_time(m: &ArgMatches, key: &str) -> Result<Option<i64>, ExecuteError> {
    m.try_get_one::<String>(key)
        .ok()
        .flatten()
        .map(|s| obz_core::time::parse_time(s.as_str()))
        .transpose()
        .map_err(time_err)
}

/// Resolve `--from` / `--to` into a `(start, end)` pair with a default for `from`.
fn resolve_from_to(m: &ArgMatches, default_from: &str) -> Result<(i64, i64), ExecuteError> {
    let from = m.get_one::<String>("from").map(String::as_str);
    let to = m.get_one::<String>("to").map(String::as_str);
    obz_core::time::resolve_time_range(from, to, default_from).map_err(time_err)
}

/// Extract a required `--limit` value (clap guarantees a default).
///
/// # Panics
///
/// Panics if clap did not provide a default — should never happen.
///
/// # Errors
///
/// Returns [`ExecuteError`] if the user explicitly passes `--limit 0`,
/// because backend APIs like Prometheus treat `limit=0` as "unlimited".
fn required_limit(m: &ArgMatches) -> Result<usize, ExecuteError> {
    let limit = *m
        .get_one::<usize>("limit")
        .expect("clap provides default for --limit");
    if limit == 0 {
        return Err(ExecuteError::Obz(ObzError::InvalidArgument {
            code: ErrorCode::InvalidFlag,
            message: "--limit must be at least 1 (0 would disable the limit)".to_string(),
            suggestion: None,
        }));
    }
    Ok(limit)
}

/// Convert a time parsing error into an [`ExecuteError`].
fn time_err(e: impl std::fmt::Display) -> ExecuteError {
    ExecuteError::Obz(ObzError::InvalidArgument {
        code: ErrorCode::InvalidTimeRange,
        message: e.to_string(),
        suggestion: None,
    })
}

fn parse_fields(m: &ArgMatches) -> Option<Vec<String>> {
    let raw = m.get_one::<String>("fields")?;
    let fields: Vec<String> = raw
        .split(',')
        .map(str::trim)
        .filter(|field| !field.is_empty())
        .map(str::to_string)
        .collect();

    if fields.is_empty() {
        None
    } else {
        Some(fields)
    }
}

// ---------------------------------------------------------------------------
// Signal dispatchers
// ---------------------------------------------------------------------------

async fn dispatch_metric(
    provider: &BuiltProvider,
    m: &ArgMatches,
    output: OutputFormat,
    fields: Option<&[String]>,
    truncate: Option<usize>,
) -> Result<(), ExecuteError> {
    use obz_core::execute;

    match m.subcommand() {
        Some(("query", m)) => {
            let query = m
                .get_one::<String>("query")
                .expect("clap requires --query")
                .clone();
            let is_range = metric_query_is_range(m);
            let step = m.get_one::<String>("step").map(String::as_str);
            let limit = required_limit(m)?;
            let timeout = m.get_one::<String>("timeout").map(String::as_str);

            let (start, end) = resolve_from_to(m, DEFAULT_FROM)?;
            let step_secs = step
                .map(obz_core::time::parse_step)
                .transpose()
                .map_err(time_err)?;
            let timeout_dur = timeout
                .map(|t| obz_core::time::parse_step(t).map(std::time::Duration::from_secs))
                .transpose()
                .map_err(time_err)?;

            execute::execute_metric_query(
                provider,
                &MetricQueryParams {
                    query,
                    is_range,
                    start,
                    end,
                    step: step_secs,
                    limit: Some(limit),
                    timeout: timeout_dur,
                },
                limit,
                output,
                fields,
                truncate,
                &mut std::io::stdout(),
            )
            .await
        }

        Some(("list", m)) => {
            let match_expr = m.get_one::<String>("match").cloned();
            let limit = required_limit(m)?;
            let start = parse_optional_time(m, "from")?;
            let end = parse_optional_time(m, "to")?;

            execute::execute_metric_list(
                provider,
                &MetricMetadataParams {
                    match_expr,
                    match_exprs: vec![],
                    start,
                    end,
                    limit: Some(limit),
                },
                output,
                fields,
                truncate,
                &mut std::io::stdout(),
            )
            .await
        }

        Some(("info", m)) => {
            let metric_name = m
                .get_one::<String>("metric_name")
                .expect("clap requires <metric_name>")
                .clone();
            execute::execute_metric_info(
                provider,
                &MetricInfoParams { metric_name },
                output,
                fields,
                truncate,
                &mut std::io::stdout(),
            )
            .await
        }

        Some(("labels", m)) => {
            let match_expr = m.get_one::<String>("match").cloned();
            let start = parse_optional_time(m, "from")?;
            let end = parse_optional_time(m, "to")?;

            execute::execute_metric_labels(
                provider,
                &MetricMetadataParams {
                    match_expr,
                    match_exprs: vec![],
                    start,
                    end,
                    limit: None,
                },
                output,
                fields,
                truncate,
                &mut std::io::stdout(),
            )
            .await
        }

        Some(("label-values", m)) => {
            let label_name = m
                .get_one::<String>("label_name")
                .expect("clap requires <label_name>")
                .clone();
            let match_expr = m.get_one::<String>("match").cloned();
            let limit = required_limit(m)?;
            let start = parse_optional_time(m, "from")?;
            let end = parse_optional_time(m, "to")?;

            execute::execute_metric_label_values(
                provider,
                &LabelValuesParams {
                    label_name,
                    match_expr,
                    start,
                    end,
                    limit: Some(limit),
                },
                output,
                fields,
                truncate,
                &mut std::io::stdout(),
            )
            .await
        }

        Some(("series", m)) => {
            let match_exprs: Vec<String> = m
                .get_many::<String>("match")
                .unwrap_or_default()
                .cloned()
                .collect();
            let limit = required_limit(m)?;
            let start = parse_optional_time(m, "from")?;
            let end = parse_optional_time(m, "to")?;

            execute::execute_metric_series(
                provider,
                &MetricMetadataParams {
                    match_expr: None,
                    match_exprs,
                    start,
                    end,
                    limit: Some(limit),
                },
                output,
                fields,
                truncate,
                &mut std::io::stdout(),
            )
            .await
        }

        // Extension commands (e.g. provider-specific metric subcommands).
        Some((cmd, m)) => {
            dispatch_extension_command(provider, "metric", cmd, m, output, fields, truncate).await
        }
        None => unreachable!("clap enforces subcommand_required"),
    }
}

fn metric_query_is_range(m: &ArgMatches) -> bool {
    m.get_one::<String>("from").is_some()
        || m.get_one::<String>("to").is_some()
        || m.get_one::<String>("step").is_some()
}

async fn dispatch_log(
    provider: &BuiltProvider,
    m: &ArgMatches,
    output: OutputFormat,
    fields: Option<&[String]>,
    truncate: Option<usize>,
) -> Result<(), ExecuteError> {
    use obz_core::execute;

    match m.subcommand() {
        Some(("search", m)) => {
            let query = m
                .get_one::<String>("query")
                .expect("clap provides default for --query")
                .clone();
            let limit = required_limit(m)?;
            let (start, end) = resolve_from_to(m, DEFAULT_FROM)?;

            execute::execute_log_search(
                provider,
                &LogSearchParams {
                    query,
                    start,
                    end,
                    limit,
                },
                output,
                fields,
                truncate,
                &mut std::io::stdout(),
            )
            .await
        }
        // Extension commands (e.g. provider-specific log subcommands).
        Some((cmd, m)) => {
            dispatch_extension_command(provider, "log", cmd, m, output, fields, truncate).await
        }
        None => unreachable!("clap enforces subcommand_required"),
    }
}

async fn dispatch_trace(
    provider: &BuiltProvider,
    m: &ArgMatches,
    output: OutputFormat,
    fields: Option<&[String]>,
    truncate: Option<usize>,
) -> Result<(), ExecuteError> {
    use obz_core::execute;

    match m.subcommand() {
        Some(("search", m)) => {
            let query = m
                .get_one::<String>("query")
                .expect("clap requires --query")
                .clone();
            let limit = required_limit(m)?;
            let (start, end) = resolve_from_to(m, DEFAULT_FROM)?;
            execute::execute_trace_search(
                provider,
                &TraceSearchParams {
                    query,
                    start,
                    end,
                    limit,
                },
                output,
                fields,
                truncate,
                &mut std::io::stdout(),
            )
            .await
        }
        Some(("get", m)) => {
            let trace_id = m
                .get_one::<String>("trace_id")
                .expect("clap requires <trace_id>")
                .clone();
            let (start, end) = resolve_from_to(m, DEFAULT_FROM)?;
            execute::execute_trace_get(
                provider,
                &TraceGetParams {
                    trace_id,
                    start,
                    end,
                },
                output,
                fields,
                truncate,
                &mut std::io::stdout(),
            )
            .await
        }
        // Extension commands (e.g. `trace services`, `trace operations`).
        Some((cmd, m)) => {
            dispatch_extension_command(provider, "trace", cmd, m, output, fields, truncate).await
        }
        _ => unreachable!("clap enforces subcommand_required"),
    }
}

/// Dispatch an extension command by collecting its flag values and calling
/// the core execute function.
async fn dispatch_extension_command(
    provider: &BuiltProvider,
    signal: &str,
    command: &str,
    m: &ArgMatches,
    output: OutputFormat,
    fields: Option<&[String]>,
    truncate: Option<usize>,
) -> Result<(), ExecuteError> {
    use obz_core::execute;

    // Collect extension command flags into a vector of key-value pairs.
    // Skip global flags (provider, endpoint, output, from, to, verbose,
    // timeout) to avoid leaking them into the provider's ExtensionParams.args.
    const EXTRA_SKIP: &[&str] = &[
        "provider", "output", "fields", "truncate", "from", "to", "verbose", "timeout",
    ];
    let skip = |key: &str| GLOBAL_CONFIG_KEYS.contains(&key) || EXTRA_SKIP.contains(&key);
    let mut args = Vec::new();
    for id in m.ids() {
        let id_str = id.as_str();
        if skip(id_str) {
            continue;
        }
        if let Ok(Some(values)) = m.try_get_many::<String>(id_str) {
            for v in values {
                args.push((id_str.to_string(), v.clone()));
            }
        } else if let Ok(Some(v)) = m.try_get_one::<String>(id_str) {
            args.push((id_str.to_string(), v.clone()));
        } else if let Ok(Some(v)) = m.try_get_one::<i64>(id_str) {
            args.push((id_str.to_string(), v.to_string()));
        } else if let Ok(true) = m
            .try_get_one::<bool>(id_str)
            .map(|v| v.copied().unwrap_or(false))
        {
            args.push((id_str.to_string(), "true".to_string()));
        }
    }

    // Parse --from/--to if the extension command declared them as flags.
    // If not present, these remain None — the provider decides defaults.
    let start = parse_optional_time(m, "from")?;
    let end = parse_optional_time(m, "to")?;

    let params = ExtensionParams {
        start,
        end,
        signal: signal.to_string(),
        args,
    };

    // `command` is passed as both the dispatch key and the `result_type` in
    // the JSON response. This is intentional — the command name (e.g.
    // "services") doubles as a reasonable result_type label.
    execute::execute_extension_command(
        provider,
        command,
        &params,
        output,
        fields,
        truncate,
        &mut std::io::stdout(),
    )
    .await
}

fn dispatch_skill(
    m: &ArgMatches,
    config_dir: &Path,
    registry: &ProviderRegistry,
) -> Result<(), ObzError> {
    match m.subcommand() {
        Some(("list", sub_m)) => {
            let output: OutputFormat = sub_m
                .get_one::<CliOutputFormat>("output")
                .copied()
                .unwrap_or(CliOutputFormat::Table)
                .into();
            skills::list(registry, output)
        }
        Some(("show", m)) => {
            let names: Vec<String> = m
                .get_many::<String>("names")
                .unwrap_or_default()
                .cloned()
                .collect();
            skills::show(&names)
        }
        Some(("install", m)) => {
            let dir = m
                .get_one::<String>("dir")
                .expect("clap requires --dir")
                .as_str();
            let all = m.get_flag("all");
            let names: Vec<String> = m
                .get_many::<String>("names")
                .unwrap_or_default()
                .cloned()
                .collect();
            skills::install(dir, &names, all, config_dir, registry)
        }
        _ => unreachable!("clap enforces subcommand_required"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::build_cli;
    use obz_core::registry::ProviderRegistry;

    fn metric_query_matches(args: &[&str]) -> ArgMatches {
        let registry = ProviderRegistry::new();
        let matches = build_cli(&registry, None)
            .try_get_matches_from(args)
            .expect("CLI parsing should succeed");
        let Some(("metric", metric_matches)) = matches.subcommand() else {
            panic!("expected metric subcommand");
        };
        metric_matches.clone()
    }

    #[test]
    fn metric_query_without_from_or_step_stays_instant() {
        let matches = metric_query_matches(&["obz", "metric", "query", "-q", "up", "-p", "vm"]);

        let Some(("query", query_matches)) = matches.subcommand() else {
            panic!("expected query subcommand");
        };

        assert!(!metric_query_is_range(query_matches));
        let (start, end) =
            resolve_from_to(query_matches, DEFAULT_FROM).expect("time range should resolve");
        assert!(end >= start);
        // Default window is ~1h (3600s).
        let window = end - start;
        assert!(
            (3500..=3700).contains(&window),
            "default window should be ~1h, got {window}s"
        );
    }

    #[test]
    fn metric_query_with_step_implies_range_even_without_from() {
        let matches = metric_query_matches(&[
            "obz", "metric", "query", "-q", "up", "--step", "1m", "-p", "vm",
        ]);

        let Some(("query", query_matches)) = matches.subcommand() else {
            panic!("expected query subcommand");
        };

        assert!(metric_query_is_range(query_matches));
        let step = query_matches
            .get_one::<String>("step")
            .map(String::as_str)
            .map(obz_core::time::parse_step)
            .transpose()
            .expect("step should parse");
        assert_eq!(step, Some(60));
    }

    #[test]
    fn metric_query_with_explicit_from_is_range_even_without_step() {
        let matches = metric_query_matches(&[
            "obz", "metric", "query", "-q", "up", "--from", "now-2h", "-p", "vm",
        ]);

        let Some(("query", query_matches)) = matches.subcommand() else {
            panic!("expected query subcommand");
        };

        assert!(metric_query_is_range(query_matches));
        assert!(query_matches.get_one::<String>("step").is_none());
    }

    #[test]
    fn metric_query_with_explicit_to_only_is_range() {
        let matches = metric_query_matches(&[
            "obz", "metric", "query", "-q", "up", "--to", "now", "-p", "vm",
        ]);

        let Some(("query", query_matches)) = matches.subcommand() else {
            panic!("expected query subcommand");
        };

        assert!(metric_query_is_range(query_matches));
    }

    fn log_matches(args: &[&str]) -> ArgMatches {
        let mut registry = ProviderRegistry::new();
        obz_providers::register_all(&mut registry);
        let matches = build_cli(&registry, Some("loki"))
            .try_get_matches_from(args)
            .expect("CLI parsing should succeed");
        let Some(("log", log_matches)) = matches.subcommand() else {
            panic!("expected log subcommand");
        };
        log_matches.clone()
    }

    #[test]
    fn parse_optional_time_returns_none_for_extension_command_without_time_flags() {
        let matches = log_matches(&["obz", "log", "labels", "-p", "loki"]);

        let Some(("labels", extension_matches)) = matches.subcommand() else {
            panic!("expected labels subcommand");
        };

        assert_eq!(
            parse_optional_time(extension_matches, "from").unwrap(),
            None
        );
        assert_eq!(parse_optional_time(extension_matches, "to").unwrap(), None);
    }

    #[test]
    fn parse_optional_time_still_parses_registered_core_time_flags() {
        let matches = log_matches(&[
            "obz",
            "log",
            "search",
            "-q",
            "{service_name=~\".+\"}",
            "--from",
            "now-5m",
            "-p",
            "loki",
        ]);

        let Some(("search", core_matches)) = matches.subcommand() else {
            panic!("expected search subcommand");
        };

        let parsed = parse_optional_time(core_matches, "from").unwrap();
        assert!(parsed.is_some());
        assert_eq!(parse_optional_time(core_matches, "to").unwrap(), None);
    }
}
