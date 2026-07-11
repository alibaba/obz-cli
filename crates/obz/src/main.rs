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
mod telemetry;

use std::io::Write;
use std::process::ExitCode;
use std::time::{Instant, SystemTime};

use obz_core::execute::ExecuteError;
use obz_core::model::response::Response;
use obz_core::registry::ProviderRegistry;

use telemetry::CliOutcome;

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
    // reqwest is built with "rustls-no-provider"; install ring before any TLS.
    let _ = rustls::crypto::ring::default_provider().install_default();

    install_panic_hook();

    let start_time = SystemTime::now();
    let start_instant = Instant::now();

    let traceparent = telemetry::downstream_traceparent();
    let outcome = run_cli(&traceparent);

    let duration = start_instant.elapsed();
    telemetry::record(&outcome, start_time, duration, &traceparent);

    ExitCode::from(outcome.exit_code as u8)
}

fn run_cli(traceparent: &str) -> CliOutcome {
    let registry = build_registry();

    let config_dir = std::env::var_os("OBZ_CONFIG_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| dirs_or_default("obz"));

    let raw_args: Vec<String> = std::env::args().collect();
    let selected_provider = raw_args.windows(2).find_map(|w| {
        if w[0] == "-p" || w[0] == "--provider" {
            Some(w[1].clone())
        } else {
            w[0].strip_prefix("--provider=").map(str::to_string)
        }
    });

    let selected_provider = selected_provider.or_else(|| {
        let cfg = match config::load(&config_dir) {
            Ok(c) => c,
            Err(e) => {
                let _ = writeln!(std::io::stderr(), "Warning: failed to load config: {e}");
                return None;
            }
        };
        let signal = extract_signal(&raw_args);
        signal.and_then(|s| cfg.default_provider(s).map(str::to_string))
    });

    let matches = cli::build_cli(&registry, selected_provider.as_deref()).get_matches();

    if let Some(("completions", sub)) = matches.subcommand() {
        let shell = *sub
            .get_one::<clap_complete::Shell>("shell")
            .expect("clap requires <shell>");
        cli::generate_completions(&registry, shell);
        return CliOutcome::success("cli");
    }

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
                return CliOutcome::success("cli");
            }
            Err(e) => {
                let _ = writeln!(std::io::stderr(), "Error generating man pages: {e}");
                return CliOutcome::error(1, None, "cli");
            }
        }
    }

    if let Some(("provider", sub)) = matches.subcommand() {
        if let Some(("list", _)) = sub.subcommand() {
            let obz_config = match config::load(&config_dir) {
                Ok(c) => c,
                Err(e) => {
                    let _ = writeln!(std::io::stderr(), "Error: {e}");
                    return CliOutcome::error(1, None, "provider");
                }
            };
            provider_cmd::list_providers(&registry, &obz_config, &config_dir);
            return CliOutcome::success("provider");
        }

        if let Some(("check", check_sub)) = sub.subcommand() {
            let obz_config = match config::load(&config_dir) {
                Ok(c) => c,
                Err(e) => {
                    let _ = writeln!(std::io::stderr(), "Error: {e}");
                    return CliOutcome::error(1, None, "provider");
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
                    return CliOutcome::error(1, None, "provider");
                }
            };

            let exit_code = runtime.block_on(provider_cmd::check_providers(
                &registry,
                &obz_config,
                name,
                &config_dir,
            ));
            let code = exit_code_to_i32(exit_code);
            if code == 0 {
                return CliOutcome::success("provider");
            }
            return CliOutcome::error(code, None, "provider");
        }
    }

    let signal_module = matches.subcommand_name().unwrap_or("cli").to_string();

    let (provider_for_errors, result) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("failed to build tokio runtime")
        .block_on(dispatch::run(&registry, matches, &config_dir, traceparent));

    match result {
        Ok(()) => CliOutcome::success(&signal_module),
        Err(ExecuteError::Io(e)) if e.kind() == std::io::ErrorKind::BrokenPipe => {
            CliOutcome::success(&signal_module)
        }
        Err(ExecuteError::Obz(obz_err)) => {
            let detail = obz_err.to_error_detail(provider_for_errors.as_deref());

            let _ = writeln!(std::io::stderr(), "Error: {}", detail.message);
            if let Some(ref chain) = detail.source_chain {
                for cause in chain {
                    let _ = writeln!(std::io::stderr(), "  Caused by: {cause}");
                }
            }
            if let Some(ref suggestion) = detail.suggestion {
                let _ = writeln!(std::io::stderr(), "Tip: {suggestion}");
            }

            let exit_code = detail.category.exit_code();

            let resp: Response<serde_json::Value> = Response::error(detail.clone());
            let stdout = std::io::stdout();
            let mut out = stdout.lock();
            let _ = serde_json::to_writer_pretty(&mut out, &resp);
            let _ = writeln!(out);

            CliOutcome::error(exit_code, Some(detail.code), &signal_module)
        }
        Err(e) => {
            let _ = writeln!(std::io::stderr(), "Error: {e}");
            CliOutcome::error(1, None, &signal_module)
        }
    }
}

fn exit_code_to_i32(code: ExitCode) -> i32 {
    if code == ExitCode::SUCCESS {
        0
    } else {
        1
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
