//! Provider registry.
//!
//! The registry is the runtime catalog of available providers.
//! It is analogous to `DuckDB`'s catalog — providers register their
//! metadata and factory functions here at startup. The obz shell then
//! looks up a provider by name and instantiates it on demand with the
//! user-supplied endpoint and credentials.
//!
//! # Why lazy instantiation?
//!
//! Different providers use different default ports and authentication
//! schemes. The endpoint is supplied by the user at invocation time via
//! `--endpoint`, so providers cannot be instantiated until after argument
//! parsing. The registry stores factory functions; the shell calls the
//! right one after resolving `--provider` and `--endpoint`.
//!
//! # Registration flow (performed by the obz shell at startup)
//!
//! ```text
//! obz main()
//!   → build ProviderRegistry
//!   → registry.register(ProviderMeta { name: "<provider>", aliases: &[...], ... })
//!   → ... (repeat for each built-in provider)
//!   → clap parses args
//!   → registry.get("<alias>")? → &ProviderMeta
//!   → meta.build(config)? → BuiltProvider { metric: Some(Box<dyn MetricProvider>), ... }
//! ```

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use crate::cmd_path::StandardCommand;
use crate::descriptor::{CommandDescriptor, FlagDescriptor};
use crate::model::error::{ErrorCode, ObzError};
use crate::provider::traits::{ExtensionProvider, LogProvider, MetricProvider, TraceProvider};
use crate::provider::ProviderConfig;

/// A fully instantiated provider ready to execute commands.
///
/// Built by calling [`ProviderMeta::build`] after the user's endpoint
/// and credentials are known. Each signal capability is `None` if the
/// provider does not support that signal.
pub struct BuiltProvider {
    /// The canonical provider name (for use in response metadata).
    pub name: &'static str,
    /// Query language for metric queries (e.g. `"MetricsQL"`).
    pub metric_query_language: Option<&'static str>,
    /// Query language for log queries (e.g. `"LogsQL"`).
    pub log_query_language: Option<&'static str>,
    /// Metric provider implementation.
    pub metric: Option<Box<dyn MetricProvider>>,
    /// Log provider implementation.
    pub log: Option<Box<dyn LogProvider>>,
    /// Trace provider implementation.
    pub trace: Option<Box<dyn TraceProvider>>,
    /// Extension command provider implementation.
    pub extension: Option<Box<dyn ExtensionProvider>>,
}

/// Type alias for the provider factory function.
///
/// Takes connection config and returns a fully built provider or an error
/// if the HTTP client cannot be constructed (e.g. invalid TLS config).
pub type ProviderFactory = fn(&ProviderConfig) -> Result<BuiltProvider, ObzError>;

/// Result of a provider-specific health check.
#[derive(Debug, Clone)]
pub struct CheckResult {
    /// Severity level of the check outcome.
    pub severity: CheckSeverity,
    /// Human-readable status message.
    pub message: String,
    /// What the check verified.
    pub scope: CheckScope,
    /// End-to-end latency for the live check when a request was attempted.
    pub latency: Option<Duration>,
}

/// Severity level for a provider check result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckSeverity {
    /// Check passed successfully.
    Ok,
    /// Check completed but with a warning (e.g. server error, degraded).
    Warn,
    /// Check failed.
    Fail,
}

/// What a provider check verified.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckScope {
    /// Only network connectivity was verified.
    Connectivity,
    /// Both connectivity and authentication were verified.
    ConnectivityAndAuth,
    /// Credentials are configured but the check could not verify them.
    ConfiguredNotVerifiable,
}

/// Type alias for an async provider check function.
///
/// The function takes the provider's resolved config and returns a `CheckResult`.
pub type ProviderCheckFn =
    fn(&ProviderConfig) -> Pin<Box<dyn Future<Output = CheckResult> + Send + '_>>;

