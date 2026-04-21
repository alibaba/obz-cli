//! PromQL API response types — re-exported from the shared `promql` module.
//!
//! VictoriaMetrics uses the standard Prometheus HTTP API, so all response
//! types are shared with other PromQL-compatible providers (e.g. SLS).

// Re-export kept for test imports and future provider-specific overrides.
#[allow(unused_imports)]
pub(crate) use crate::promql::response::*;
