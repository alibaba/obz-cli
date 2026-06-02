use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::time::{Duration, SystemTime};

use opentelemetry::metrics::MeterProvider;
use opentelemetry::trace::{
    Span, SpanContext, SpanId, SpanKind, Status, TraceContextExt, TraceFlags, TraceId, TraceState,
    Tracer, TracerProvider,
};
use opentelemetry::{Context, KeyValue};
use opentelemetry_sdk::metrics::SdkMeterProvider;
use opentelemetry_sdk::trace::SdkTracerProvider;
use opentelemetry_sdk::Resource;

use obz_core::ErrorCode;

pub(crate) struct CliOutcome {
    pub exit_code: i32,
    pub error_type: Option<String>,
    pub module: String,
}

// ---------------------------------------------------------------------------
// W3C Trace Context propagation
// ---------------------------------------------------------------------------

struct ParsedTraceparent {
    version: u8,
    trace_id: String,
    parent_id: String,
    trace_flags: u8,
}

fn parse_traceparent(value: &str) -> Option<ParsedTraceparent> {
    let parts: Vec<&str> = value.trim().split('-').collect();
    if parts.len() < 4 {
        return None;
    }
    let version = u8::from_str_radix(parts[0], 16).ok()?;
    if version == 0xff {
        return None;
    }
    let trace_id = parts[1];
    if trace_id.len() != 32 || trace_id.chars().all(|c| c == '0') {
        return None;
    }
    let parent_id = parts[2];
    if parent_id.len() != 16 || parent_id.chars().all(|c| c == '0') {
        return None;
    }
    let trace_flags = u8::from_str_radix(parts[3], 16).ok()?;
    Some(ParsedTraceparent {
        version,
        trace_id: trace_id.to_string(),
        parent_id: parent_id.to_string(),
        trace_flags,
    })
}

fn random_hex(len: usize) -> String {
    let s = RandomState::new();
    let mut h = s.build_hasher();
    h.write_u64(
        SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64,
    );
    h.write_u32(std::process::id());
    let v1 = h.finish();

    let s2 = RandomState::new();
    let mut h2 = s2.build_hasher();
    h2.write_u64(v1.wrapping_mul(6364136223846793005).wrapping_add(1));
    let v2 = h2.finish();

    let hex = format!("{v1:016x}{v2:016x}");
    hex[..len].to_string()
}

/// Build a downstream `traceparent` header value.
///
/// If `TRACEPARENT` is set in the environment, the trace-id and trace-flags
/// are inherited from the upstream context and a fresh span-id is generated
/// for the CLI invocation. Otherwise a completely new trace context is
/// created.
pub(crate) fn downstream_traceparent() -> String {
    if let Some(parsed) = std::env::var("TRACEPARENT")
        .ok()
        .as_deref()
        .and_then(parse_traceparent)
    {
        let child_span_id = random_hex(16);
        format!(
            "{:02x}-{}-{}-{:02x}",
            parsed.version, parsed.trace_id, child_span_id, parsed.trace_flags
        )
    } else {
        let trace_id = random_hex(32);
        let span_id = random_hex(16);
        format!("00-{trace_id}-{span_id}-01")
    }
}

impl CliOutcome {
    pub(crate) fn success(module: &str) -> Self {
        Self {
            exit_code: 0,
            error_type: None,
            module: module.to_string(),
        }
    }

    pub(crate) fn error(exit_code: i32, error_code: Option<ErrorCode>, module: &str) -> Self {
        Self {
            exit_code,
            error_type: error_type_for(exit_code, error_code),
            module: module.to_string(),
        }
    }
}

fn error_type_for(exit_code: i32, error_code: Option<ErrorCode>) -> Option<String> {
    if exit_code == 0 {
        return None;
    }
    match error_code {
        Some(ErrorCode::Timeout) => Some("timeout".to_string()),
        _ => Some("_OTHER".to_string()),
    }
}

pub(crate) fn record(
    outcome: &CliOutcome,
    start_time: SystemTime,
    duration: Duration,
    traceparent: &str,
) {
    if std::env::var_os("OTEL_EXPORTER_OTLP_ENDPOINT").is_none() {
        return;
    }

    let _ = record_inner(outcome, start_time, duration, traceparent);
}