/// Which standard (core) commands a provider supports.
///
/// Used by the obz shell to generate accurate `--help` output showing
/// provider support tags next to each core command.
/// Set a field to `true` if the provider implements that operation.
#[derive(Debug, Clone, Copy, Default)]
pub struct SupportedCommands {
    // --- metric ---
    pub metric_query: bool,
    pub metric_list: bool,
    pub metric_info: bool,
    pub metric_labels: bool,
    pub metric_label_values: bool,
    pub metric_series: bool,
    // --- log ---
    pub log_search: bool,
    // --- trace ---
    pub trace_search: bool,
    pub trace_get: bool,
}

/// Static metadata and factory for a single provider.
///
/// Stored in the registry; does not hold any live connections or state.
pub struct ProviderMeta {
    /// Canonical name used as the primary `--provider` value.
    pub name: &'static str,
    /// Human-readable display name shown in help and diagnostics.
    pub display_name: &'static str,
    /// All accepted `--provider` aliases, including the canonical name.
    /// Insertion order is preserved for deterministic `--help` output.
    pub aliases: &'static [&'static str],
    /// Which standard core commands this provider supports.
    /// Used to generate provider support tags in `--help` output.
    pub supported_commands: SupportedCommands,
    /// Factory function — instantiates the provider from a `ProviderConfig`.
    pub build: ProviderFactory,
    /// Optional health check function registered by the provider.
    ///
    /// When `Some`, `provider check` calls this to verify connectivity
    /// and (where possible) authentication. When `None`, the check is skipped.
    pub check: Option<ProviderCheckFn>,
    /// Provider-specific flags per standard command.
    /// Key is the standard command identifier for the target core command.
    pub command_flags: &'static [(StandardCommand, &'static [FlagDescriptor])],
    /// Extension commands registered by this provider.
    ///
    /// Each entry is a `(signal, descriptor)` pair where `signal` is the
    /// signal group the command belongs to (e.g. `"trace"`, `"metric"`).
    /// Unlike `command_flags` (which uses [`StandardCommand`] identifiers), extension commands
    /// use a bare signal name because the command name itself lives in
    /// the descriptor.
    pub extension_commands: &'static [(&'static str, CommandDescriptor)],
}

/// Runtime catalog of available providers.
///
/// Built once at startup by the obz shell. Providers are not instantiated
/// until the user's `--provider` and `--endpoint` are known.
#[derive(Default)]
pub struct ProviderRegistry {
    /// Alias → index into `metas`. `BTreeMap` for deterministic ordering.
    by_alias: BTreeMap<String, usize>,
    /// Insertion-ordered list of provider metadata.
    metas: Vec<ProviderMeta>,
}

