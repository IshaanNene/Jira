//! HTTP serving with graceful shutdown, and the standard health endpoints.

use std::{future::Future, net::SocketAddr, sync::Arc};

use axum::{http::StatusCode, routing::get, Json, Router};
use serde_json::json;
use tokio::net::TcpListener;

/// Resolves on SIGINT or SIGTERM. Kubernetes sends SIGTERM before killing the pod.
pub async fn shutdown_signal() {
    let ctrl_c = async {
        if tokio::signal::ctrl_c().await.is_err() {
            std::future::pending::<()>().await;
        }
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut s) => {
                s.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! { () = ctrl_c => {}, () = terminate => {} }
    tracing::info!("shutdown signal received, draining");
}

pub async fn serve_http(addr: SocketAddr, router: Router) -> anyhow::Result<()> {
    let listener = TcpListener::bind(addr).await?;
    tracing::info!(%addr, "http server listening");
    axum::serve(listener, router.into_make_service_with_connect_info::<SocketAddr>())
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

/// `/health/live` (process is up), `/health/ready` (dependencies reachable),
/// `/health/startup` (same check as ready; separate path so probes can differ).
pub fn health_router<F, Fut>(ready: F) -> Router
where
    F: Fn() -> Fut + Send + Sync + 'static,
    Fut: Future<Output = bool> + Send + 'static,
{
    let ready = Arc::new(ready);
    let check = move || {
        let ready = Arc::clone(&ready);
        async move {
            if ready().await {
                (StatusCode::OK, Json(json!({ "status": "ok" })))
            } else {
                (StatusCode::SERVICE_UNAVAILABLE, Json(json!({ "status": "unavailable" })))
            }
        }
    };
    Router::new()
        .route("/health/live", get(|| async { Json(json!({ "status": "ok" })) }))
        .route("/health/ready", get(check.clone()))
        .route("/health/startup", get(check))
}