fn record_inner(
    outcome: &CliOutcome,
    start_time: SystemTime,
    duration: Duration,
    traceparent: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let executable_path = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "obz".to_string());

    let service_name =
        std::env::var("OTEL_SERVICE_NAME").unwrap_or_else(|_| "obz".to_string());

    let resource = Resource::builder()
        .with_service_name(service_name)
        .with_attributes([KeyValue::new(
            "service.version",
            env!("CARGO_PKG_VERSION"),
        )])
        .build();

    let span_exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_http()
        .build()?;
    let tracer_provider = SdkTracerProvider::builder()
        .with_resource(resource.clone())
        .with_simple_exporter(span_exporter)
        .build();

    let metric_exporter = opentelemetry_otlp::MetricExporter::builder()
        .with_http()
        .build()?;
    let meter_provider = SdkMeterProvider::builder()
        .with_resource(resource)
        .with_periodic_exporter(metric_exporter)
        .build();

    // Execution Span (SpanKind::Internal per OTel CLI semantic conventions)
    let tracer = tracer_provider.tracer("obz");
    let end_time = start_time + duration;

    let mut span_attrs = vec![
        KeyValue::new("process.executable.name", "obz"),
        KeyValue::new("process.exit.code", i64::from(outcome.exit_code)),
        KeyValue::new("process.pid", i64::from(std::process::id())),
        KeyValue::new("process.executable.path", executable_path.clone()),
        KeyValue::new("process.module", outcome.module.clone()),
    ];
    if let Some(ref et) = outcome.error_type {
        span_attrs.push(KeyValue::new("error.type", et.clone()));
    }

    let parent_cx = build_parent_context(traceparent);

    let mut span = tracer
        .span_builder("obz")
        .with_kind(SpanKind::Internal)
        .with_start_time(start_time)
        .with_attributes(span_attrs)
        .start_with_context(&tracer, &parent_cx);

    if outcome.exit_code != 0 {
        span.set_status(Status::error(""));
    }
    span.end_with_timestamp(end_time);

    // Metrics
    let meter = meter_provider.meter("obz");

    let mut metric_attrs = vec![
        KeyValue::new("process.executable.path", executable_path),
        KeyValue::new("process.exit.code", i64::from(outcome.exit_code)),
        KeyValue::new("process.module", outcome.module.clone()),
        KeyValue::new("service.version", env!("CARGO_PKG_VERSION")),
    ];
    if let Some(ref et) = outcome.error_type {
        metric_attrs.push(KeyValue::new("error.type", et.clone()));
    }

    let counter = meter
        .u64_counter("process.cli.operation")
        .with_unit("count")
        .build();
    counter.add(1, &metric_attrs);

    let histogram = meter
        .f64_histogram("process.cli.duration")
        .with_unit("ms")
        .build();
    histogram.record(duration.as_millis() as f64, &metric_attrs);

    let _ = tracer_provider.shutdown();
    let _ = meter_provider.shutdown();

    Ok(())
}

/// Build an OTel [`Context`] from the downstream `traceparent` string we
/// generated earlier. The span recorded here becomes a child of the
/// upstream `TRACEPARENT` (if one was set), keeping the same trace-id.
fn build_parent_context(traceparent: &str) -> Context {
    let Some(parsed) = parse_traceparent(traceparent) else {
        return Context::current();
    };
    let Ok(trace_id_bytes) = hex_to_bytes::<16>(&parsed.trace_id) else {
        return Context::current();
    };
    let Ok(parent_id_bytes) = hex_to_bytes::<8>(&parsed.parent_id) else {
        return Context::current();
    };
    let trace_id = TraceId::from_bytes(trace_id_bytes);
    let span_id = SpanId::from_bytes(parent_id_bytes);
    let flags = TraceFlags::new(parsed.trace_flags);

    let span_context = SpanContext::new(trace_id, span_id, flags, true, TraceState::default());
    Context::current().with_remote_span_context(span_context)
}

