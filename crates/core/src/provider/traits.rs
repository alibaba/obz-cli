//! Provider trait definitions.
//!
//! One trait per observability signal. Each trait defines the operations
//! that a backend provider must implement for that signal.
//!
//! Traits use `#[async_trait]` so they can be stored as `dyn Trait` in the
//! [`ProviderRegistry`](crate::registry::ProviderRegistry). This allows
//! obz to store heterogeneous providers in a single registry without
//! knowing their concrete types at compile time.

use std::collections::BTreeMap;

use async_trait::async_trait;

use crate::model::metric::MetricInfoDetail;
use crate::model::trace::TraceDetail;

use super::params::{
    ExtensionParams, LabelValuesParams, LogSearchParams, MetricInfoParams, MetricMetadataParams,
    MetricQueryParams, TraceGetParams, TraceSearchParams,
};
use super::results::{
    ExtensionResult, LogSearchResult, MetricQueryResult, ProviderResult, TraceSearchResult,
};

/// Provider for metric queries.
#[async_trait]
pub trait MetricProvider: Send + Sync {
    /// Execute a metric query (instant or range).
    async fn query(&self, params: &MetricQueryParams) -> ProviderResult<MetricQueryResult>;

    /// List metric names.
    async fn list(&self, params: &MetricMetadataParams) -> ProviderResult<Vec<String>>;

    /// Get metric metadata (type, description, unit).
    async fn info(&self, params: &MetricInfoParams) -> ProviderResult<Vec<MetricInfoDetail>>;

    /// List label names.
    async fn labels(&self, params: &MetricMetadataParams) -> ProviderResult<Vec<String>>;

    /// List values for a specific label.
    async fn label_values(&self, params: &LabelValuesParams) -> ProviderResult<Vec<String>>;

    /// Find series matching the given selectors.
    async fn series(
        &self,
        params: &MetricMetadataParams,
    ) -> ProviderResult<Vec<BTreeMap<String, String>>>;
}

/// Provider for log queries.
#[async_trait]
pub trait LogProvider: Send + Sync {
    /// Search for log entries.
    async fn search(&self, params: &LogSearchParams) -> ProviderResult<LogSearchResult>;
}

/// Provider for trace queries.
#[async_trait]
pub trait TraceProvider: Send + Sync {
    /// Search for spans.
    async fn search(&self, params: &TraceSearchParams) -> ProviderResult<TraceSearchResult>;

    /// Get all spans for a specific trace.
    async fn get_trace(&self, params: &TraceGetParams) -> ProviderResult<TraceDetail>;
}

/// Provider for extension commands (provider-specific operations).
///
/// Extension commands are dynamic subcommands declared via
/// [`CommandDescriptor`](crate::descriptor::CommandDescriptor) and dispatched
/// through a single `execute` method keyed by command name.
#[async_trait]
pub trait ExtensionProvider: Send + Sync {
    /// Execute a provider-specific extension command.
    ///
    /// The `command` name matches the [`CommandDescriptor::name`](crate::descriptor::CommandDescriptor::name)
    /// declared in the provider's [`ProviderMeta`](crate::registry::ProviderMeta).
    async fn execute(
        &self,
        command: &str,
        params: &ExtensionParams,
    ) -> ProviderResult<ExtensionResult>;
}
