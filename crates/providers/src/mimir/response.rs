//! Re-export shared PromQL response types.
//!
//! Mimir exposes a 100% Prometheus-compatible HTTP API, so we reuse the
//! shared `promql` response deserialization types without modification.

// Re-export kept for test imports and future provider-specific overrides.
#[allow(unused_imports)]
pub(crate) use crate::promql::response::*;
