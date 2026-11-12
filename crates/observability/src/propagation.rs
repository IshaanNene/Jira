//! W3C trace-context propagation across HTTP (inbound) and gRPC (outbound).

use opentelemetry::{
    global,
    propagation::{Extractor, Injector},
    Context,
};
use tracing_opentelemetry::OpenTelemetrySpanExt;

struct HeaderExtractor<'a>(&'a http::HeaderMap);

impl Extractor for HeaderExtractor<'_> {
    fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).and_then(|v| v.to_str().ok())
    }
    fn keys(&self) -> Vec<&str> {
        self.0.keys().map(http::HeaderName::as_str).collect()
    }
}

struct MetadataInjector<'a>(&'a mut tonic::metadata::MetadataMap);

impl Injector for MetadataInjector<'_> {
    fn set(&mut self, key: &str, value: String) {
        if let (Ok(key), Ok(value)) =
            (tonic::metadata::MetadataKey::from_bytes(key.as_bytes()), tonic::metadata::MetadataValue::try_from(value.as_str()))
        {
            self.0.insert(key, value);
        }
    }
}

/// Parent context carried by an incoming request's `traceparent` header.
pub fn extract_remote_context(headers: &http::HeaderMap) -> Context {
    global::get_text_map_propagator(|p| p.extract(&HeaderExtractor(headers)))
}

/// Copy the current span's trace context into outgoing gRPC metadata.
pub fn inject_into_metadata(metadata: &mut tonic::metadata::MetadataMap) {
    let cx = tracing::Span::current().context();
    global::get_text_map_propagator(|p| p.inject_context(&cx, &mut MetadataInjector(metadata)));
}
