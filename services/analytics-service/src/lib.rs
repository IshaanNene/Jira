//! Analytics service: dashboards (PostgreSQL) and the charts on them, computed from facts in
//! ClickHouse that the ingester derives from issue, sprint and project events.
//!
//! ClickHouse holds nothing of record: drop the database, reset the consumer group's
//! offsets, and the ingester rebuilds it from the topics.

pub mod clickhouse;
pub mod http;
pub mod ingest;
pub mod source;
pub mod widgets;

use std::{net::SocketAddr, sync::Arc};

use auth_middleware::{GrpcPolicySource, PolicyClient, RedisHandle, TokenVerifier};
use axum::extract::FromRef;
use event_bus::{run_consumer, KafkaSettings};
use event_contracts::topics;
use observability::{HttpLayerConfig, Telemetry};
use platform_common::{config, server};
use sqlx::{migrate::Migrator, PgPool};
use tenant_context::{pools_from_env, DbRouter, PoolSettings, RegistryPlacement};

use crate::clickhouse::ClickHouse;

pub const SERVICE_NAME: &str = "analytics-service";
pub static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

#[derive(Clone)]
pub struct AppState {
    pub db: Arc<DbRouter>,
    /// Read-only ClickHouse user, limited by row policy to the tenant of each query.
    pub reader: ClickHouse,
    pub verifier: Arc<TokenVerifier>,
    pub policy: Arc<PolicyClient>,
    pub allowed_origins: Arc<Vec<String>>,
}

impl FromRef<AppState> for Arc<TokenVerifier> {
    fn from_ref(state: &AppState) -> Self {
        Arc::clone(&state.verifier)
    }
}

fn clickhouse_settings() -> anyhow::Result<clickhouse::Settings> {
    Ok(clickhouse::Settings {
        url: config::required("CLICKHOUSE_URL")?,
        database: config::or("CLICKHOUSE_DATABASE", "analytics"),
    })
}

fn clickhouse_user(role: &str) -> anyhow::Result<ClickHouse> {
    ClickHouse::new(
        &clickhouse_settings()?,
        &config::required(&format!("CLICKHOUSE_{role}_USER"))?,
        &config::required(&format!("CLICKHOUSE_{role}_PASSWORD"))?,
    )
}

/// PostgreSQL migrations, then the ClickHouse tables and row policies.
pub async fn migrate() -> anyhow::Result<()> {
    let pool = PgPool::connect(&config::required("DATABASE_MIGRATION_URL")?).await?;
    MIGRATOR.run(&pool).await?;
    pool.close().await;
    let owner = clickhouse_user("OWNER")?;
    clickhouse::migrate(&owner, &config::required("CLICKHOUSE_READER_USER")?, &config::required("CLICKHOUSE_WRITER_USER")?)
        .await?;
    tracing::info!("migrations applied");
    Ok(())
}

pub async fn run(telemetry: Telemetry) -> anyhow::Result<Telemetry> {
    let http_addr: SocketAddr = config::or("HTTP_ADDR", "127.0.0.1:8113").parse()?;
    let secret = config::required("INTERNAL_SERVICE_SECRET")?;
    let registry = config::or("TENANT_SERVICE_GRPC_URL", "http://127.0.0.1:9102");
    let permission = config::or("PERMISSION_SERVICE_GRPC_URL", "http://127.0.0.1:9108");
    let issue = config::or("ISSUE_SERVICE_GRPC_URL", "http://127.0.0.1:9105");

    let db = Arc::new(DbRouter::new(
        pools_from_env(&PoolSettings::from_env()?)?,
        Arc::new(RegistryPlacement::new(&registry, &secret, SERVICE_NAME)?),
    ));
    let redis = RedisHandle::connect_in_background(&config::required("REDIS_URL")?)?;
    let policy = Arc::new(PolicyClient::new(Arc::new(GrpcPolicySource::new(&permission, &secret, SERVICE_NAME)?), redis.clone()));
    let reader = clickhouse_user("READER")?;

    match KafkaSettings::from_env(SERVICE_NAME) {
        Some(settings) => {
            let handler = Arc::new(ingest::Ingester {
                store: clickhouse_user("WRITER")?,
                source: Arc::new(source::GrpcSource::new(&issue, &secret)?),
            });
            tokio::spawn(async move {
                if let Err(error) =
                    run_consumer(&settings, ingest::CONSUMER, &[topics::ISSUE, topics::AGILE, topics::PROJECT], handler).await
                {
                    tracing::error!(%error, "analytics ingester stopped");
                }
            });
        }
        None => tracing::warn!("KAFKA_BROKERS is not set: charts will not follow changes"),
    }

    let state = AppState {
        db: Arc::clone(&db),
        reader: reader.clone(),
        verifier: Arc::new(
            TokenVerifier::remote(config::required("TOKEN_ISSUER")?, config::required("JWKS_URL")?)?.with_revocations(redis),
        ),
        policy,
        allowed_origins: Arc::new(config::list("ALLOWED_ORIGINS")),
    };
    let health = server::health_router(move || {
        let (db, reader) = (Arc::clone(&db), reader.clone());
        async move {
            let pg = match db.global() {
                Ok(pool) => sqlx::query("SELECT 1").execute(pool).await.is_ok(),
                Err(_) => false,
            };
            pg && reader.ping().await
        }
    });
    let app = observability::apply_http_layers(http::router(state), &HttpLayerConfig::default())
        .merge(health)
        .merge(observability::metrics_router(telemetry.metrics.clone()));
    server::serve_http(http_addr, app).await?;
    Ok(telemetry)
}
