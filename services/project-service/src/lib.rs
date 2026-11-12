//! Project service: projects, components, versions and project access.

pub mod access;
pub mod domain;
pub mod grpc;
pub mod http;
pub mod store;

use std::{net::SocketAddr, sync::Arc};

use api_contracts::project::v1::projects_server::ProjectsServer;
use auth_middleware::{service_auth::ServiceServerAuth, GrpcPolicySource, PolicyClient, RedisHandle, TokenVerifier};
use axum::extract::FromRef;
use observability::{HttpLayerConfig, Telemetry};
use platform_common::{config, server};
use sqlx::{migrate::Migrator, PgPool};
use tenant_context::{pools_from_env, DbRouter, PoolSettings, RegistryPlacement};

use crate::access::{GrpcAccess, ProjectAccess};

pub const SERVICE_NAME: &str = "project-service";
pub static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

#[derive(Clone)]
pub struct AppState {
    pub db: Arc<DbRouter>,
    pub verifier: Arc<TokenVerifier>,
    pub policy: Arc<PolicyClient>,
    pub access: Arc<dyn ProjectAccess>,
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
    let http_addr: SocketAddr = config::or("HTTP_ADDR", "127.0.0.1:8104").parse()?;
    let grpc_addr: SocketAddr = config::or("GRPC_ADDR", "127.0.0.1:9104").parse()?;
    let secret = config::required("INTERNAL_SERVICE_SECRET")?;
    let registry = config::or("TENANT_SERVICE_GRPC_URL", "http://127.0.0.1:9102");
    let permission = config::or("PERMISSION_SERVICE_GRPC_URL", "http://127.0.0.1:9108");
    let user = config::or("USER_SERVICE_GRPC_URL", "http://127.0.0.1:9103");

    let db = Arc::new(DbRouter::new(
        pools_from_env(&PoolSettings::from_env()?)?,
        Arc::new(RegistryPlacement::new(&registry, &secret, SERVICE_NAME)?),
    ));
    let redis = RedisHandle::connect_in_background(&config::required("REDIS_URL")?)?;
    let state = AppState {
        db: Arc::clone(&db),
        verifier: Arc::new(
            TokenVerifier::remote(config::required("TOKEN_ISSUER")?, config::required("JWKS_URL")?)?
                .with_revocations(redis.clone()),
        ),
        policy: Arc::new(PolicyClient::new(Arc::new(GrpcPolicySource::new(&permission, &secret, SERVICE_NAME)?), redis)),
        access: Arc::new(GrpcAccess::new(&permission, &user, &secret)?),
        allowed_origins: Arc::new(config::list("ALLOWED_ORIGINS")),
    };

    let health_db = Arc::clone(&db);
    let health = server::health_router(move || {
        let db = Arc::clone(&health_db);
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

    tracing::info!(addr = %grpc_addr, "grpc server listening");
    let grpc = tonic::transport::Server::builder()
        .timeout(std::time::Duration::from_secs(5))
        .add_service(ProjectsServer::with_interceptor(
            grpc::ProjectsService { db },
            ServiceServerAuth::new(&secret, SERVICE_NAME)?,
        ))
        .serve_with_shutdown(grpc_addr, server::shutdown_signal());

    tokio::try_join!(server::serve_http(http_addr, app), async { grpc.await.map_err(anyhow::Error::from) })?;
    Ok(telemetry)
}
