//! clap command tree construction for the obz CLI.
//!
//! Builds the full command tree with global flags, signal subcommands
//! (metric, log, trace), and dynamically registered provider-specific
//! flags and extension commands.

use clap::{Arg, ArgAction, Command, ValueEnum};
use clap_complete::Shell;
use obz_core::descriptor::{FlagDescriptor, FlagType};
use obz_core::output::OutputFormat;
use obz_core::registry::ProviderRegistry;

use crate::help;

/// clap-compatible output format (mirrors [`OutputFormat`] in obz-core).
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum CliOutputFormat {
    /// JSON output (default, optimized for AI Agents).
    Json,
    /// Human-readable table.
    Table,
    /// Comma-separated values.
    Csv,
}

impl From<CliOutputFormat> for OutputFormat {
    fn from(f: CliOutputFormat) -> Self {
        match f {
            CliOutputFormat::Json => OutputFormat::Json,
            CliOutputFormat::Table => OutputFormat::Table,
            CliOutputFormat::Csv => OutputFormat::Csv,
        }
    }
}

/// Help heading for connection-related flags.
const HEADING_CONNECTION: &str = "Connection";

/// Help heading for provider-specific flags (shown when no provider is selected).
const HEADING_PROVIDER: &str = "Provider Options";

/// Build the full clap command tree.
pub(crate) fn build_cli(registry: &ProviderRegistry, selected_provider: Option<&str>) -> Command {
    Command::new("obz")
        .version(env!("CARGO_PKG_VERSION"))
        .long_version(long_version())
        .about("A multi-backend observability CLI tool")
        .after_long_help(
            "TIME FORMATS:\n  \
             now-1h, now-15m        Relative (supports s/m/h/d/w)\n  \
             2024-01-01T00:00:00Z   RFC3339\n  \
             @1704067200            Unix timestamp\n\n\
             Run 'obz <signal> --help' for signal-specific commands and provider support.",
        )
        .subcommand(add_provider_flags(
            metric_command(registry, selected_provider),
            registry,
            "metric",
            selected_provider,
        ))
        .subcommand(add_provider_flags(
            log_command(registry, selected_provider),
            registry,
            "log",
            selected_provider,
        ))
        .subcommand(add_provider_flags(
            trace_command(registry, selected_provider),
            registry,
            "trace",
            selected_provider,
        ))
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommand(completions_command())
        .subcommand(man_pages_command())
        .subcommand(provider_command())
        .subcommand(skill_command())
}

/// Convert a [`FlagDescriptor`] into a clap [`Arg`].
///
/// Provider-specific flags are always optional at the clap level — required
/// enforcement happens at runtime in the provider's `build()` function so
/// that other providers sharing the same command are not affected.
///
/// * `provider_tag` — When `Some`, the help text is prefixed with a provider
///   attribution tag like `[sls]` or `[mimir, loki, tempo]`.
/// * `heading` — Optional help heading override (e.g. "Provider Options").
fn flag_descriptor_to_arg(
    fd: &FlagDescriptor,
    provider_tag: Option<&str>,
    heading: Option<&'static str>,
) -> Arg {
    let mut arg = Arg::new(fd.name).long(fd.name).required(false); // runtime enforcement, not clap
    if let Some(c) = fd.short {
        arg = arg.short(c);
    }

    // When a provider tag is present, prefix the help text with [provider].
    // Otherwise use the original static description directly (zero alloc).
    if let Some(tag) = provider_tag {
        arg = arg.help(format!("[{tag}] {}", fd.description));
    } else {
        arg = arg.help(fd.description);
    }
    if let Some(h) = heading {
        arg = arg.help_heading(h);
    }
    if let Some(default) = fd.default {
        arg = arg.default_value(default);
    }
    if fd.repeatable {
        arg = arg.action(ArgAction::Append);
    }
    match fd.flag_type {
        FlagType::Bool => {
            arg = arg.action(ArgAction::SetTrue);
        }
        FlagType::Int => {
            arg = arg.value_parser(clap::value_parser!(i64));
        }
        FlagType::Duration | FlagType::String => {}
    }
    arg
}

