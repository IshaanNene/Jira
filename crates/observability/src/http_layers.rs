//! The standard HTTP middleware stack. Every service applies the same one so that
//! request ids, timeouts, limits, logs and metrics behave identically everywhere.

use std::time::{Duration, Instant};

use axum::{
    extract::{DefaultBodyLimit, MatchedPath, Request},
    http::{header, HeaderName, HeaderValue, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use metrics_exporter_prometheus::PrometheusHandle;
use tower_http::{catch_panic::CatchPanicLayer, set_header::SetResponseHeaderLayer, timeout::TimeoutLayer, trace::TraceLayer};
use tracing_opentelemetry::OpenTelemetrySpanExt;

const REQUEST_ID: HeaderName = HeaderName::from_static("x-request-id");

#[derive(Debug, Clone)]
pub struct HttpLayerConfig {
    pub request_timeout: Duration,
    pub max_body_bytes: usize,
}

impl Default for HttpLayerConfig {
    fn default() -> Self {
        Self { request_timeout: Duration::from_secs(10), max_body_bytes: 1024 * 1024 }
    }
}

pub fn metrics_router(handle: PrometheusHandle) -> Router {
    Router::new().route("/metrics", get(move || std::future::ready(handle.render())))
}

/// Wrap `router` with request-id handling, tracing, metrics, timeout, body limit,
/// panic containment and baseline security headers.
pub fn apply_http_layers(router: Router, config: &HttpLayerConfig) -> Router {
    let trace = TraceLayer::new_for_http().make_span_with(|req: &http::Request<_>| {
        let request_id = req.headers().get(&REQUEST_ID).and_then(|v| v.to_str().ok()).unwrap_or("");
        let span = tracing::info_span!(
            "http.request",
            method = %req.method(),
            path = %req.uri().path(),
            request_id = %request_id,
            tenant_id = tracing::field::Empty,
            user_id = tracing::field::Empty,
        );
        span.set_parent(crate::extract_remote_context(req.headers()));
        span
    });

    router
        .layer(DefaultBodyLimit::max(config.max_body_bytes))
        .layer(TimeoutLayer::with_status_code(StatusCode::GATEWAY_TIMEOUT, config.request_timeout))
        .layer(middleware::from_fn(record_metrics))
        .layer(trace)
        .layer(middleware::from_fn(request_id_scope))
        .layer(SetResponseHeaderLayer::if_not_present(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff")))
        .layer(SetResponseHeaderLayer::if_not_present(header::REFERRER_POLICY, HeaderValue::from_static("no-referrer")))
        .layer(SetResponseHeaderLayer::if_not_present(header::CACHE_CONTROL, HeaderValue::from_static("no-store")))
        .layer(CatchPanicLayer::custom(|_| {
            tracing::error!("handler panicked");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }))
}

/// Accept a well-formed inbound `x-request-id` (set by the gateway) or mint one,
/// expose it to the task, and echo it on the response.
async fn request_id_scope(mut req: Request, next: Next) -> Response {
    let inbound = req
        .headers()
        .get(&REQUEST_ID)
        .and_then(|v| v.to_str().ok())
        .filter(|v| !v.is_empty() && v.len() <= 64 && v.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
        .map(str::to_owned);
    let id = inbound.unwrap_or_else(|| platform_common::new_id().to_string());
    let value = HeaderValue::from_str(&id).unwrap_or_else(|_| HeaderValue::from_static("invalid"));
    req.headers_mut().insert(REQUEST_ID, value.clone());
    let mut response = platform_common::request_context::scope(id, next.run(req)).await;
    response.headers_mut().insert(REQUEST_ID, value);
    response
}

async fn record_metrics(req: Request, next: Next) -> Response {
    let method = req.method().as_str().to_owned();
    // The route template, not the raw path: keeps label cardinality bounded.
    let route = req.extensions().get::<MatchedPath>().map_or_else(|| "unmatched".to_owned(), |p| p.as_str().to_owned());
    let started = Instant::now();
    let response = next.run(req).await;
    let status = response.status().as_u16().to_string();
    let labels = [("method", method), ("route", route), ("status", status)];
    metrics::counter!("http_requests_total", &labels).increment(1);
    metrics::histogram!("http_request_duration_seconds", &labels).record(started.elapsed().as_secs_f64());
    response
}
