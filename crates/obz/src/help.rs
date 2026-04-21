//! Dynamic help generation for the obz CLI.
//!
//! Builds the CORE COMMANDS, EXTENSIONS, and EXAMPLES sections
//! for `--help` output. When `-p` is specified, only that provider's
//! support and extensions are shown.

use obz_core::registry::{ProviderMeta, ProviderRegistry};
use obz_core::SupportedCommands;

/// Core metric commands with their short descriptions.
const METRIC_CORE_CMDS: &[(&str, &str)] = &[
    ("query", "Execute a metric query (instant or range)"),
    ("list", "List metric names"),
    ("info", "Get metric metadata (type, description, unit)"),
    ("labels", "List label names"),
    ("label-values", "List values for a specific label"),
    ("series", "Find series matching selectors"),
];

/// Core log commands with their short descriptions.
const LOG_CORE_CMDS: &[(&str, &str)] = &[("search", "Search for log entries")];

/// Core trace commands with their short descriptions.
const TRACE_CORE_CMDS: &[(&str, &str)] = &[
    ("search", "Search for spans across traces"),
    ("get", "Get all spans for a specific trace by ID"),
];

/// Returns whether the given provider name/alias matches a [`ProviderMeta`].
fn provider_matches(meta: &ProviderMeta, name: &str) -> bool {
    meta.aliases.contains(&name)
}

/// Type alias for a function that returns which providers support a given command name.
type SupportersFn = fn(&ProviderRegistry, &str) -> Vec<&'static str>;

/// Type alias for a function that checks if a single provider supports a command.
type SupportedFn = fn(&SupportedCommands, &str) -> bool;

/// Returns which providers support a given command, using the provided
/// check function to test each provider's [`SupportedCommands`].
fn providers_supporting(
    registry: &ProviderRegistry,
    cmd: &str,
    check: SupportedFn,
) -> Vec<&'static str> {
    registry
        .all()
        .iter()
        .filter(|m| check(&m.supported_commands, cmd))
        .map(|m| m.name)
        .collect()
}

fn metric_supported(s: &SupportedCommands, cmd: &str) -> bool {
    match cmd {
        "query" => s.metric_query,
        "list" => s.metric_list,
        "info" => s.metric_info,
        "labels" => s.metric_labels,
        "label-values" => s.metric_label_values,
        "series" => s.metric_series,
        _ => false,
    }
}

fn log_supported(s: &SupportedCommands, cmd: &str) -> bool {
    match cmd {
        "search" => s.log_search,
        _ => false,
    }
}

fn trace_supported(s: &SupportedCommands, cmd: &str) -> bool {
    match cmd {
        "search" => s.trace_search,
        "get" => s.trace_get,
        _ => false,
    }
}

/// Returns which providers support a given metric core command.
fn providers_supporting_metric(registry: &ProviderRegistry, cmd: &str) -> Vec<&'static str> {
    providers_supporting(registry, cmd, metric_supported)
}

/// Returns which providers support a given log core command.
fn providers_supporting_log(registry: &ProviderRegistry, cmd: &str) -> Vec<&'static str> {
    providers_supporting(registry, cmd, log_supported)
}

/// Returns which providers support a given trace core command.
fn providers_supporting_trace(registry: &ProviderRegistry, cmd: &str) -> Vec<&'static str> {
    providers_supporting(registry, cmd, trace_supported)
}

/// Collect extension commands grouped by provider for a given signal.
///
/// Returns a list of `(provider_meta, commands)` pairs, filtered by
/// `selected_provider` if specified. Only providers with at least one
/// extension command for the given signal are included.
fn extension_commands_by_provider<'a>(
    registry: &'a ProviderRegistry,
    selected_provider: Option<&str>,
    signal: &str,
) -> Vec<(&'a ProviderMeta, Vec<&'a obz_core::CommandDescriptor>)> {
    registry
        .all()
        .iter()
        .filter(|m| selected_provider.map_or(true, |p| provider_matches(m, p)))
        .filter_map(|m| {
            let commands: Vec<&obz_core::CommandDescriptor> = m
                .extension_commands
                .iter()
                .filter(|(s, _)| *s == signal)
                .map(|(_, cmd)| cmd)
                .collect();

            (!commands.is_empty()).then_some((m, commands))
        })
        .collect()
}