/// Register provider-specific flags (from [`FlagDescriptor`]) into clap
/// subcommands.
///
/// For each provider's `command_flags`, the key is an
/// [`obz_core::StandardCommand`].
/// This function matches by signal group (e.g. `"metric"`) and registers
/// the flag into the corresponding subcommand.
///
/// When `selected_provider` is `Some`, only flags from that provider are
/// registered and no provider tag is shown. When `None`, all provider
/// flags are registered with a `[provider]` tag in the help text.
fn add_provider_flags(
    mut signal_cmd: Command,
    registry: &ProviderRegistry,
    signal: &str,
    selected_provider: Option<&str>,
) -> Command {
    // Collect provider-specific flags and group them by provider so that
    // the `--help` output shows flags grouped by provider rather than in
    // alphabetical order by flag name.
    //
    // Phase 1: collect all (subcmd, flag_name) → FlagEntry, preserving
    //          insertion order via `IndexMap` so flags from the same
    //          provider stay together (registry iteration order → provider
    //          declaration order).
    // Phase 2: register the collected flags into the clap subcommands.

    /// A collected flag entry with its descriptor, owning providers, and
    /// a sort key that preserves provider-grouped ordering.
    struct FlagEntry<'a> {
        fd: &'a FlagDescriptor,
        providers: Vec<&'a str>,
        /// `(provider_index, flag_position)` — used to sort flags so that
        /// all flags from the same provider appear together, in their
        /// original declaration order.  For shared flags (multiple
        /// providers declare the same flag name), the sort key comes from
        /// the *first* provider that declared the flag.
        sort_key: (usize, usize),
    }

    // subcmd → { flag_name → FlagEntry }
    // Using `BTreeMap` for subcmd names (stable ordering) and a plain
    // `Vec` + linear scan for the inner collection. The number of flags
    // per subcommand is small (< 20), so O(n) lookup is fine and avoids
    // pulling in `IndexMap`.
    let mut flags_by_subcmd: std::collections::BTreeMap<&str, Vec<FlagEntry>> =
        std::collections::BTreeMap::new();

    for (provider_idx, meta) in registry.all().iter().enumerate() {
        // When a specific provider is selected, skip all others.
        if let Some(selected) = selected_provider {
            if !meta.aliases.contains(&selected) && meta.name != selected {
                continue;
            }
        }

        for &(cmd, flags) in meta.command_flags {
            if cmd.signal() == signal {
                let subcmd = cmd.subcommand();
                let entries = flags_by_subcmd.entry(subcmd).or_default();
                for (flag_pos, fd) in flags.iter().enumerate() {
                    if let Some(existing) = entries.iter_mut().find(|e| e.fd.name == fd.name) {
                        // Flag already registered by an earlier provider — just
                        // append this provider name for the tag (e.g. "[mimir, loki]").
                        // The sort key stays with the first provider.
                        if !existing.providers.contains(&meta.name) {
                            existing.providers.push(meta.name);
                        }
                    } else {
                        entries.push(FlagEntry {
                            fd,
                            providers: vec![meta.name],
                            sort_key: (provider_idx, flag_pos),
                        });
                    }
                }
            }
        }
    }

    // Register collected flags into the corresponding subcommands,
    // sorted by (provider_index, flag_position) so flags from the same
    // provider are grouped together.
    for (subcmd_name, entries) in &mut flags_by_subcmd {
        entries.sort_by_key(|e| e.sort_key);
        signal_cmd = signal_cmd.mut_subcommand(subcmd_name, |subcmd| {
            let mut s = subcmd;
            for entry in entries.iter() {
                let tag = if selected_provider.is_some() {
                    // Provider is known — no tag needed.
                    None
                } else {
                    Some(entry.providers.join(", "))
                };
                s = s.arg(flag_descriptor_to_arg(
                    entry.fd,
                    tag.as_deref(),
                    Some(HEADING_PROVIDER),
                ));
            }
            s
        });
    }

    signal_cmd
}

/// Register extension commands (from [`CommandDescriptor`](obz_core::CommandDescriptor))
/// as clap subcommands under the given signal command (e.g. `trace`).
///
/// Extension commands are provider-specific subcommands declared via
/// [`CommandDescriptor`](obz_core::CommandDescriptor) in the provider's
/// [`ProviderMeta`](obz_core::registry::ProviderMeta). Each extension
/// command becomes a real clap subcommand with full tab-completion and `--help`.
///
/// Only commands whose signal tag matches `signal` are registered.
/// This mirrors [`add_provider_flags`] which filters `command_flags` by
/// [`obz_core::StandardCommand::signal`].
fn add_extension_commands(
    mut signal_cmd: Command,
    registry: &ProviderRegistry,
    signal: &str,
    selected_provider: Option<&str>,
) -> Command {
    let mut seen_command_names = std::collections::BTreeSet::new();
    let show_grouped_help = selected_provider.is_none();

    for meta in registry.all() {
        if let Some(selected) = selected_provider {
            if !meta.aliases.contains(&selected) && meta.name != selected {
                continue;
            }
        }

        for &(cmd_signal, ref cmd) in meta.extension_commands {
            if cmd_signal != signal {
                continue;
            }

            // In grouped mode (no -p), only register the first occurrence of
            // each command name as a hidden clap subcommand. Duplicate names
            // from other providers are skipped at the clap level — this means
            // only the first provider's flags are registered for parsing.
            // Runtime dispatch resolves to the active provider regardless.
            // The help display (after_help/after_long_help) still shows all
            // providers' commands independently via help.rs.
            if show_grouped_help && !seen_command_names.insert(cmd.name) {
                continue;
            }

            let mut subcmd = Command::new(cmd.name).about(cmd.description);
            for fd in cmd.flags {
                // Extension command flags are command-specific, not provider-
                // injected global flags, so no provider tag or heading needed.
                subcmd = subcmd.arg(flag_descriptor_to_arg(fd, None, None));
            }
            subcmd = subcmd
                .arg(from_arg("Start time (e.g. now-1h, 2024-01-01T00:00:00Z)"))
                .arg(to_arg("End time (e.g. now, 2024-01-01T01:00:00Z)"));

            if show_grouped_help {
                subcmd = subcmd.hide(true);
            }

            signal_cmd = signal_cmd.subcommand(subcmd);
        }
    }

    signal_cmd
}

