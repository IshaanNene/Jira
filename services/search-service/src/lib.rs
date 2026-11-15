//! Search service: full-text issue search on OpenSearch, kept current from issue events.
//!
//! The index is not data of record: it can be dropped and rebuilt at any time from
//! issue-service (`search-service reindex <tenant-id>`). Saved filters are, and live in PostgreSQL.

pub mod filters;
pub mod http;
pub mod index;
pub mod indexer;
pub mod mql;
pub mod people;
pub mod source;
pub mod subscriptions;

use std::{net::SocketAddr, sync::Arc};

use auth_middleware::{GrpcPolicySource, PolicyClient, RedisHandle, TokenVerifier};
use axum::extract::FromRef;
use event_bus::{run_consumer, KafkaSettings};
use event_contracts::topics;
use observability::{HttpLayerConfig, Telemetry};
use platform_common::{config, server};
use sqlx::{migrate::Migrator, PgPool};
use tenant_context::{pools_from_env, DbRouter, PoolSettings, RegistryPlacement};
use uuid::Uuid;

pub const SERVICE_NAME: &str = "search-service";
pub static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

#[derive(Clone)]
pub struct AppState {
    pub index: index::Index,
    /// Saved filters.
    pub db: Arc<DbRouter>,
    pub verifier: Arc<TokenVerifier>,
    pub policy: Arc<PolicyClient>,
    pub people: Arc<dyn people::People>,
    pub allowed_origins: Arc<Vec<String>>,
}

impl FromRef<AppState> for Arc<TokenVerifier> {
    fn from_ref(state: &AppState) -> Self {
        Arc::clone(&state.verifier)
    }
}

fn open_index() -> anyhow::Result<index::Index> {
    index::Index::new(&config::required("OPENSEARCH_URL")?, &config::or("SEARCH_INDEX", "issues-v1"))
}

async fn ensure_index(index: &index::Index) -> anyhow::Result<()> {
    let shards = config::or("SEARCH_INDEX_SHARDS", "1").parse()?;
    let replicas = config::or("SEARCH_INDEX_REPLICAS", "0").parse()?;
    index.ensure(shards, replicas).await.map_err(|e| anyhow::anyhow!("search index unavailable: {e}"))
}

fn indexer(index: index::Index) -> anyhow::Result<indexer::Indexer> {
    let secret = config::required("INTERNAL_SERVICE_SECRET")?;
    let issue = config::or("ISSUE_SERVICE_GRPC_URL", "http://127.0.0.1:9105");
    Ok(indexer::Indexer { index, source: Arc::new(source::GrpcSource::new(&issue, &secret)?) })
}

pub async fn migrate() -> anyhow::Result<()> {
    let pool = PgPool::connect(&config::required("DATABASE_MIGRATION_URL")?).await?;
    MIGRATOR.run(&pool).await?;
    pool.close().await;
    tracing::info!("migrations applied");
    Ok(())
}

/// Rebuild one tenant's documents from issue-service.
pub async fn reindex(tenant_id: Uuid) -> anyhow::Result<()> {
    let index = open_index()?;
    ensure_index(&index).await?;
    let count = indexer(index)?.reindex(tenant_id).await.map_err(|e| anyhow::anyhow!("{e}"))?;
    println!("indexed {count} issues for tenant {tenant_id}");
    Ok(())
}

pub async fn run(telemetry: Telemetry) -> anyhow::Result<Telemetry> {
    let http_addr: SocketAddr = config::or("HTTP_ADDR", "127.0.0.1:8111").parse()?;
    let secret = config::required("INTERNAL_SERVICE_SECRET")?;
    let permission = config::or("PERMISSION_SERVICE_GRPC_URL", "http://127.0.0.1:9108");
    let user = config::or("USER_SERVICE_GRPC_URL", "http://127.0.0.1:9103");
    let registry = config::or("TENANT_SERVICE_GRPC_URL", "http://127.0.0.1:9102");
    let db = Arc::new(DbRouter::new(
        pools_from_env(&PoolSettings::from_env()?)?,
        Arc::new(RegistryPlacement::new(&registry, &secret, SERVICE_NAME)?),
    ));

    let index = open_index()?;
    ensure_index(&index).await?;
    let redis = RedisHandle::connect_in_background(&config::required("REDIS_URL")?)?;
    let policy = Arc::new(PolicyClient::new(Arc::new(GrpcPolicySource::new(&permission, &secret, SERVICE_NAME)?), redis.clone()));

    match KafkaSettings::from_env(SERVICE_NAME) {
        Some(settings) => {
            let handler = Arc::new(indexer(index.clone())?);
            tokio::spawn(async move {
                if let Err(error) = run_consumer(&settings, indexer::CONSUMER, &[topics::ISSUE, topics::PROJECT], handler).await {
                    tracing::error!(%error, "search indexer stopped");
                }
            });
        }
        None => tracing::warn!("KAFKA_BROKERS is not set: the index will not follow changes"),
    }

    let people: Arc<dyn people::People> = Arc::new(people::GrpcPeople::new(&user, &secret)?);
    let state = AppState {
        index: index.clone(),
        db: Arc::clone(&db),
        verifier: Arc::new(
            TokenVerifier::remote(config::required("TOKEN_ISSUER")?, config::required("JWKS_URL")?)?.with_revocations(redis),
        ),
        policy,
        people,
        allowed_origins: Arc::new(config::list("ALLOWED_ORIGINS")),
    };
    // Subscribed filters run on schedule; their digests leave through the outbox.
    subscriptions::spawn(state.clone(), db.pools());
    event_bus::spawn_relay(SERVICE_NAME, db.pools())?;
    let health = server::health_router(move || {
        let (index, db) = (index.clone(), Arc::clone(&db));
        async move {
            let pg = match db.global() {
                Ok(pool) => sqlx::query("SELECT 1").execute(pool).await.is_ok(),
                Err(_) => false,
            };
            pg && index.ready().await
        }
    });
    let app = observability::apply_http_layers(http::router(state), &HttpLayerConfig::default())
        .merge(health)
        .merge(observability::metrics_router(telemetry.metrics.clone()));
    server::serve_http(http_addr, app).await?;
    Ok(telemetry)
}
