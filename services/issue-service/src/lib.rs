//! Issue service: issue lifecycle, keys, comments, links, history, watchers.

pub mod attachments;
pub mod automation;
pub mod custom_fields;
pub mod directory;
pub mod domain;
pub mod dto;
pub mod grpc;
pub mod http;
pub mod http_automation;
pub mod http_fields;
pub mod richtext;
pub mod store;
pub mod workflows;

use std::{net::SocketAddr, sync::Arc};

use api_contracts::issue::v1::issues_server::IssuesServer;
use auth_middleware::{service_auth::ServiceServerAuth, GrpcPolicySource, PolicyClient, RedisHandle, TokenVerifier};
use axum::extract::FromRef;
use observability::{HttpLayerConfig, Telemetry};
use platform_common::{config, server};
use sqlx::{migrate::Migrator, PgPool};
use tenant_context::{pools_from_env, DbRouter, PoolSettings, RegistryPlacement};

use crate::directory::{Directory, GrpcDirectory};

pub const SERVICE_NAME: &str = "issue-service";
pub static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

#[derive(Clone)]
pub struct AppState {
    pub db: Arc<DbRouter>,
    pub verifier: Arc<TokenVerifier>,
    pub policy: Arc<PolicyClient>,
    pub directory: Arc<dyn Directory>,
    pub workflows: Arc<dyn workflows::Workflows>,
    /// Attachment bytes. Any S3-compatible store.
    pub blobs: Arc<dyn object_store::ObjectStore>,
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
    let http_addr: SocketAddr = config::or("HTTP_ADDR", "127.0.0.1:8105").parse()?;
    let grpc_addr: SocketAddr = config::or("GRPC_ADDR", "127.0.0.1:9105").parse()?;
    let secret = config::required("INTERNAL_SERVICE_SECRET")?;
    let registry = config::or("TENANT_SERVICE_GRPC_URL", "http://127.0.0.1:9102");
    let permission = config::or("PERMISSION_SERVICE_GRPC_URL", "http://127.0.0.1:9108");
    let project = config::or("PROJECT_SERVICE_GRPC_URL", "http://127.0.0.1:9104");
    let user = config::or("USER_SERVICE_GRPC_URL", "http://127.0.0.1:9103");

    let db = Arc::new(DbRouter::new(
        pools_from_env(&PoolSettings::from_env()?)?,
        Arc::new(RegistryPlacement::new(&registry, &secret, SERVICE_NAME)?),
    ));
    event_bus::spawn_relay(SERVICE_NAME, db.pools())?;
    let redis = RedisHandle::connect_in_background(&config::required("REDIS_URL")?)?;
    let state = AppState {
        db: Arc::clone(&db),
        verifier: Arc::new(
            TokenVerifier::remote(config::required("TOKEN_ISSUER")?, config::required("JWKS_URL")?)?
                .with_revocations(redis.clone()),
        ),
        policy: Arc::new(PolicyClient::new(Arc::new(GrpcPolicySource::new(&permission, &secret, SERVICE_NAME)?), redis)),
        directory: Arc::new(GrpcDirectory::new(&project, &user, &secret)?),
        workflows: Arc::new(workflows::GrpcWorkflows::new(
            &config::or("WORKFLOW_SERVICE_GRPC_URL", "http://127.0.0.1:9106"),
            &secret,
        )?),
        blobs: attachments::store_from_env()?,
        allowed_origins: Arc::new(config::list("ALLOWED_ORIGINS")),
    };

    automation::spawn_scheduler(state.clone(), db.pools());
    let (directory, workflows) = (Arc::clone(&state.directory), Arc::clone(&state.workflows));

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
        .add_service(IssuesServer::with_interceptor(
            grpc::IssuesService { db, directory, workflows },
            ServiceServerAuth::new(&secret, SERVICE_NAME)?,
        ))
        .serve_with_shutdown(grpc_addr, server::shutdown_signal());

    tokio::try_join!(server::serve_http(http_addr, app), async { grpc.await.map_err(anyhow::Error::from) })?;
    Ok(telemetry)
}
