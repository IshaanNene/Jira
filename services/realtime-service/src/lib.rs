//! Realtime service: turns platform events into live hints for browsers (Server-Sent
//! Events), with a short replay buffer in Redis. See ADR-0008.

pub mod channels;
pub mod http;
pub mod hub;

use std::{net::SocketAddr, sync::Arc, time::Duration};

use async_trait::async_trait;
use auth_middleware::{GrpcPolicySource, PolicyClient, RedisHandle, TokenVerifier};
use axum::extract::FromRef;
use event_bus::{run_consumer, Event, Handler, HandlerError, KafkaSettings};
use event_contracts::topics;
use observability::{HttpLayerConfig, Telemetry};
use platform_common::{config, server};
use tokio::sync::Semaphore;

pub const SERVICE_NAME: &str = "realtime-service";
/// Events older than this are not announced: anyone connecting later loads fresh state.
const STALE_AFTER: Duration = Duration::from_secs(300);

#[derive(Clone)]
pub struct AppState {
    pub verifier: Arc<TokenVerifier>,
    pub policy: Arc<PolicyClient>,
    pub hub: Arc<hub::Hub>,
    /// Caps open streams per instance.
    pub slots: Arc<Semaphore>,
    /// How often an open stream re-checks the session and the caller's access.
    pub recheck: Duration,
    pub allowed_origins: Arc<Vec<String>>,
}

impl FromRef<AppState> for Arc<TokenVerifier> {
    fn from_ref(state: &AppState) -> Self {
        Arc::clone(&state.verifier)
    }
}

/// Consumer: announce each event on its channels.
pub struct Announcer {
    pub hub: Arc<hub::Hub>,
}

#[async_trait]
impl Handler for Announcer {
    async fn handle(&self, event: &Event) -> Result<(), HandlerError> {
        let (channels, hint) = channels::route(event);
        if channels.is_empty() {
            return Ok(());
        }
        let age = time::OffsetDateTime::now_utc() - event.envelope.occurred_at;
        if age > STALE_AFTER {
            return Ok(());
        }
        let tenant = event.tenant_id()?;
        let data = serde_json::to_string(&hint).map_err(|e| HandlerError::Permanent(e.into()))?;
        for channel in channels {
            // Hints are best effort: if Redis is down, clients resync when it returns.
            if let Err(error) = self.hub.publish(tenant, &channel, &data).await {
                metrics::counter!("realtime_dropped_total").increment(1);
                tracing::warn!(%error, %channel, "could not announce event");
            }
        }
        Ok(())
    }
}

pub const TOPICS: [&str; 5] = [topics::ISSUE, topics::AGILE, topics::PROJECT, topics::WORKFLOW, topics::NOTIFICATION];

pub async fn run(telemetry: Telemetry) -> anyhow::Result<Telemetry> {
    let http_addr: SocketAddr = config::or("HTTP_ADDR", "127.0.0.1:8109").parse()?;
    let secret = config::required("INTERNAL_SERVICE_SECRET")?;
    let permission = config::or("PERMISSION_SERVICE_GRPC_URL", "http://127.0.0.1:9108");
    let redis_url = config::required("REDIS_URL")?;
    let redis = RedisHandle::connect_in_background(&redis_url)?;
    let hub = hub::Hub::start(&redis_url, redis.clone())?;
    let max_connections: usize = config::or("REALTIME_MAX_CONNECTIONS", "10000").parse()?;

    match KafkaSettings::from_env(SERVICE_NAME) {
        Some(settings) => {
            let handler = Arc::new(Announcer { hub: Arc::clone(&hub) });
            tokio::spawn(async move {
                if let Err(error) = run_consumer(&settings, SERVICE_NAME, &TOPICS, handler).await {
                    tracing::error!(%error, "realtime consumer stopped");
                }
            });
        }
        None => tracing::warn!("KAFKA_BROKERS is not set: no events will be announced"),
    }

    let state = AppState {
        verifier: Arc::new(
            TokenVerifier::remote(config::required("TOKEN_ISSUER")?, config::required("JWKS_URL")?)?
                .with_revocations(redis.clone()),
        ),
        policy: Arc::new(PolicyClient::new(Arc::new(GrpcPolicySource::new(&permission, &secret, SERVICE_NAME)?), redis.clone())),
        hub,
        slots: Arc::new(Semaphore::new(max_connections)),
        recheck: Duration::from_secs(60),
        allowed_origins: Arc::new(config::list("ALLOWED_ORIGINS")),
    };
    let health = server::health_router(move || {
        let redis = redis.clone();
        async move { redis.is_connected() }
    });
    let app = observability::apply_http_layers(http::router(state), &HttpLayerConfig::default())
        .merge(health)
        .merge(observability::metrics_router(telemetry.metrics.clone()));
    server::serve_http(http_addr, app).await?;
    Ok(telemetry)
}