/// Render the compact extension command listing for `-h` (short help).
///
/// Shows one line per provider with comma-separated command names.
/// When a specific provider is selected, shows a single flat line.
fn render_extension_short_help(
    providers: &[(&ProviderMeta, Vec<&obz_core::CommandDescriptor>)],
    selected_provider: Option<&str>,
) -> String {
    if providers.is_empty() {
        return String::new();
    }

    if let Some(provider) = selected_provider {
        if let Some((_, commands)) = providers.first() {
            let names = commands
                .iter()
                .map(|cmd| cmd.name)
                .collect::<Vec<_>>()
                .join(", ");
            return format!("EXTENSIONS ({provider}):  {names}");
        }
        return String::new();
    }

    let provider_width = providers
        .iter()
        .map(|(meta, _)| meta.name.len())
        .max()
        .unwrap_or(0);

    let mut out = String::from("EXTENSIONS:\n");
    for (idx, (meta, commands)) in providers.iter().enumerate() {
        if idx > 0 {
            out.push('\n');
        }

        let names = commands
            .iter()
            .map(|cmd| cmd.name)
            .collect::<Vec<_>>()
            .join(", ");
        let provider_tag = format!("[{}]", meta.name);
        let padding = provider_width.saturating_sub(meta.name.len()) + 2;
        out.push_str(&format!("  {provider_tag}{}{names}", " ".repeat(padding)));
    }
    out
}

/// Render the full extension command listing for `--help` (long help).
///
/// Shows commands grouped under `[provider]` headers with descriptions.
/// When a specific provider is selected, uses a flat list without group header.
/// Command name alignment is computed per provider group independently.
fn render_extension_long_help(
    providers: &[(&ProviderMeta, Vec<&obz_core::CommandDescriptor>)],
    selected_provider: Option<&str>,
) -> String {
    if providers.is_empty() {
        return String::new();
    }

    let mut out = String::new();
    if let Some(provider) = selected_provider {
        if let Some((_, commands)) = providers.first() {
            let width = commands.iter().map(|cmd| cmd.name.len()).max().unwrap_or(0);
            out.push_str(&format!("EXTENSIONS ({provider}):\n"));
            for cmd in commands {
                out.push_str(&format!("  {:<width$}  {}\n", cmd.name, cmd.description));
            }
            return out.trim_end().to_string();
        }
        return String::new();
    }

    out.push_str("EXTENSIONS:\n");
    for (idx, (meta, commands)) in providers.iter().enumerate() {
        if idx > 0 {
            out.push('\n');
        }
        let width = commands.iter().map(|cmd| cmd.name.len()).max().unwrap_or(0);
        out.push_str(&format!("  [{}]\n", meta.name));
        for cmd in commands {
            out.push_str(&format!("    {:<width$}  {}\n", cmd.name, cmd.description));
        }
    }

    out.trim_end().to_string()
}

/// Build the `after_help` and `after_long_help` strings for a signal group (`metric`, `log`, `trace`).
///
/// Generic over the core commands slice, supporter lookup fn, per-provider support fn,
/// and examples string. The extension command display is shared for all signals.
fn signal_help(
    registry: &ProviderRegistry,
    selected_provider: Option<&str>,
    core_cmds: &[(&str, &str)],
    supporters_fn: SupportersFn,
    supported_fn: SupportedFn,
    signal: &str,
    examples: &str,
) -> (String, String) {
    let mut out = String::new();

    out.push_str("CORE COMMANDS:\n");
    for &(cmd, desc) in core_cmds {
        if let Some(p) = selected_provider {
            let supported = registry
                .all()
                .iter()
                .any(|m| provider_matches(m, p) && supported_fn(&m.supported_commands, cmd));
            if supported {
                out.push_str(&format!("  {cmd:<16}  {desc}\n"));
            }
        } else {
            let supporters = supporters_fn(registry, cmd);
            let tag = if supporters.is_empty() {
                String::new()
            } else {
                format!("  [{}]", supporters.join(", "))
            };
            out.push_str(&format!("  {cmd:<16}  {desc}{tag}\n"));
        }
    }

    let extensions = extension_commands_by_provider(registry, selected_provider, signal);
    if !extensions.is_empty() {
        out.push('\n');
        out.push_str(&render_extension_long_help(&extensions, selected_provider));
    }

    out.push_str("\n\nEXAMPLES:\n");
    out.push_str(examples);

    (
        render_extension_short_help(&extensions, selected_provider),
        out,
    )
}

