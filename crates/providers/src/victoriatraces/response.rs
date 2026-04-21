//! Re-export shared Jaeger API response types.
//!
//! `VictoriaTraces` uses the standard Jaeger HTTP API response format.
//! All types are defined in the shared `jaegerapi` module.

pub(crate) use crate::jaegerapi::response::*;