// ---------------------------------------------------------------------------
// Reusable Arg builders for common flags
// ---------------------------------------------------------------------------

/// Create a `--from` time argument.
fn from_arg(help: &'static str) -> Arg {
    Arg::new("from")
        .long("from")
        .allow_hyphen_values(true)
        .help(help)
}

/// Create a `--to` time argument.
fn to_arg(help: &'static str) -> Arg {
    Arg::new("to")
        .long("to")
        .allow_hyphen_values(true)
        .help(help)
}

/// Create a `-n/--limit` argument with the given default and help text.
fn limit_arg(default: &'static str, help: &'static str) -> Arg {
    Arg::new("limit")
        .short('n')
        .long("limit")
        .default_value(default)
        .value_parser(clap::value_parser!(usize))
        .help(help)
}

/// Create a `-m/--match` argument.
fn match_arg(help: &'static str) -> Arg {
    Arg::new("match").short('m').long("match").help(help)
}

/// Add connection/auth/debug flags that should only apply to query commands.
///
/// These flags are attached to the signal root (`metric`, `log`, `trace`) and
/// marked global so their subcommands inherit them. Utility commands such as
/// `config`, `provider`, and `completions` do not accept them.
fn with_query_global_args(cmd: Command) -> Command {
    cmd.arg(
        Arg::new("output")
            .short('o')
            .long("output")
            .global(true)
            .default_value("json")
            .value_parser(clap::builder::EnumValueParser::<CliOutputFormat>::new())
            .help("Output format"),
    )
    .arg(
        Arg::new("fields")
            .long("fields")
            .global(true)
            .help("Comma-separated list of fields to include in output (dot notation supported)"),
    )
    .arg(
        Arg::new("truncate")
            .long("truncate")
            .global(true)
            .value_parser(clap::builder::RangedU64ValueParser::<usize>::new().range(1..))
            .help("Truncate string values longer than N characters (min: 1)"),
    )
    .arg(
        Arg::new("provider")
            .short('p')
            .long("provider")
            .global(true)
            .help("Provider name or alias (e.g. vm, sls, my-sls-logs)")
            .help_heading(HEADING_CONNECTION),
    )
    .arg(
        Arg::new("endpoint")
            .long("endpoint")
            .global(true)
            .help("Provider endpoint URL (required)")
            .help_heading(HEADING_CONNECTION),
    )
    .arg(
        Arg::new("timeout")
            .long("timeout")
            .global(true)
            .help("HTTP request timeout (e.g. 60s, 2m) [default: 30s]")
            .help_heading(HEADING_CONNECTION),
    )
    .arg(
        Arg::new("verbose")
            .short('v')
            .long("verbose")
            .global(true)
            .action(ArgAction::SetTrue)
            .help("Print HTTP request and response details to stderr"),
    )
}

