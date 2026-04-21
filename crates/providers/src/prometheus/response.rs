//! Re-export shared Prometheus API response types.
//!
//! Prometheus uses the standard PromQL HTTP API response format.
//! All types are defined in the shared `promql` module.

// Re-export kept for test imports and future provider-specific overrides.
#[allow(unused_imports)]
pub(crate) use crate::promql::response::*;