impl ProviderRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a provider.
    ///
    /// # Panics
    ///
    /// Panics if any alias conflicts with an already-registered provider.
    /// This is a programming error caught at startup, not a runtime error.
    pub fn register(&mut self, meta: ProviderMeta) {
        let idx = self.metas.len();
        for &alias in meta.aliases {
            assert!(
                self.by_alias.insert(alias.to_string(), idx).is_none(),
                "provider alias '{}' is already registered",
                alias
            );
        }
        self.metas.push(meta);
    }

    /// Look up provider metadata by name or alias.
    ///
    /// # Errors
    ///
    /// Returns [`ObzError::InvalidArgument`] if the name is not registered.
    pub fn get(&self, name: &str) -> Result<&ProviderMeta, ObzError> {
        self.by_alias
            .get(name)
            .map(|&i| &self.metas[i])
            .ok_or_else(|| ObzError::InvalidArgument {
                code: ErrorCode::InvalidFlag,
                message: format!(
                    "unknown provider '{}' — run `obz provider list` to see available providers",
                    name
                ),
                suggestion: None,
            })
    }

    /// All registered provider metadata in insertion order.
    pub fn all(&self) -> &[ProviderMeta] {
        &self.metas
    }

    /// All accepted alias strings in sorted order (for clap validation).
    pub fn all_aliases(&self) -> Vec<&str> {
        self.by_alias.keys().map(String::as_str).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal dummy `ProviderMeta` for testing.
    ///
    /// Uses `SupportedCommands::default()` and no extension commands — the
    /// registry tests only care about name/alias resolution, not capabilities.
    fn dummy_meta(name: &'static str, aliases: &'static [&'static str]) -> ProviderMeta {
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
            extension_commands: &[],
        }
    }

    #[test]
    fn test_get_resolves_provider_by_name_and_alias() {
        let mut registry = ProviderRegistry::new();
        registry.register(dummy_meta("victoriametrics", &["vm", "victoriametrics"]));

        let meta = registry.get("vm").unwrap();
        assert_eq!(meta.name, "victoriametrics");

        let meta2 = registry.get("victoriametrics").unwrap();
        assert_eq!(meta2.name, "victoriametrics");
    }

    #[test]
    fn test_get_returns_error_for_unknown_provider() {
        let registry = ProviderRegistry::new();
        match registry.get("nonexistent") {
            Err(ObzError::InvalidArgument { code, message, .. }) => {
                assert_eq!(code, ErrorCode::InvalidFlag);
                assert!(message.contains("nonexistent"));
            }
            Err(other) => panic!("expected InvalidArgument, got {other:?}"),
            Ok(_) => panic!("expected error, got Ok"),
        }
    }

    #[test]
    fn test_all_preserves_insertion_order() {
        let mut registry = ProviderRegistry::new();
        registry.register(dummy_meta("alpha", &["alpha"]));
        registry.register(dummy_meta("beta", &["beta"]));
        registry.register(dummy_meta("gamma", &["gamma"]));

        let names: Vec<&str> = registry.all().iter().map(|m| m.name).collect();
        assert_eq!(names, vec!["alpha", "beta", "gamma"]);
    }

    #[test]
    fn test_all_aliases_returns_sorted_list() {
        let mut registry = ProviderRegistry::new();
        registry.register(dummy_meta("victoriametrics", &["vm", "victoriametrics"]));
        registry.register(dummy_meta("victorialogs", &["vl", "victorialogs"]));

        let aliases = registry.all_aliases();
        // BTreeMap is sorted, so aliases should be in alphabetical order.
        assert_eq!(aliases, vec!["victorialogs", "victoriametrics", "vl", "vm"]);
    }

    #[test]
    #[should_panic(expected = "provider alias 'vm' is already registered")]
    fn test_register_panics_on_duplicate_alias() {
        let mut registry = ProviderRegistry::new();
        registry.register(dummy_meta("provider1", &["vm"]));
        registry.register(dummy_meta("provider2", &["vm"])); // should panic
    }

    #[test]
    fn test_new_registry_is_empty() {
        let registry = ProviderRegistry::new();
        assert!(registry.all().is_empty());
        assert!(registry.all_aliases().is_empty());
    }

    #[test]
    fn test_registry_with_extension_commands() {
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
                    description: "Top queries",
                    flags: &[],
                },
            ),
        ];

        fn build_with_ext(_: &ProviderConfig) -> Result<BuiltProvider, ObzError> {
            Ok(BuiltProvider {
                name: "test",
                metric_query_language: None,
                log_query_language: None,
                metric: None,
                log: None,
                trace: None,
                extension: None,
            })
        }

        let meta = ProviderMeta {
            name: "testprov",
            display_name: "TestProvider",
            aliases: &["tp"],
            supported_commands: SupportedCommands::default(),
            build: build_with_ext,
            check: None,
            command_flags: &[],
            extension_commands: EXT_CMDS,
        };

        let mut registry = ProviderRegistry::new();
        registry.register(meta);

        let retrieved = registry.get("tp").unwrap();
        assert_eq!(retrieved.extension_commands.len(), 2);
        assert_eq!(retrieved.extension_commands[0].0, "trace");
        assert_eq!(retrieved.extension_commands[0].1.name, "services");
        assert_eq!(retrieved.extension_commands[1].0, "metric");
        assert_eq!(retrieved.extension_commands[1].1.name, "top-queries");
    }
}
