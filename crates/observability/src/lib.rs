//! Logging, tracing and metrics setup shared by every service.
//!
//! * Logs: structured JSON on stdout (collected by Loki in Kubernetes).
//! * Traces: OpenTelemetry over OTLP/HTTP when `OTEL_EXPORTER_OTLP_ENDPOINT` is set.
//! * Metrics: Prometheus text format on `/metrics`.

mod http_layers;
mod propagation;

pub use http_layers::{apply_http_layers, metrics_router, HttpLayerConfig};
pub use propagation::{extract_remote_context, inject_into_metadata};

use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};
use opentelemetry::{global, trace::TracerProvider as _, KeyValue};
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::{propagation::TraceContextPropagator, runtime, trace::TracerProvider, Resource};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter, Layer};

/// Keeps exporters alive. Call [`Telemetry::shutdown`] before the process exits so
/// buffered spans are flushed.
pub struct Telemetry {
    provider: Option<TracerProvider>,
    pub metrics: PrometheusHandle,
}

impl Telemetry {
    pub fn shutdown(self) {
        if let Some(provider) = self.provider {
            if let Err(err) = provider.shutdown() {
                eprintln!("failed to flush traces: {err}");
            }
        }
    }
}

/// Install the global logger, tracer and metrics recorder. Call once, first thing in `main`.
pub fn init(service: &'static str) -> anyhow::Result<Telemetry> {
    global::set_text_map_propagator(TraceContextPropagator::new());

    let environment = platform_common::config::or("ENVIRONMENT", "development");
    let provider = match platform_common::config::optional("OTEL_EXPORTER_OTLP_ENDPOINT") {
        Some(endpoint) => {
            let exporter = opentelemetry_otlp::SpanExporter::builder()
                .with_http()
                .with_endpoint(format!("{}/v1/traces", endpoint.trim_end_matches('/')))
                .build()?;
            Some(
                TracerProvider::builder()
                    .with_batch_exporter(exporter, runtime::Tokio)
                    .with_resource(Resource::new(vec![
                        KeyValue::new("service.name", service),
                        KeyValue::new("deployment.environment", environment.clone()),
                    ]))
                    .build(),
            )
        }
        None => None,
    };

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,sqlx=warn,tower_http=info"));
    let pretty = platform_common::config::or("LOG_FORMAT", "json") == "pretty";
    let log_layer = if pretty {
        tracing_subscriber::fmt::layer().compact().boxed()
    } else {
        tracing_subscriber::fmt::layer().json().flatten_event(true).with_current_span(true).with_span_list(false).boxed()
    };
    let otel_layer = provider.as_ref().map(|p| tracing_opentelemetry::layer().with_tracer(p.tracer(service)));

    tracing_subscriber::registry().with(filter).with(log_layer).with(otel_layer).try_init()?;

    let metrics = PrometheusBuilder::new()
        .add_global_label("service", service)
        .add_global_label("environment", environment)
        .install_recorder()?;

    tracing::info!(service, "telemetry initialised");
    Ok(Telemetry { provider, metrics })
}