fn metric_command(registry: &ProviderRegistry, selected_provider: Option<&str>) -> Command {
    let (short_help, long_help) = help::metric_help(registry, selected_provider);
    let cmd = with_query_global_args(
        Command::new("metric")
            .about("Query and explore metrics")
            .after_help(short_help)
            .after_long_help(long_help)
            .subcommand_required(true)
            .arg_required_else_help(true)
            .subcommand(
                Command::new("query")
                    .about("Execute a metric query (instant or range)")
                    .arg(
                        Arg::new("query")
                            .short('q')
                            .long("query")
                            .required(true)
                            .help("Query expression"),
                    )
                    .arg(from_arg("Start time [default: now-1h]"))
                    .arg(to_arg("End time [default: now]"))
                    .arg(Arg::new("step").long("step").help(
                        "Range step (e.g. 15s, 1m). Auto-calculated to ~100 points if omitted",
                    ))
                    .arg(limit_arg("1000", "Maximum series returned")),
            )
            .subcommand(
                Command::new("list")
                    .about("List metric names")
                    .arg(match_arg("Filter expression"))
                    .arg(from_arg("Start time"))
                    .arg(to_arg("End time"))
                    .arg(limit_arg("100", "Maximum results")),
            )
            .subcommand(
                Command::new("info")
                    .about("Get metric metadata (type, description, unit)")
                    .arg(Arg::new("metric_name").required(true).help("Metric name")),
            )
            .subcommand(
                Command::new("labels")
                    .about("List label names")
                    .arg(match_arg("Series selector"))
                    .arg(from_arg("Start time"))
                    .arg(to_arg("End time")),
            )
            .subcommand(
                Command::new("label-values")
                    .about("List values for a specific label")
                    .arg(Arg::new("label_name").required(true).help("Label name"))
                    .arg(match_arg("Series selector"))
                    .arg(from_arg("Start time"))
                    .arg(to_arg("End time"))
                    .arg(limit_arg("100", "Maximum results")),
            )
            .subcommand(
                Command::new("series")
                    .about("Find series matching the given selectors")
                    .arg(
                        match_arg("Series selector (repeatable)")
                            .required(true)
                            .action(ArgAction::Append),
                    )
                    .arg(from_arg("Start time"))
                    .arg(to_arg("End time"))
                    .arg(limit_arg("1000", "Maximum results")),
            ),
    );

    // Register extension commands (e.g. provider-specific metric subcommands).
    add_extension_commands(cmd, registry, "metric", selected_provider)
}

fn log_command(registry: &ProviderRegistry, selected_provider: Option<&str>) -> Command {
    let (short_help, long_help) = help::log_help(registry, selected_provider);
    let cmd = with_query_global_args(
        Command::new("log")
            .about("Search and analyze logs")
            .after_help(short_help)
            .after_long_help(long_help)
            .subcommand_required(true)
            .arg_required_else_help(true)
            .subcommand(
                Command::new("search")
                    .about("Search for log entries")
                    .arg(
                        Arg::new("query")
                            .short('q')
                            .long("query")
                            .default_value("*")
                            .help("Query expression"),
                    )
                    .arg(from_arg("Start time [default: now-1h]"))
                    .arg(to_arg("End time [default: now]"))
                    .arg(limit_arg("100", "Maximum entries")),
            ),
    );

    // Register extension commands (e.g. provider-specific log subcommands).
    add_extension_commands(cmd, registry, "log", selected_provider)
}

fn trace_command(registry: &ProviderRegistry, selected_provider: Option<&str>) -> Command {
    let (short_help, long_help) = help::trace_help(registry, selected_provider);
    let cmd = with_query_global_args(
        Command::new("trace")
            .about("Search and inspect distributed traces")
            .after_help(short_help)
            .after_long_help(long_help)
            .subcommand_required(true)
            .arg_required_else_help(true)
            .subcommand(
                Command::new("search")
                    .about("Search for spans across traces")
                    .arg(
                        Arg::new("query")
                            .short('q')
                            .long("query")
                            .required(true)
                            .help("Service name or query expression"),
                    )
                    .arg(from_arg("Start time [default: now-1h]"))
                    .arg(to_arg("End time [default: now]"))
                    .arg(limit_arg("20", "Maximum traces returned")),
            )
            .subcommand(
                Command::new("get")
                    .about("Get all spans for a specific trace by ID")
                    .arg(Arg::new("trace_id").required(true).help("Trace ID (hex)"))
                    .arg(from_arg("Start time [default: now-1h]"))
                    .arg(to_arg("End time [default: now]")),
            ),
    );

    // Register extension commands (e.g. VT's `services` and `operations`).
    add_extension_commands(cmd, registry, "trace", selected_provider)
}

/// Build the `completions` subcommand.
fn completions_command() -> Command {
    Command::new("completions")
        .about("Generate shell completion scripts")
        .long_about(
            "Generate shell completion scripts for obz.\n\n\
             EXAMPLES:\n  \
             obz completions bash > ~/.local/share/bash-completion/completions/obz\n  \
             obz completions zsh  > ~/.zfunc/_obz\n  \
             obz completions fish > ~/.config/fish/completions/obz.fish",
        )
        .arg(
            Arg::new("shell")
                .required(true)
                .value_parser(clap::builder::EnumValueParser::<Shell>::new())
                .help("Target shell [bash|zsh|fish|powershell|elvish]"),
        )
}

