//! obz: A multi-backend observability CLI tool.
//!
//! Pure shell — all framework logic lives in `obz-core`,
//! all backend implementations in `obz-providers`.
//!
//! Responsibilities of this binary:
//!   1. Build the clap command tree (see [`cli`])
//!   2. Register built-in provider metadata into the core `ProviderRegistry`
//!   3. After clap parses: resolve `--provider` + `--endpoint`, instantiate
//!      exactly one provider, then route to the appropriate core execute
//!      function (see [`dispatch`])
//!
//! No business logic lives here.

mod cli;
mod config;
mod credential_cache;
mod credential_process;
mod dispatch;
mod help;
mod manpage;
mod provider_cmd;
mod resolve;
mod skills;

use std::io::Write;
use std::process::ExitCode;

use obz_core::execute::ExecuteError;
use obz_core::model::response::Response;
use obz_core::registry::ProviderRegistry;

// ---------------------------------------------------------------------------
// Registry
//
// obz shell knows nothing about individual providers. All registration
// is delegated to obz_providers::register_all — adding a new data source
// only requires changes inside obz-providers, never here.
// ---------------------------------------------------------------------------

fn build_registry() -> ProviderRegistry {
    let mut registry = ProviderRegistry::new();
    obz_providers::register_all(&mut registry);
    registry
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

fn main() -> ExitCode {
    // Install ring as the default rustls crypto provider.
    // reqwest is built with "rustls-no-provider" to avoid pulling aws-lc-rs,
    // so we must install a provider before any TLS connections are made.
    // Using `let _ =` instead of `.expect()` because `install_default()`
    // returns `Err` if a provider is already installed (harmless).
    let _ = rustls::crypto::ring::default_provider().install_default();

    install_panic_hook();

    let registry = build_registry();

    // Resolve the configuration directory early — used both for pre-parse
    // (default provider in --help) and for dispatch.
    let config_dir = std::env::var_os("OBZ_CONFIG_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| dirs_or_default("obz"));

    // Pre-parse --provider / -p from raw args to customize --help output.
    let raw_args: Vec<String> = std::env::args().collect();
    let selected_provider = raw_args.windows(2).find_map(|w| {
        if w[0] == "-p" || w[0] == "--provider" {
            Some(w[1].clone())
        } else {
            w[0].strip_prefix("--provider=").map(str::to_string)
        }
    });

    // When no -p is given, check config for a default provider.
    // This allows --help output to show only the default provider's flags.
    let selected_provider = selected_provider.or_else(|| {
        let cfg = match config::load(&config_dir) {
            Ok(c) => c,
            Err(e) => {
                // Don't silently swallow config errors — the user should
                // know their YAML is broken even in --help mode.
                let _ = writeln!(std::io::stderr(), "Warning: failed to load config: {e}");
                return None;
            }
        };
        // Extract the signal subcommand (metric/log/trace) from raw args.
        // Only consider the first positional token (skip flags and their
        // values) to avoid mismatching parameter values like `--query metric`.
        let signal = extract_signal(&raw_args);
        signal.and_then(|s| cfg.default_provider(s).map(str::to_string))
    });

    let matches = cli::build_cli(&registry, selected_provider.as_deref()).get_matches();

    // Handle `completions` before provider resolution — it does not need --provider.
    if let Some(("completions", sub)) = matches.subcommand() {
        let shell = *sub
            .get_one::<clap_complete::Shell>("shell")
            .expect("clap requires <shell>");
        cli::generate_completions(&registry, shell);
        return ExitCode::SUCCESS;
    }

    // Handle `generate-man-pages` before provider resolution.
    if let Some(("generate-man-pages", sub)) = matches.subcommand() {
        let out_dir = sub
            .get_one::<String>("out_dir")
            .expect("clap requires <out_dir>");
        let cmd = cli::build_cli(&registry, None);
        match manpage::generate_all(&cmd, std::path::Path::new(out_dir)) {
            Ok(()) => {
                let count = std::fs::read_dir(out_dir)
                    .map(|rd| rd.filter_map(Result::ok).count())
                    .unwrap_or(0);
                eprintln!("Generated {count} man pages in {out_dir}");
                return ExitCode::SUCCESS;
            }
            Err(e) => {
                let _ = writeln!(std::io::stderr(), "Error generating man pages: {e}");
                return ExitCode::from(1);
            }
        }
    }

    // Handle `provider list` before provider resolution — it does not need --provider.
    if let Some(("provider", sub)) = matches.subcommand() {
        if let Some(("list", _)) = sub.subcommand() {
            // Missing config dir or missing files are fine (empty config),
            // but a *broken* config file (YAML parse error, unreadable) is
            // a real error — the user should fix it before trusting the
            // provider status display.
            let obz_config = match config::load(&config_dir) {
                Ok(c) => c,
                Err(e) => {
                    let _ = writeln!(std::io::stderr(), "Error: {e}");
                    return ExitCode::from(1);
                }
            };
            provider_cmd::list_providers(&registry, &obz_config, &config_dir);
            return ExitCode::SUCCESS;
        }

        if let Some(("check", check_sub)) = sub.subcommand() {
            let obz_config = match config::load(&config_dir) {
                Ok(c) => c,
                Err(e) => {
                    let _ = writeln!(std::io::stderr(), "Error: {e}");
                    return ExitCode::from(1);
                }
            };

            let name = check_sub.get_one::<String>("name").map(String::as_str);
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    let _ = writeln!(
                        std::io::stderr(),
                        "Error: failed to build tokio runtime: {error}"
                    );
                    return ExitCode::from(1);
                }
            };

            return runtime.block_on(provider_cmd::check_providers(
                &registry,
                &obz_config,
                name,
                &config_dir,
            ));
        }
    }

    let (provider_for_errors, result) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("failed to build tokio runtime")
        .block_on(dispatch::run(&registry, matches, &config_dir));

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(ExecuteError::Io(e)) if e.kind() == std::io::ErrorKind::BrokenPipe => ExitCode::SUCCESS,
        Err(ExecuteError::Obz(obz_err)) => {
            let detail = obz_err.to_error_detail(provider_for_errors.as_deref());

            // Human-readable summary to stderr.
            let _ = writeln!(std::io::stderr(), "Error: {}", detail.message);
            if let Some(ref chain) = detail.source_chain {
                for cause in chain {
                    let _ = writeln!(std::io::stderr(), "  Caused by: {cause}");
                }
            }
            if let Some(ref suggestion) = detail.suggestion {
                let _ = writeln!(std::io::stderr(), "Tip: {suggestion}");
            }

            let exit_code = detail.category.exit_code() as u8;

            // Structured JSON to stdout — AI Agents parse this.
            let resp: Response<serde_json::Value> = Response::error(detail);
            let stdout = std::io::stdout();
            let mut out = stdout.lock();
            let _ = serde_json::to_writer_pretty(&mut out, &resp);
            let _ = writeln!(out);

            ExitCode::from(exit_code)
        }
        Err(e) => {
            let _ = writeln!(std::io::stderr(), "Error: {e}");
            ExitCode::from(1)
        }
    }
}

