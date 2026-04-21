//! obz-core: Core framework for the obz observability CLI.
//!
//! This crate provides everything needed to build obz providers and
//! applications:
//!
//! - [`model`] — Unified data models and response envelope
//! - [`provider`] — Provider trait definitions (params, results, traits)
//! - [`descriptor`] — Extension command and flag descriptors (clap-ready)
//! - [`registry`] — Runtime provider catalog (`ProviderRegistry`)
//! - [`execute`] — Core command execution functions
//! - [`time`] — Time expression parser (now-1h, RFC3339, Unix timestamps)
//! - [`output`] — Output formatting pipeline (JSON, Table, CSV)
//! - [`cmd_path`] — Standard command identifiers for provider `command_flags` declarations

pub mod cmd_path;
pub mod descriptor;
pub mod execute;
pub mod model;
pub mod output;
pub mod provider;
pub mod registry;
pub mod time;

// Re-export common types at the crate root.
pub use cmd_path::StandardCommand;
pub use descriptor::{CommandDescriptor, FlagDescriptor, FlagType};
pub use model::error::{ErrorCategory, ErrorCode, ErrorDetail, ObzError};
pub use model::log::{LogEntry, Severity};
pub use model::metric::{DataPoint, MetricInfoDetail, MetricSeries, MetricType, SeriesStats};
pub use model::response::{
    ExtensionData, LabelValuesData, LogSearchData, MetricInfoData, MetricQueryData, QueryMetadata,
    Response, ResponseStatus, ScalarData, SeriesListData, StringListData, TimeRange,
    TraceDetailData, TraceSearchData,
};
pub use model::trace::{Span, SpanEvent, SpanKind, SpanStatus, TraceDetail};
pub use provider::{
    auth_missing_error, is_sensitive_key, ExtensionParams, ExtensionProvider, ExtensionResult,
    ProviderConfig, Signal,
};
pub use registry::{
    BuiltProvider, ProviderFactory, ProviderMeta, ProviderRegistry, SupportedCommands,
};
