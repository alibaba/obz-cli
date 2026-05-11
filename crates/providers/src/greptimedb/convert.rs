//! Re-export shared PromQL conversion functions.
//!
//! GreptimeDB's Prometheus-compatible API returns identical response
//! payloads to standard Prometheus, so we reuse the shared conversion
//! logic from the `promql` module.

// Keep this re-export module parallel to PromQL-based providers that may add
// provider-specific conversion overrides later.
#[allow(unused_imports)]
pub(crate) use crate::promql::convert::*;
