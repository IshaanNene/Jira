//! User & organization service: profiles, organizations, memberships and invitations.

pub mod audit;
pub mod domain;
pub mod grpc;
pub mod http;
pub mod invite_mail;
pub mod profile_sync;
pub mod scim;
pub mod store;
pub mod tenants;

use std::{net::SocketAddr, sync::Arc};

use api_contracts::user::v1::membership_server::MembershipServer;
use auth_middleware::{service_auth::ServiceServerAuth, RedisHandle, TokenVerifier};
use axum::{extract::FromRef, Router};
use observability::{HttpLayerConfig, Telemetry};
use platform_common::{config, server};
use sqlx::{migrate::Migrator, PgPool};
use tenant_context::{pools_from_env, DbRouter, PoolSettings, RegistryPlacement};

use crate::tenants::{GrpcProvisioner, TenantProvisioner};

pub const SERVICE_NAME: &str = "user-service";
pub static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

#[derive(Debug, Clone)]
pub struct Settings {
    pub allowed_origins: Vec<String>,
    /// Public base URL of the web app; used to build invitation links.
    pub web_base_url: String,
    /// Public SCIM base URL given to identity providers (`SCIM_BASE_URL`; default
    /// `{WEB_BASE_URL}/scim/v2`, for when the web host also serves the API).
    pub scim_base_url: String,
}

#[derive(Clone)]
pub struct AppState {
    pub db: Arc<DbRouter>,
    pub verifier: Arc<TokenVerifier>,
    pub tenants: Arc<dyn TenantProvisioner>,
    pub settings: Arc<Settings>,
    /// Invitation emails; `None` without a mail server (links are then only shown to the inviter).
    pub mail: Option<Arc<invite_mail::InviteMail>>,
}

impl FromRef<AppState> for Arc<TokenVerifier> {
    fn from_ref(state: &AppState) -> Self {
        Arc::clone(&state.verifier)
    }
}

pub struct Config {
    pub http_addr: SocketAddr,
    pub grpc_addr: SocketAddr,
    pub redis_url: String,
    pub tenant_service_grpc_url: String,
    pub internal_secret: String,
    pub token_issuer: String,
    pub jwks_url: String,
    /// `(SMTP_URL, EMAIL_FROM)` when email is set up.
    pub smtp: Option<(String, String)>,
    pub settings: Settings,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        Ok(Self {
            http_addr: config::or("HTTP_ADDR", "127.0.0.1:8103").parse()?,
            grpc_addr: config::or("GRPC_ADDR", "127.0.0.1:9103").parse()?,
            redis_url: config::required("REDIS_URL")?,
            tenant_service_grpc_url: config::or("TENANT_SERVICE_GRPC_URL", "http://127.0.0.1:9102"),
            internal_secret: config::required("INTERNAL_SERVICE_SECRET")?,
            token_issuer: config::required("TOKEN_ISSUER")?,
            jwks_url: config::required("JWKS_URL")?,
            smtp: config::optional("SMTP_URL")
                .map(|url| (url, config::or("EMAIL_FROM", "Meridian <invitations@meridian.localhost>"))),
            settings: {
                let web_base_url = config::or("WEB_BASE_URL", "http://localhost:5173");
                Settings {
                    allowed_origins: config::list("ALLOWED_ORIGINS"),
                    scim_base_url: config::or("SCIM_BASE_URL", &format!("{}/scim/v2", web_base_url.trim_end_matches('/'))),
                    web_base_url,
                }
            },
        })
    }
}

pub async fn migrate() -> anyhow::Result<()> {
    let pool = PgPool::connect(&config::required("DATABASE_MIGRATION_URL")?).await?;
    MIGRATOR.run(&pool).await?;
    pool.close().await;
    tracing::info!("migrations applied");
    Ok(())
}

pub fn app(state: AppState, telemetry: &Telemetry) -> Router {
    let db = Arc::clone(&state.db);
    let health = server::health_router(move || {
        let db = Arc::clone(&db);
        async move {
            match db.global() {
                Ok(pool) => sqlx::query("SELECT 1").execute(pool).await.is_ok(),
                Err(_) => false,
            }
        }
    });
    observability::apply_http_layers(http::router(state), &HttpLayerConfig::default())
        .merge(health)
        .merge(observability::metrics_router(telemetry.metrics.clone()))
}

pub async fn run(config: Config, telemetry: Telemetry) -> anyhow::Result<Telemetry> {
    let pools = pools_from_env(&PoolSettings::from_env()?)?;
    let placement = RegistryPlacement::new(&config.tenant_service_grpc_url, &config.internal_secret, SERVICE_NAME)?;
    let db = Arc::new(DbRouter::new(pools, Arc::new(placement)));
    event_bus::spawn_relay(SERVICE_NAME, db.pools())?;
    match event_bus::KafkaSettings::from_env(SERVICE_NAME) {
        Some(settings) => {
            use event_contracts::topics;
            let handler = Arc::new(audit::Recorder { db: Arc::clone(&db) });
            let topics = [topics::ORGANIZATION, topics::PROJECT, topics::WORKFLOW, topics::AGILE, topics::ISSUE, topics::AUDIT];
            let profiles = Arc::new(profile_sync::ProfileSync { db: Arc::clone(&db) });
            let identity_settings = settings.clone();
            tokio::spawn(async move {
                if let Err(error) = event_bus::run_consumer(&settings, audit::CONSUMER, &topics, handler).await {
                    tracing::error!(%error, "audit recorder stopped");
                }
            });
            tokio::spawn(async move {
                let topics = [topics::IDENTITY];
                if let Err(error) = event_bus::run_consumer(&identity_settings, profile_sync::CONSUMER, &topics, profiles).await {
                    tracing::error!(%error, "profile sync stopped");
                }
            });
        }
        None => tracing::warn!(
            "KAFKA_BROKERS is not set: the audit log will not be recorded and email changes will not reach profiles"
        ),
    }
    let redis = RedisHandle::connect_in_background(&config.redis_url)?;
    let verifier = Arc::new(TokenVerifier::remote(config.token_issuer.clone(), config.jwks_url.clone())?.with_revocations(redis));
    let state = AppState {
        db: Arc::clone(&db),
        verifier,
        tenants: Arc::new(GrpcProvisioner::new(&config.tenant_service_grpc_url, &config.internal_secret)?),
        settings: Arc::new(config.settings),
        mail: match config.smtp {
            Some((url, from)) => {
                Some(Arc::new(invite_mail::InviteMail { mailer: Arc::new(smtp_mailer::SmtpMailer::new(&url, &from)?) }))
            }
            None => {
                tracing::warn!("SMTP_URL is not set: invitations are not emailed");
                None
            }
        },
    };

    let auth = ServiceServerAuth::new(&config.internal_secret, SERVICE_NAME)?;
    tracing::info!(addr = %config.grpc_addr, "grpc server listening");
    let grpc = tonic::transport::Server::builder()
        .timeout(std::time::Duration::from_secs(5))
        .add_service(MembershipServer::with_interceptor(grpc::MembershipService { db }, auth))
        .serve_with_shutdown(config.grpc_addr, server::shutdown_signal());

    tokio::try_join!(server::serve_http(config.http_addr, app(state, &telemetry)), async {
        grpc.await.map_err(anyhow::Error::from)
    })?;
    Ok(telemetry)
}
