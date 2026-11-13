//! Agile service: boards, their columns, the order of issues, and sprints.

pub mod cleanup;
pub mod domain;
pub mod http;
pub mod rank;
pub mod sources;
pub mod store;

use std::{net::SocketAddr, sync::Arc};

use auth_middleware::{GrpcPolicySource, PolicyClient, RedisHandle, TokenVerifier};
use axum::extract::FromRef;
use observability::{HttpLayerConfig, Telemetry};
use platform_common::{config, server};
use sqlx::{migrate::Migrator, PgPool};
use tenant_context::{pools_from_env, DbRouter, PoolSettings, RegistryPlacement};

pub const SERVICE_NAME: &str = "agile-service";
pub static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

#[derive(Clone)]
pub struct AppState {
    pub db: Arc<DbRouter>,
    pub verifier: Arc<TokenVerifier>,
    pub policy: Arc<PolicyClient>,
    pub projects: Arc<dyn sources::Projects>,
    pub issues: Arc<dyn sources::Issues>,
    pub allowed_origins: Arc<Vec<String>>,
}

impl FromRef<AppState> for Arc<TokenVerifier> {
    fn from_ref(state: &AppState) -> Self {
        Arc::clone(&state.verifier)
    }
}

pub async fn migrate() -> anyhow::Result<()> {
    let pool = PgPool::connect(&config::required("DATABASE_MIGRATION_URL")?).await?;
    MIGRATOR.run(&pool).await?;
    pool.close().await;
    tracing::info!("migrations applied");
    Ok(())
}

pub async fn run(telemetry: Telemetry) -> anyhow::Result<Telemetry> {
    let http_addr: SocketAddr = config::or("HTTP_ADDR", "127.0.0.1:8107").parse()?;
    let secret = config::required("INTERNAL_SERVICE_SECRET")?;
    let registry = config::or("TENANT_SERVICE_GRPC_URL", "http://127.0.0.1:9102");
    let permission = config::or("PERMISSION_SERVICE_GRPC_URL", "http://127.0.0.1:9108");
    let project = config::or("PROJECT_SERVICE_GRPC_URL", "http://127.0.0.1:9104");
    let issue = config::or("ISSUE_SERVICE_GRPC_URL", "http://127.0.0.1:9105");

    let db = Arc::new(DbRouter::new(
        pools_from_env(&PoolSettings::from_env()?)?,
        Arc::new(RegistryPlacement::new(&registry, &secret, SERVICE_NAME)?),
    ));
    event_bus::spawn_relay(SERVICE_NAME, db.pools())?;
    match event_bus::KafkaSettings::from_env(SERVICE_NAME) {
        Some(settings) => {
            let handler = Arc::new(cleanup::Cleanup { db: Arc::clone(&db) });
            let topics = [event_contracts::topics::ISSUE, event_contracts::topics::PROJECT];
            tokio::spawn(async move {
                if let Err(error) = event_bus::run_consumer(&settings, cleanup::CONSUMER, &topics, handler).await {
                    tracing::error!(%error, "agile cleanup consumer stopped");
                }
            });
        }
        None => tracing::warn!("KAFKA_BROKERS is not set: placements of deleted issues are not cleaned up"),
    }
    let redis = RedisHandle::connect_in_background(&config::required("REDIS_URL")?)?;
    let state = AppState {
        db: Arc::clone(&db),
        verifier: Arc::new(
            TokenVerifier::remote(config::required("TOKEN_ISSUER")?, config::required("JWKS_URL")?)?
                .with_revocations(redis.clone()),
        ),
        policy: Arc::new(PolicyClient::new(Arc::new(GrpcPolicySource::new(&permission, &secret, SERVICE_NAME)?), redis)),
        projects: Arc::new(sources::GrpcProjects::new(&project, &secret)?),
        issues: Arc::new(sources::GrpcIssues::new(&issue, &secret)?),
        allowed_origins: Arc::new(config::list("ALLOWED_ORIGINS")),
    };

    let health = server::health_router(move || {
        let db = Arc::clone(&db);
        async move {
            match db.global() {
                Ok(pool) => sqlx::query("SELECT 1").execute(pool).await.is_ok(),
                Err(_) => false,
            }
        }
    });
    let app = observability::apply_http_layers(http::router(state), &HttpLayerConfig::default())
        .merge(health)
        .merge(observability::metrics_router(telemetry.metrics.clone()));
    server::serve_http(http_addr, app).await?;
    Ok(telemetry)
}
