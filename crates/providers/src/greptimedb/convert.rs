//! Re-export shared PromQL conversion functions.
//!
//! GreptimeDB's Prometheus-compatible API returns identical response
//! payloads to standard Prometheus, so we reuse the shared conversion
//! logic from the `promql` module.

#[allow(unused_imports)]
pub(crate) use crate::promql::convert::*;
