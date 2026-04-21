//! Shared Jaeger HTTP API response types and conversion functions.
//!
//! This module contains the response deserialization and model conversion
//! code for the Jaeger HTTP Query API. It is shared between providers that
//! expose a Jaeger-compatible query interface:
//!
//! - **`VictoriaTraces`** — Jaeger-compatible API at `/select/jaeger/api/`
//! - **Jaeger** — native Jaeger API at `/api/`
//!
//! Both backends return identical JSON response formats, so a single set
//! of deserialization types and conversion functions serves both.

/// Jaeger API response deserialization types.
pub(crate) mod response;

/// Jaeger API response → obz model conversion functions.
pub(crate) mod convert;
