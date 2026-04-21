//! PromQL → obz model conversion — re-exported from the shared `promql` module.
//!
//! VictoriaMetrics uses the standard Prometheus HTTP API, so all conversion
//! functions are shared with other PromQL-compatible providers (e.g. SLS).

// Re-export kept for test imports and future provider-specific overrides.
#[allow(unused_imports)]
pub(crate) use crate::promql::convert::*;
