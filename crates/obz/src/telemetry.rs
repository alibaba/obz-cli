use std::time::{Duration, SystemTime};

use opentelemetry::metrics::MeterProvider;
use opentelemetry::trace::{Span, SpanKind, Status, Tracer, TracerProvider};
use opentelemetry::KeyValue;
use opentelemetry_sdk::metrics::SdkMeterProvider;
use opentelemetry_sdk::trace::SdkTracerProvider;
use opentelemetry_sdk::Resource;

use obz_core::ErrorCode;

pub(crate) struct CliOutcome {
    pub exit_code: i32,
    pub error_type: Option<String>,
    pub module: String,
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

pub(crate) fn record(outcome: &CliOutcome, start_time: SystemTime, duration: Duration) {
    if std::env::var_os("OTEL_EXPORTER_OTLP_ENDPOINT").is_none() {
        return;
    }

    let _ = record_inner(outcome, start_time, duration);
}

fn record_inner(
    outcome: &CliOutcome,
    start_time: SystemTime,
    duration: Duration,
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

    let mut span = tracer
        .span_builder("obz")
        .with_kind(SpanKind::Internal)
        .with_start_time(start_time)
        .with_attributes(span_attrs)
        .start(&tracer);

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

#[cfg(test)]
mod tests {
    use super::*;

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
        // With no OTEL_EXPORTER_OTLP_ENDPOINT set, record should return immediately.
        let outcome = CliOutcome::success("metric");
        record(&outcome, SystemTime::now(), Duration::from_millis(100));
    }
}