/// Return the default config directory following platform conventions.
///
/// - Linux/macOS: `$XDG_CONFIG_HOME/<app>` or `~/.config/<app>`
/// - Windows: `%APPDATA%/<app>`
fn dirs_or_default(app: &str) -> std::path::PathBuf {
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
        return std::path::PathBuf::from(xdg).join(app);
    }
    // Windows: %APPDATA% (e.g. C:\Users\<user>\AppData\Roaming)
    if let Some(appdata) = std::env::var_os("APPDATA") {
        return std::path::PathBuf::from(appdata).join(app);
    }
    if let Some(home) = std::env::var_os("HOME") {
        return std::path::PathBuf::from(home).join(".config").join(app);
    }
    // Last resort
    std::path::PathBuf::from(".").join(format!(".{app}"))
}

fn install_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        default_hook(panic_info);
        eprintln!();
        eprintln!("This is a bug in obz. Please report it at:");
        eprintln!("  https://github.com/alibaba/obz-cli/issues/new");
        eprintln!(
            "obz {} ({} {})",
            env!("CARGO_PKG_VERSION"),
            env!("OBZ_COMMIT_SHORT"),
            env!("OBZ_BUILD_TARGET"),
        );
    }));
}

/// Extract the first signal subcommand (`metric`, `log`, or `trace`) from
/// raw argv, skipping flags and their values.
///
/// This is a lightweight pre-parse that avoids mismatching parameter values
/// like `--query metric` as a signal subcommand. It scans argv[1..] and
/// skips any token starting with `-`, plus the token immediately after a
/// value-bearing flag. Bool flags (like `--verbose`, `-v`, `-h`) do NOT
/// consume the next token.
///
/// Returns `None` if the first positional argument is not a known signal
/// (e.g. `obz completions bash`).
fn extract_signal(args: &[String]) -> Option<&str> {
    // Known bool flags that do not take a value argument.
    // IMPORTANT: update this list when adding new root-level bool flags to
    // build_cli() — any `SetTrue` arg that can appear before the signal
    // subcommand must be listed here. Note: -v/--verbose are now scoped to
    // signal commands (metric/log/trace) via with_query_global_args(), so
    // they cannot appear before the signal subcommand.
    const BOOL_FLAGS: &[&str] = &["-h", "--help", "-V", "--version"];

    let mut skip_next = false;
    // Skip args[0] (binary name).
    for arg in args.iter().skip(1) {
        if skip_next {
            skip_next = false;
            continue;
        }
        if arg.starts_with('-') {
            // Flags with `=` (e.g. --provider=vm) are self-contained.
            // Bool flags don't consume the next token.
            if !arg.contains('=') && !BOOL_FLAGS.contains(&arg.as_str()) {
                skip_next = true;
            }
            continue;
        }
        match arg.as_str() {
            "metric" | "log" | "trace" => return Some(arg.as_str()),
            // First positional is not a signal (e.g. "completions") — stop.
            _ => return None,
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper to build a `Vec<String>` from string slices.
    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn extract_signal_basic() {
        assert_eq!(
            extract_signal(&args(&["obz", "metric", "query"])),
            Some("metric")
        );
        assert_eq!(
            extract_signal(&args(&["obz", "log", "search"])),
            Some("log")
        );
        assert_eq!(
            extract_signal(&args(&["obz", "trace", "get"])),
            Some("trace")
        );
    }

    #[test]
    fn extract_signal_skips_value_flags() {
        assert_eq!(
            extract_signal(&args(&["obz", "-p", "vm", "metric", "query"])),
            Some("metric"),
        );
        assert_eq!(
            extract_signal(&args(&["obz", "--provider", "sls", "log", "search"])),
            Some("log"),
        );
    }

    #[test]
    fn extract_signal_handles_equals_form() {
        assert_eq!(
            extract_signal(&args(&["obz", "--provider=vm", "log"])),
            Some("log"),
        );
        assert_eq!(
            extract_signal(&args(&["obz", "--output=json", "trace"])),
            Some("trace"),
        );
    }

    #[test]
    fn extract_signal_skips_bool_flags() {
        assert_eq!(
            extract_signal(&args(&["obz", "-h", "metric"])),
            Some("metric")
        );
        assert_eq!(
            extract_signal(&args(&["obz", "-V", "metric"])),
            Some("metric")
        );
    }

    #[test]
    fn extract_signal_non_signal_positional() {
        assert_eq!(extract_signal(&args(&["obz", "completions", "bash"])), None);
        assert_eq!(extract_signal(&args(&["obz", "provider", "list"])), None);
    }

    #[test]
    fn extract_signal_empty() {
        assert_eq!(extract_signal(&args(&["obz"])), None);
        assert_eq!(extract_signal(&args(&[])), None);
    }

    #[test]
    fn extract_signal_does_not_match_flag_values() {
        // "--query metric" — "metric" is a flag value, not a signal.
        assert_eq!(
            extract_signal(&args(&["obz", "--query", "metric", "log"])),
            Some("log"),
        );
    }
}