/// Build the `after_help` and `after_long_help` strings for `obz metric --help`.
pub(crate) fn metric_help(
    registry: &ProviderRegistry,
    selected_provider: Option<&str>,
) -> (String, String) {
    signal_help(
        registry,
        selected_provider,
        METRIC_CORE_CMDS,
        providers_supporting_metric,
        metric_supported,
        "metric",
        "  obz metric query -p vm --endpoint http://localhost:8428 -q 'up'\n\
         \x20 obz metric list  -p vm --endpoint http://localhost:8428 --limit 20\n\
         \x20 obz metric query -p vm --endpoint http://localhost:8428 \\\n\
         \x20     -q 'rate(http_requests_total[5m])' --from now-1h --step 1m -o table\n",
    )
}

/// Build the `after_help` and `after_long_help` strings for `obz log --help`.
pub(crate) fn log_help(
    registry: &ProviderRegistry,
    selected_provider: Option<&str>,
) -> (String, String) {
    signal_help(
        registry,
        selected_provider,
        LOG_CORE_CMDS,
        providers_supporting_log,
        log_supported,
        "log",
        "  obz log search -p vl --endpoint http://localhost:9428 -q 'error' --limit 50\n",
    )
}

/// Build the `after_help` and `after_long_help` strings for `obz trace --help`.
pub(crate) fn trace_help(
    registry: &ProviderRegistry,
    selected_provider: Option<&str>,
) -> (String, String) {
    signal_help(
        registry,
        selected_provider,
        TRACE_CORE_CMDS,
        providers_supporting_trace,
        trace_supported,
        "trace",
        "  obz trace search -p vt --endpoint http://localhost:10428 -q 'cart' --limit 10\n\
         \x20 obz trace get    -p vt --endpoint http://localhost:10428 2e62a34ece72499fa08897393365be2f\n",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use obz_core::descriptor::CommandDescriptor;
    use obz_core::provider::ProviderConfig;
    use obz_core::registry::{BuiltProvider, ProviderMeta, SupportedCommands};
    use obz_core::ObzError;

    /// Build a minimal dummy `ProviderMeta` for testing.
    fn dummy_meta(
        name: &'static str,
        aliases: &'static [&'static str],
        supported: SupportedCommands,
        ext_cmds: &'static [(&'static str, CommandDescriptor)],
    ) -> ProviderMeta {
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

        ProviderMeta {
            name,
            display_name: name,
            aliases,
            supported_commands: supported,
            build: dummy_build,
            check: None,
            command_flags: &[],
            extension_commands: ext_cmds,
        }
    }

    #[test]
    fn test_signal_help_filters_extension_commands_by_signal() {
        static EXT_CMDS: &[(&str, CommandDescriptor)] = &[
            (
                "trace",
                CommandDescriptor {
                    name: "services",
                    description: "List services",
                    flags: &[],
                },
            ),
            (
                "metric",
                CommandDescriptor {
                    name: "top-queries",
                    description: "Show top queries",
                    flags: &[],
                },
            ),
        ];

        let supported = SupportedCommands {
            trace_search: true,
            trace_get: true,
            metric_query: true,
            ..SupportedCommands::default()
        };

        let mut registry = ProviderRegistry::new();
        registry.register(dummy_meta("testprov", &["tp"], supported, EXT_CMDS));

        // trace help should show "services" but not "top-queries".
        let (_, trace) = trace_help(&registry, None);
        assert!(trace.contains("services"), "trace help missing 'services'");
        assert!(
            !trace.contains("top-queries"),
            "trace help should not contain 'top-queries'"
        );

        // metric help should show "top-queries" but not "services".
        let (_, metric) = metric_help(&registry, None);
        assert!(
            metric.contains("top-queries"),
            "metric help missing 'top-queries'"
        );
        assert!(
            !metric.contains("services"),
            "metric help should not contain 'services'"
        );
    }

    #[test]
    fn test_signal_help_with_selected_provider() {
        static EXT_A: &[(&str, CommandDescriptor)] = &[(
            "trace",
            CommandDescriptor {
                name: "services",
                description: "List services",
                flags: &[],
            },
        )];
        static EXT_B: &[(&str, CommandDescriptor)] = &[(
            "trace",
            CommandDescriptor {
                name: "tags",
                description: "List tags",
                flags: &[],
            },
        )];

        let supported = SupportedCommands {
            trace_search: true,
            trace_get: true,
            ..SupportedCommands::default()
        };

        let mut registry = ProviderRegistry::new();
        registry.register(dummy_meta("vt", &["vt"], supported, EXT_A));
        registry.register(dummy_meta("tempo", &["tempo"], supported, EXT_B));

        // With -p vt, should show only VT's "services", not Tempo's "tags".
        let (_, help) = trace_help(&registry, Some("vt"));
        assert!(help.contains("services"), "vt help missing 'services'");
        assert!(!help.contains("tags"), "vt help should not contain 'tags'");
        assert!(
            help.contains("EXTENSIONS (vt)"),
            "should show provider-specific heading"
        );

        // With -p tempo, should show only Tempo's "tags".
        let (_, help) = trace_help(&registry, Some("tempo"));
        assert!(help.contains("tags"), "tempo help missing 'tags'");
        assert!(
            !help.contains("services"),
            "tempo help should not contain 'services'"
        );
    }

    #[test]
    fn test_signal_help_short_help_groups_extensions_by_provider() {
        static LOKI_EXTS: &[(&str, CommandDescriptor)] = &[
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
                    name: "fields",
                    description: "List detected fields in log content",
                    flags: &[],
                },
            ),
        ];
        static VLOGS_EXTS: &[(&str, CommandDescriptor)] = &[(
            "log",
            CommandDescriptor {
                name: "field-names",
                description: "List available field names",
                flags: &[],
            },
        )];

        let mut registry = ProviderRegistry::new();
        registry.register(dummy_meta(
            "loki",
            &["loki"],
            SupportedCommands::default(),
            LOKI_EXTS,
        ));
        registry.register(dummy_meta(
            "victorialogs",
            &["victorialogs"],
            SupportedCommands::default(),
            VLOGS_EXTS,
        ));

        let (short, long) = log_help(&registry, None);

        assert_eq!(
            short,
            "EXTENSIONS:\n  [loki]          labels, fields\n  [victorialogs]  field-names"
        );
        assert!(long.contains("EXTENSIONS:"));
        assert!(long.contains("  [loki]"));
        assert!(long.contains("    labels  List available label names"));
        assert!(long.contains("  [victorialogs]"));
    }

    #[test]
    fn test_signal_help_short_help_for_selected_provider_is_compact() {
        static LOKI_EXTS: &[(&str, CommandDescriptor)] = &[
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
                    flags: &[],
                },
            ),
        ];

        let mut registry = ProviderRegistry::new();
        registry.register(dummy_meta(
            "loki",
            &["lo"],
            SupportedCommands::default(),
            LOKI_EXTS,
        ));

        let (short, long) = log_help(&registry, Some("lo"));

        assert_eq!(short, "EXTENSIONS (lo):  labels, label-values");
        assert!(long.contains("CORE COMMANDS:"));
        assert!(long.contains("EXTENSIONS (lo):"));
        assert!(!long.contains("[loki]"));
    }

    #[test]
    fn test_signal_help_short_help_is_empty_without_extensions() {
        let registry = ProviderRegistry::new();
        let (short, long) = log_help(&registry, None);

        assert!(short.is_empty());
        assert!(long.contains("CORE COMMANDS:"));
        assert!(!long.contains("EXTENSIONS:"));
    }
}
