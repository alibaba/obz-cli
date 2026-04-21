//! Shared Prometheus-compatible (`PromQL`) response types, conversion functions,
//! and a reusable [`MetricProvider`] implementation.
//!
//! This module contains:
//! - **Response types** (`response`): Serde types for the standard Prometheus HTTP API JSON format.
//! - **Conversion** (`convert`): Transform Prometheus responses into obz unified data models.
//! - **Provider** (`provider`): A complete [`MetricProvider`] implementation that any
//!   PromQL-compatible backend can reuse instead of duplicating the six query methods.
//!
//! Shared across:
//! - **`VictoriaMetrics`** — native Prometheus-compatible API
//! - **Prometheus** — standard Prometheus HTTP API
//! - **Grafana Mimir** — Prometheus-compatible with multi-tenancy
//! - **SLS `MetricStore`** — `PromQL` gateway (response/convert only; SLS has custom auth)

/// Prometheus API response deserialization types.
pub(crate) mod response;

/// Prometheus API response → obz model conversion functions.
pub(crate) mod convert;

/// Shared [`MetricProvider`] implementation for PromQL-compatible backends.
pub(crate) mod provider;