/// Build the `generate-man-pages` subcommand (hidden — for packaging use).
fn man_pages_command() -> Command {
    Command::new("generate-man-pages")
        .about("Generate man pages for all commands")
        .long_about(
            "Generate ROFF man pages for obz and all subcommands.\n\n\
             This is a packaging helper — run it after building to produce\n\
             man pages that include dynamically registered provider commands.\n\n\
             EXAMPLES:\n  \
             mkdir -p man/man1 && obz generate-man-pages man/man1\n  \
             man man/man1/obz-metric-query.1",
        )
        .hide(true)
        .arg(
            Arg::new("out_dir")
                .required(true)
                .help("Output directory for .1 man page files"),
        )
}

/// Generate completion script for the given shell and write it to stdout.
///
/// Called from `main.rs` when the `completions` subcommand is matched,
/// before provider resolution — completions do not require `--provider`.
pub(crate) fn generate_completions(registry: &ProviderRegistry, shell: Shell) {
    clap_complete::generate(
        shell,
        &mut build_cli(registry, None),
        "obz",
        &mut std::io::stdout(),
    );
}

/// Build the `provider` subcommand.
fn provider_command() -> Command {
    Command::new("provider")
        .about("Provider management commands")
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommand(
            Command::new("list").about("List all registered providers and their supported signals"),
        )
        .subcommand(
            Command::new("check")
                .about("Check connectivity and authentication for configured providers")
                .arg(
                    Arg::new("name")
                        .help("Provider name to check (checks all configured providers if omitted)")
                        .num_args(1),
                ),
        )
}

fn skill_command() -> Command {
    Command::new("skills")
        .about("Manage AI agent skills for obz")
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommand(
            Command::new("list")
                .about("List available skills")
                .long_about(
                    "List all available skills with their names, descriptions, and providers.\n\n\
                     Use the output to choose which skills to install.",
                )
                .arg(
                    Arg::new("output")
                        .short('o')
                        .long("output")
                        .default_value("table")
                        .value_parser(clap::builder::EnumValueParser::<CliOutputFormat>::new())
                        .help("Output format"),
                ),
        )
        .subcommand(
            Command::new("show")
                .about("Preview skill content without installing")
                .long_about(
                    "Print the SKILL.md content for one or more skills.\n\n\
                     Use this to preview what will be installed before running\n\
                     `obz skills install`.",
                )
                .arg(
                    Arg::new("names")
                        .required(true)
                        .action(ArgAction::Append)
                        .help("Skill name(s) to show (see `obz skills list`)"),
                ),
        )
        .subcommand(
            Command::new("install")
                .about("Install skills to the specified directory")
                .long_about(
                    "Install skill files (SKILL.md) to the target directory.\n\n\
                     Three installation modes:\n\n\
                     1. Auto (default): installs core + skills matching your configured providers\n\
                     2. All:            installs every available skill (--all)\n\
                     3. Selective:      installs only the named skills\n\n\
                     EXAMPLES:\n\n  \
                     obz skills install --dir ~/.claude/skills --all\n  \
                     obz skills install --dir ~/.config/opencode/skills obz-vm obz-sls\n  \
                     obz skills install --dir ./skills",
                )
                .arg(
                    Arg::new("dir")
                        .long("dir")
                        .required(true)
                        .help("Target directory (e.g. ~/.claude/skills, ~/.config/opencode/skills)")
                        .long_help(
                            "Target directory for skill installation. Examples include\n\
                             `~/.claude/skills` and `~/.config/opencode/skills`.",
                        ),
                )
                .arg(
                    Arg::new("all")
                        .long("all")
                        .action(ArgAction::SetTrue)
                        .help("Install all available skills")
                        .long_help(
                            "Install all available skills.\n\n\
                             Without --all and without explicit names, only the core skill\n\
                             and skills matching providers in your config.yaml are installed.",
                        ),
                )
                .arg(
                    Arg::new("names")
                        .action(ArgAction::Append)
                        .help("Skill names to install (e.g. obz-vm obz-sls; see `obz skills list`)")
                        .long_help(
                            "Skill names to install. Accepts one or more names such as\n\
                             `obz-vm` and `obz-sls`. Discover valid names with `obz skills list`.",
                        ),
                ),
        )
}

