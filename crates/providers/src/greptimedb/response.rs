//! Re-export shared PromQL response types.
//!
//! GreptimeDB's Prometheus-compatible API uses the same response format
//! as standard Prometheus, so we reuse the shared `promql` response
//! deserialization types without modification.

#[allow(unused_imports)]
pub(crate) use crate::promql::response::*;
