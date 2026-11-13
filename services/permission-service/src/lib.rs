//! Permission service: owns project role bindings and the per-tenant policy version.
//! Internal gRPC only; other services evaluate permissions locally from its snapshots.

pub mod grpc;
pub mod store;

use std::{net::SocketAddr, sync::Arc};

use api_contracts::permission::v1::authorizer_server::AuthorizerServer;
use auth_middleware::{service_auth::ServiceServerAuth, RedisHandle};
use observability::Telemetry;
use platform_common::{config, server};
use sqlx::{migrate::Migrator, PgPool};
use tenant_context::{pools_from_env, DbRouter, PoolSettings, RegistryPlacement};

pub const SERVICE_NAME: &str = "permission-service";
pub static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

pub async fn migrate() -> anyhow::Result<()> {
    let pool = PgPool::connect(&config::required("DATABASE_MIGRATION_URL")?).await?;
    MIGRATOR.run(&pool).await?;
    pool.close().await;
    tracing::info!("migrations applied");
    Ok(())
}

pub async fn run(telemetry: Telemetry) -> anyhow::Result<Telemetry> {
    let http_addr: SocketAddr = config::or("HTTP_ADDR", "127.0.0.1:8108").parse()?;
    let grpc_addr: SocketAddr = config::or("GRPC_ADDR", "127.0.0.1:9108").parse()?;
    let secret = config::required("INTERNAL_SERVICE_SECRET")?;
    let registry = config::or("TENANT_SERVICE_GRPC_URL", "http://127.0.0.1:9102");

    let pools = pools_from_env(&PoolSettings::from_env()?)?;
    let db = Arc::new(DbRouter::new(pools, Arc::new(RegistryPlacement::new(&registry, &secret, SERVICE_NAME)?)));
    event_bus::spawn_relay(SERVICE_NAME, db.pools())?;
    let redis = RedisHandle::connect_in_background(&config::required("REDIS_URL")?)?;

    let health_db = Arc::clone(&db);
    let http = server::health_router(move || {
        let db = Arc::clone(&health_db);
        async move {
            match db.global() {
                Ok(pool) => sqlx::query("SELECT 1").execute(pool).await.is_ok(),
                Err(_) => false,
            }
        }
    })
    .merge(observability::metrics_router(telemetry.metrics.clone()));

    tracing::info!(addr = %grpc_addr, "grpc server listening");
    let grpc = tonic::transport::Server::builder()
        .timeout(std::time::Duration::from_secs(5))
        .add_service(AuthorizerServer::with_interceptor(
            grpc::AuthorizerService { db, redis },
            ServiceServerAuth::new(&secret, SERVICE_NAME)?,
        ))
        .serve_with_shutdown(grpc_addr, server::shutdown_signal());

    tokio::try_join!(server::serve_http(http_addr, http), async { grpc.await.map_err(anyhow::Error::from) })?;
    Ok(telemetry)
}