/// Build long version string including commit, timestamp, and target.
///
/// Shown on `obz --version`:
///   `0.1.0 (abc1234 2026-04-03 x86_64-unknown-linux-gnu)`
///
/// Returns `&'static str` because clap requires it.
fn long_version() -> &'static str {
    use std::sync::OnceLock;
    static VERSION: OnceLock<String> = OnceLock::new();
    VERSION.get_or_init(|| {
        let version = env!("CARGO_PKG_VERSION");
        let commit = env!("OBZ_COMMIT_SHORT");
        let target = env!("OBZ_BUILD_TARGET");
        let epoch: i64 = env!("OBZ_BUILD_TIMESTAMP").parse().unwrap_or(0);
        let date = if epoch > 0 {
            jiff::Timestamp::from_second(epoch)
                .map(|ts| ts.strftime("%Y-%m-%d").to_string())
                .unwrap_or_else(|_| "unknown".to_string())
        } else {
            "unknown".to_string()
        };
        format!("{version} ({commit} {date} {target})")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::error::ErrorKind;
    use obz_core::descriptor::CommandDescriptor;
    use obz_core::registry::{ProviderMeta, SupportedCommands};
    use obz_core::{BuiltProvider, ObzError, ProviderConfig};

    /// Build a minimal dummy `ProviderMeta` for testing.
    fn dummy_meta(
        name: &'static str,
        aliases: &'static [&'static str],
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
            supported_commands: SupportedCommands::default(),
            build: dummy_build,
            check: None,
            command_flags: &[],
            extension_commands: ext_cmds,
        }
    }

    #[test]
    fn test_add_extension_commands_filters_by_signal() {
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

        let mut registry = ProviderRegistry::new();
        registry.register(dummy_meta("testprovider", &["tp"], EXT_CMDS));

        // Filtering for "trace" should register "services" but not "top-queries".
        let cmd = Command::new("trace").subcommand_required(false);
        let result = add_extension_commands(cmd, &registry, "trace", Some("testprovider"));
        let sub_names: Vec<&str> = result.get_subcommands().map(Command::get_name).collect();
        assert!(
            sub_names.contains(&"services"),
            "expected 'services' in {sub_names:?}"
        );
        assert!(
            !sub_names.contains(&"top-queries"),
            "should not contain 'top-queries' in {sub_names:?}"
        );

        // Filtering for "metric" should register "top-queries" but not "services".
        let cmd = Command::new("metric").subcommand_required(false);
        let result = add_extension_commands(cmd, &registry, "metric", Some("testprovider"));
        let sub_names: Vec<&str> = result.get_subcommands().map(Command::get_name).collect();
        assert!(
            sub_names.contains(&"top-queries"),
            "expected 'top-queries' in {sub_names:?}"
        );
        assert!(
            !sub_names.contains(&"services"),
            "should not contain 'services' in {sub_names:?}"
        );
    }

    #[test]
    fn test_add_extension_commands_grouped_mode_hides_duplicates_but_lists_by_provider() {
        static EXT_A: &[(&str, CommandDescriptor)] = &[(
            "trace",
            CommandDescriptor {
                name: "services",
                description: "Provider A services",
                flags: &[],
            },
        )];
        static EXT_B: &[(&str, CommandDescriptor)] = &[(
            "trace",
            CommandDescriptor {
                name: "services",
                description: "Provider B services",
                flags: &[],
            },
        )];

        let mut registry = ProviderRegistry::new();
        registry.register(dummy_meta("provider_a", &["pa"], EXT_A));
        registry.register(dummy_meta("provider_b", &["pb"], EXT_B));

        let cmd = Command::new("trace").subcommand_required(false);
        let mut result = add_extension_commands(cmd, &registry, "trace", None);

        // Hidden grouped mode should still only register one parseable subcommand
        // for a duplicate name, while after_help lists both providers.
        let services_count = result
            .get_subcommands()
            .filter(|s| s.get_name() == "services")
            .count();
        assert_eq!(services_count, 1, "duplicate 'services' registered");

        let help = result.render_help().to_string();
        assert!(!help.contains("EXTENSIONS:"));
        assert!(!help.contains("COMMANDS:\n  services"));
    }

    #[test]
    fn test_add_extension_commands_grouped_mode_hidden_commands_are_parseable() {
        static EXT_CMDS: &[(&str, CommandDescriptor)] = &[(
            "log",
            CommandDescriptor {
                name: "labels",
                description: "List labels",
                flags: &[],
            },
        )];

        let mut registry = ProviderRegistry::new();
        registry.register(dummy_meta("loki", &["loki"], EXT_CMDS));

        let mut cmd = add_extension_commands(Command::new("log"), &registry, "log", None);
        let matches = cmd
            .clone()
            .try_get_matches_from(["log", "labels"])
            .expect("hidden extension command should still parse");

        let Some((subcmd, _)) = matches.subcommand() else {
            panic!("expected labels subcommand");
        };
        assert_eq!(subcmd, "labels");

        let help = cmd.render_help().to_string();
        assert!(!help.contains("EXTENSIONS:"));
        assert!(!help.contains("COMMANDS:\n  labels"));
    }

    #[test]
    fn test_add_extension_commands_selected_provider_shows_only_visible_provider_commands() {
        static LOKI_EXTS: &[(&str, CommandDescriptor)] = &[
            (
                "log",
                CommandDescriptor {
                    name: "labels",
                    description: "List label names",
                    flags: &[],
                },
            ),
            (
                "log",
                CommandDescriptor {
                    name: "fields",
                    description: "List detected fields",
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
        registry.register(dummy_meta("loki", &["lo"], LOKI_EXTS));
        registry.register(dummy_meta("victorialogs", &["vl"], VLOGS_EXTS));

        let mut cmd = add_extension_commands(Command::new("log"), &registry, "log", Some("lo"));
        let sub_names: Vec<&str> = cmd.get_subcommands().map(Command::get_name).collect();

        assert_eq!(sub_names, vec!["labels", "fields"]);
        let help = cmd.render_help().to_string();
        assert!(!help.contains("EXTENSIONS:"));
        assert!(help.contains("labels  List label names"));
        assert!(help.contains("fields  List detected fields"));
        assert!(!help.contains("field-names"));
    }

    #[test]
    fn test_add_extension_commands_include_from_and_to_args() {
        static EXT_CMDS: &[(&str, CommandDescriptor)] = &[(
            "trace",
            CommandDescriptor {
                name: "services",
                description: "List services",
                flags: &[],
            },
        )];

        let mut registry = ProviderRegistry::new();
        registry.register(dummy_meta("testprovider", &["tp"], EXT_CMDS));

        let cmd = add_extension_commands(Command::new("trace"), &registry, "trace", Some("tp"));
        let services = cmd
            .get_subcommands()
            .find(|subcmd| subcmd.get_name() == "services")
            .expect("services extension should be registered");

        assert!(services
            .get_arguments()
            .any(|arg| arg.get_id().as_str() == "from"));
        assert!(services
            .get_arguments()
            .any(|arg| arg.get_id().as_str() == "to"));
    }

    #[test]
    fn test_extension_command_short_flag_is_registered() {
        static EXT_CMDS: &[(&str, CommandDescriptor)] = &[(
            "log",
            CommandDescriptor {
                name: "stats",
                description: "Run stats query",
                flags: &[FlagDescriptor {
                    name: "query",
                    flag_type: FlagType::String,
                    required: true,
                    default: None,
                    description: "Stats query",
                    repeatable: false,
                    short: Some('q'),
                }],
            },
        )];

        let mut registry = ProviderRegistry::new();
        registry.register(dummy_meta("testprovider", &["tp"], EXT_CMDS));

        let cmd = add_extension_commands(Command::new("log"), &registry, "log", Some("tp"));
        let stats = cmd
            .get_subcommands()
            .find(|s| s.get_name() == "stats")
            .expect("stats extension should be registered");

        let query_arg = stats
            .get_arguments()
            .find(|a| a.get_id().as_str() == "query")
            .expect("query arg should be registered");

        assert_eq!(query_arg.get_short(), Some('q'));
        assert_eq!(query_arg.get_long(), Some("query"));
    }

    #[test]
    fn test_extension_command_short_flag_none_omits_short() {
        static EXT_CMDS: &[(&str, CommandDescriptor)] = &[(
            "trace",
            CommandDescriptor {
                name: "operations",
                description: "List operations",
                flags: &[FlagDescriptor {
                    name: "service",
                    flag_type: FlagType::String,
                    required: true,
                    default: None,
                    description: "Service name",
                    repeatable: false,
                    short: None,
                }],
            },
        )];

        let mut registry = ProviderRegistry::new();
        registry.register(dummy_meta("testprovider", &["tp"], EXT_CMDS));

        let cmd = add_extension_commands(Command::new("trace"), &registry, "trace", Some("tp"));
        let ops = cmd
            .get_subcommands()
            .find(|s| s.get_name() == "operations")
            .expect("operations extension should be registered");

        let service_arg = ops
            .get_arguments()
            .find(|a| a.get_id().as_str() == "service")
            .expect("service arg should be registered");

        assert_eq!(service_arg.get_short(), None);
        assert_eq!(service_arg.get_long(), Some("service"));
    }

    #[test]
    fn test_long_version_format() {
        let v = long_version();
        // Should match pattern: "X.Y.Z (commit date target)"
        assert!(v.contains(env!("CARGO_PKG_VERSION")));
        assert!(v.contains(env!("OBZ_COMMIT_SHORT")));
        assert!(v.contains(env!("OBZ_BUILD_TARGET")));
        // Should contain parentheses
        assert!(v.contains('('));
        assert!(v.contains(')'));
    }

    fn help_text(args: &[&str]) -> String {
        let registry = ProviderRegistry::new();
        let err = build_cli(&registry, None)
            .try_get_matches_from(args)
            .expect_err("help should exit early");
        assert_eq!(err.kind(), ErrorKind::DisplayHelp);
        err.to_string()
    }

    #[test]
    fn test_utility_command_help_hides_unrelated_global_flags() {
        let completions = help_text(&["obz", "completions", "--help"]);
        assert!(completions.contains("Generate shell completion scripts for obz."));
        assert!(completions.contains("Usage: obz completions <shell>"));
        assert!(!completions.contains("--endpoint"));
        assert!(!completions.contains("--token"));
        assert!(!completions.contains("--verbose"));

        let man_pages = help_text(&["obz", "generate-man-pages", "--help"]);
        assert!(man_pages.contains("Generate ROFF man pages for obz and all subcommands."));
        assert!(man_pages.contains("Usage: obz generate-man-pages <out_dir>"));
        assert!(!man_pages.contains("--endpoint"));
        assert!(!man_pages.contains("--token"));
        assert!(!man_pages.contains("--verbose"));

        let provider = help_text(&["obz", "provider", "list", "--help"]);
        assert!(provider.contains("List all registered providers and their supported signals"));
        assert!(provider.contains("Usage: obz provider list"));
        assert!(!provider.contains("--endpoint"));
        assert!(!provider.contains("--token"));
        assert!(!provider.contains("--verbose"));

        let provider_check = help_text(&["obz", "provider", "check", "--help"]);
        assert!(provider_check
            .contains("Check connectivity and authentication for configured providers"));
        assert!(provider_check.contains("Usage: obz provider check [name]"));
        assert!(!provider_check.contains("--endpoint"));
        assert!(!provider_check.contains("--token"));
        assert!(!provider_check.contains("--verbose"));
    }

    #[test]
    fn test_provider_check_subcommand_is_registered() {
        let registry = ProviderRegistry::new();
        let matches = build_cli(&registry, None)
            .try_get_matches_from(["obz", "provider", "check", "vm"])
            .expect("provider check should parse");

        let Some(("provider", provider_matches)) = matches.subcommand() else {
            panic!("expected provider subcommand");
        };
        let Some(("check", check_matches)) = provider_matches.subcommand() else {
            panic!("expected provider check subcommand");
        };

        assert_eq!(
            check_matches.get_one::<String>("name").map(String::as_str),
            Some("vm")
        );
    }

    #[test]
    fn test_provider_registry_resolves_alias_to_canonical_provider_name() {
        let mut registry = ProviderRegistry::new();
        registry.register(ProviderMeta {
            name: "elasticsearch",
            display_name: "Elasticsearch",
            aliases: &["es", "elasticsearch"],
            supported_commands: SupportedCommands::default(),
            build: |_config: &ProviderConfig| unreachable!("not used in tests"),
            check: None,
            command_flags: &[],
            extension_commands: &[],
        });
        assert_eq!(registry.get("es").unwrap().name, "elasticsearch");
    }

    #[test]
    fn test_query_command_accepts_scoped_global_flags() {
        let registry = ProviderRegistry::new();
        let matches = build_cli(&registry, None)
            .try_get_matches_from([
                "obz",
                "metric",
                "--provider",
                "vm",
                "--endpoint",
                "http://localhost:8428",
                "--verbose",
                "query",
                "-q",
                "up",
            ])
            .expect("query command should accept signal-scoped global flags");

        let Some(("metric", metric_matches)) = matches.subcommand() else {
            panic!("expected metric subcommand");
        };

        assert_eq!(
            metric_matches
                .get_one::<String>("provider")
                .map(String::as_str),
            Some("vm")
        );
        assert_eq!(
            metric_matches
                .get_one::<String>("endpoint")
                .map(String::as_str),
            Some("http://localhost:8428")
        );
        assert!(metric_matches.get_flag("verbose"));
    }

    #[test]
    fn test_signal_help_shows_query_global_flags() {
        let help = help_text(&["obz", "metric", "--help"]);

        assert!(help.contains("--provider"));
        assert!(help.contains("--output"));
        assert!(help.contains("--endpoint"));
        assert!(help.contains("--timeout"));
        assert!(help.contains("--verbose"));
        assert!(!help.contains("--token"));
        assert!(!help.contains("--username"));
        assert!(!help.contains("--password"));
    }

    #[test]
    fn test_utility_commands_reject_query_global_flags() {
        let registry = ProviderRegistry::new();
        let err = build_cli(&registry, None)
            .try_get_matches_from(["obz", "completions", "--endpoint", "http://x", "bash"])
            .expect_err("completions should reject query-only flags");
        assert_eq!(err.kind(), ErrorKind::UnknownArgument);
    }
}