fn hex_to_bytes<const N: usize>(hex: &str) -> Result<[u8; N], ()> {
    if hex.len() != N * 2 {
        return Err(());
    }
    let mut bytes = [0u8; N];
    for (i, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).map_err(|_| ())?;
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_traceparent_valid() {
        let tp = parse_traceparent("00-4bf92f3577b86cd56163f4d0e6c7318e-00f067aa0ba902b7-01");
        assert!(tp.is_some());
        let tp = tp.unwrap();
        assert_eq!(tp.version, 0);
        assert_eq!(tp.trace_id, "4bf92f3577b86cd56163f4d0e6c7318e");
        assert_eq!(tp.parent_id, "00f067aa0ba902b7");
        assert_eq!(tp.trace_flags, 1);
    }

    #[test]
    fn parse_traceparent_rejects_invalid() {
        assert!(parse_traceparent("").is_none());
        assert!(parse_traceparent("not-a-traceparent").is_none());
        // Invalid version 0xff
        assert!(parse_traceparent(
            "ff-4bf92f3577b86cd56163f4d0e6c7318e-00f067aa0ba902b7-01"
        )
        .is_none());
        // All-zero trace-id
        assert!(parse_traceparent(
            "00-00000000000000000000000000000000-00f067aa0ba902b7-01"
        )
        .is_none());
        // All-zero parent-id
        assert!(parse_traceparent(
            "00-4bf92f3577b86cd56163f4d0e6c7318e-0000000000000000-01"
        )
        .is_none());
        // Wrong trace-id length
        assert!(parse_traceparent("00-4bf92f-00f067aa0ba902b7-01").is_none());
        // Wrong parent-id length
        assert!(parse_traceparent(
            "00-4bf92f3577b86cd56163f4d0e6c7318e-00f067-01"
        )
        .is_none());
    }

    #[test]
    fn parse_traceparent_accepts_unsampled() {
        let tp = parse_traceparent("00-4bf92f3577b86cd56163f4d0e6c7318e-00f067aa0ba902b7-00");
        assert!(tp.is_some());
        assert_eq!(tp.unwrap().trace_flags, 0);
    }

    #[test]
    fn downstream_traceparent_format() {
        let tp = downstream_traceparent();
        let parts: Vec<&str> = tp.split('-').collect();
        assert_eq!(parts.len(), 4);
        assert_eq!(parts[0], "00");
        assert_eq!(parts[1].len(), 32);
        assert_eq!(parts[2].len(), 16);
        assert_eq!(parts[3].len(), 2);
    }

    #[test]
    fn hex_to_bytes_valid() {
        let result = hex_to_bytes::<4>("deadbeef");
        assert_eq!(result, Ok([0xde, 0xad, 0xbe, 0xef]));
    }

    #[test]
    fn hex_to_bytes_wrong_length() {
        assert!(hex_to_bytes::<4>("dead").is_err());
        assert!(hex_to_bytes::<4>("deadbeefaa").is_err());
    }

    #[test]
    fn error_type_success_returns_none() {
        assert_eq!(error_type_for(0, None), None);
        assert_eq!(error_type_for(0, Some(ErrorCode::Timeout)), None);
    }

    #[test]
    fn error_type_timeout() {
        assert_eq!(
            error_type_for(1, Some(ErrorCode::Timeout)),
            Some("timeout".to_string())
        );
    }

    #[test]
    fn error_type_other_errors() {
        assert_eq!(
            error_type_for(1, Some(ErrorCode::AuthMissing)),
            Some("_OTHER".to_string())
        );
        assert_eq!(
            error_type_for(2, Some(ErrorCode::InvalidFlag)),
            Some("_OTHER".to_string())
        );
        assert_eq!(
            error_type_for(3, Some(ErrorCode::BackendError)),
            Some("_OTHER".to_string())
        );
        assert_eq!(
            error_type_for(4, Some(ErrorCode::DnsError)),
            Some("_OTHER".to_string())
        );
        assert_eq!(
            error_type_for(5, Some(ErrorCode::NotSupported)),
            Some("_OTHER".to_string())
        );
        assert_eq!(
            error_type_for(1, None),
            Some("_OTHER".to_string())
        );
    }

    #[test]
    fn cli_outcome_success() {
        let outcome = CliOutcome::success("metric");
        assert_eq!(outcome.exit_code, 0);
        assert!(outcome.error_type.is_none());
        assert_eq!(outcome.module, "metric");
    }

    #[test]
    fn cli_outcome_error() {
        let outcome = CliOutcome::error(4, Some(ErrorCode::Timeout), "log");
        assert_eq!(outcome.exit_code, 4);
        assert_eq!(outcome.error_type.as_deref(), Some("timeout"));
        assert_eq!(outcome.module, "log");
    }

    #[test]
    fn record_noop_without_endpoint() {
        let outcome = CliOutcome::success("metric");
        record(
            &outcome,
            SystemTime::now(),
            Duration::from_millis(100),
            "00-4bf92f3577b86cd56163f4d0e6c7318e-00f067aa0ba902b7-01",
        );
    }
}
