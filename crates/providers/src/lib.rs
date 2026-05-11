//! Built-in provider implementations for the obz observability CLI.
//!
//! This crate contains concrete backend implementations that translate
//! between vendor-specific APIs and the unified obz data models defined
//! in [`obz_core`].
//!
//! # Public API
//!
//! - [`register_all`] — registers all built-in providers into the registry.
//! - [`probe`] — lightweight HTTP endpoint probes for provider connectivity checks.
//!
//! External crates must not depend on individual provider modules directly —
//! all provider access is through the [`obz_core::ProviderRegistry`] abstraction.
//!
//! # Adding a new provider
//!
//! 1. Create a module (e.g., `src/datadog/`) implementing the relevant
//!    [`obz_core::provider`] traits with `#[async_trait]`
//! 2. Add a `pub(crate) fn meta() -> ProviderMeta` in that module
//! 3. Add one line `registry.register(<name>::meta())` in [`register_all`]
//!
//! The obz shell (`main.rs`) never needs to change.

mod datadog;
mod elasticsearch;
mod greptimedb;
mod jaeger;
mod jaegerapi;
mod loki;
mod mimir;
mod opensearch;
pub mod probe;
mod prometheus;
mod promql;
mod sls;
mod tempo;
mod victorialogs;
mod victoriametrics;
mod victoriatraces;

mod util;

/// Register all built-in providers into the registry.
///
/// This is the **only** function the obz shell calls to set up providers.
/// Adding a new provider requires only a new module + one line here.
/// The obz shell never changes when new providers are added.
///
/// **Registration order matters** for extension commands and provider-
/// specific flags: when multiple providers declare the same command name
/// or flag name under the same signal, the **first registered** provider
/// wins for the help description shown to users. Runtime dispatch always
/// routes to the user-selected provider (`-p`), so behavior is correct
/// regardless of order — only help text is affected.
pub fn register_all(registry: &mut obz_core::ProviderRegistry) {
    registry.register(victoriametrics::meta());
    registry.register(victorialogs::meta());
    registry.register(victoriatraces::meta());
    registry.register(sls::meta());
    registry.register(datadog::meta());
    registry.register(prometheus::meta());
    registry.register(greptimedb::meta());
    registry.register(jaeger::meta());
    registry.register(opensearch::meta());
    registry.register(elasticsearch::meta());
    registry.register(mimir::meta());
    registry.register(loki::meta());
    registry.register(tempo::meta());
}
