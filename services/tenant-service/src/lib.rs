//! Tenant registry service: the authoritative map from organization to tenant, tier,
//! region and database placement. Internal gRPC only; the HTTP port serves health and metrics.

pub mod domain;
pub mod grpc;
pub mod store;

use std::net::SocketAddr;

use api_contracts::tenant::v1::tenant_registry_server::TenantRegistryServer;
use auth_middleware::service_auth::ServiceServerAuth;
use axum::Router;
use platform_common::{config, server};
use sqlx::{migrate::Migrator, PgPool};
use tenant_context::{connect_pool, PoolSettings, DEFAULT_DESTINATION};

pub const SERVICE_NAME: &str = "tenant-service";
pub static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

pub struct Config {
    pub http_addr: SocketAddr,
    pub grpc_addr: SocketAddr,
    pub database_url: String,
    pub internal_secret: String,
    pub default_region: String,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        Ok(Self {
            http_addr: config::or("HTTP_ADDR", "127.0.0.1:8102").parse()?,
            grpc_addr: config::or("GRPC_ADDR", "127.0.0.1:9102").parse()?,
            database_url: config::required("DATABASE_URL")?,
            internal_secret: config::required("INTERNAL_SERVICE_SECRET")?,
            default_region: config::or("DEFAULT_REGION", "local-1"),
        })
    }
}

pub async fn migrate() -> anyhow::Result<()> {
    let url = config::required("DATABASE_MIGRATION_URL")?;
    let pool = PgPool::connect(&url).await?;
    MIGRATOR.run(&pool).await?;
    pool.close().await;
    tracing::info!("migrations applied");
    Ok(())
}

pub fn http_router(pool: PgPool, metrics: observability::Telemetry) -> (Router, observability::Telemetry) {
    let health = server::health_router(move || {
        let pool = pool.clone();
        async move { sqlx::query("SELECT 1").execute(&pool).await.is_ok() }
    });
    (health.merge(observability::metrics_router(metrics.metrics.clone())), metrics)
}

pub async fn run(config: Config, telemetry: observability::Telemetry) -> anyhow::Result<observability::Telemetry> {
    let pool = connect_pool(&config.database_url, &PoolSettings::from_env()?)?;
    // Best effort at boot: if the database is not up yet readiness stays red and the
    // next restart (or the first successful probe cycle) creates it.
    if let Err(err) = store::ensure_default_cluster(&pool, &config.default_region, DEFAULT_DESTINATION).await {
        tracing::warn!(error = %err, "could not ensure default cluster at startup");
    }

    let (http, telemetry) = http_router(pool.clone(), telemetry);
    let registry = grpc::RegistryService { pool, default_region: config.default_region.clone() };
    let auth = ServiceServerAuth::new(&config.internal_secret, SERVICE_NAME)?;

    tracing::info!(addr = %config.grpc_addr, "grpc server listening");
    let grpc = tonic::transport::Server::builder()
        .timeout(std::time::Duration::from_secs(5))
        .add_service(TenantRegistryServer::with_interceptor(registry, auth))
        .serve_with_shutdown(config.grpc_addr, server::shutdown_signal());

    tokio::try_join!(server::serve_http(config.http_addr, http), async { grpc.await.map_err(anyhow::Error::from) })?;
    Ok(telemetry)
}
