//! Notification service: in-app notifications for issue activity (assignments, comments,
//! mentions, status changes), delivered live through realtime-service.

pub mod directory;
pub mod domain;
pub mod email;
pub mod http;
pub mod mailer;
pub mod notifier;
pub mod sender;
pub mod store;
pub mod unsubscribe;

use std::{net::SocketAddr, sync::Arc};

use auth_middleware::{GrpcPolicySource, PolicyClient, RedisHandle, TokenVerifier};
use axum::extract::FromRef;
use event_bus::{run_consumer, KafkaSettings};
use event_contracts::topics;
use observability::{HttpLayerConfig, Telemetry};
use platform_common::{config, server};
use sqlx::{migrate::Migrator, PgPool};
use tenant_context::{pools_from_env, DbRouter, PoolSettings, RegistryPlacement};

pub const SERVICE_NAME: &str = "notification-service";
pub static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

#[derive(Clone)]
pub struct AppState {
    pub db: Arc<DbRouter>,
    pub verifier: Arc<TokenVerifier>,
    pub policy: Arc<PolicyClient>,
    pub allowed_origins: Arc<Vec<String>>,
    pub unsubscribe: Arc<unsubscribe::UnsubscribeKey>,
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
    let http_addr: SocketAddr = config::or("HTTP_ADDR", "127.0.0.1:8110").parse()?;
    let secret = config::required("INTERNAL_SERVICE_SECRET")?;
    let registry = config::or("TENANT_SERVICE_GRPC_URL", "http://127.0.0.1:9102");
    let permission = config::or("PERMISSION_SERVICE_GRPC_URL", "http://127.0.0.1:9108");
    let issue = config::or("ISSUE_SERVICE_GRPC_URL", "http://127.0.0.1:9105");
    let user = config::or("USER_SERVICE_GRPC_URL", "http://127.0.0.1:9103");

    let db = Arc::new(DbRouter::new(
        pools_from_env(&PoolSettings::from_env()?)?,
        Arc::new(RegistryPlacement::new(&registry, &secret, SERVICE_NAME)?),
    ));
    event_bus::spawn_relay(SERVICE_NAME, db.pools())?;
    let redis = RedisHandle::connect_in_background(&config::required("REDIS_URL")?)?;
    let policy = Arc::new(PolicyClient::new(Arc::new(GrpcPolicySource::new(&permission, &secret, SERVICE_NAME)?), redis.clone()));

    let unsubscribe = Arc::new(unsubscribe::UnsubscribeKey::derive(&secret));
    // Email is optional: without SMTP_URL notifications are in-app only.
    let email = match config::optional("SMTP_URL") {
        Some(url) => {
            let mailer = mailer::SmtpMailer::new(&url, &config::or("EMAIL_FROM", "Meridian <notifications@meridian.localhost>"))?;
            sender::spawn(db.pools(), Arc::new(mailer));
            Some(notifier::EmailSettings {
                app_url: config::required("APP_URL")?.trim_end_matches('/').to_owned(),
                delay: std::time::Duration::from_secs(config::or("EMAIL_DELAY_SECONDS", "60").parse()?),
                unsubscribe: Arc::clone(&unsubscribe),
            })
        }
        None => {
            tracing::warn!("SMTP_URL is not set: notifications are not emailed");
            None
        }
    };

    match KafkaSettings::from_env(SERVICE_NAME) {
        Some(settings) => {
            let handler = Arc::new(notifier::Notifier {
                db: Arc::clone(&db),
                directory: Arc::new(directory::GrpcDirectory::new(&issue, &user, &secret)?),
                policy: Arc::clone(&policy),
                email,
            });
            tokio::spawn(async move {
                if let Err(error) = run_consumer(&settings, notifier::CONSUMER, &[topics::ISSUE, topics::SEARCH], handler).await {
                    tracing::error!(%error, "notification consumer stopped");
                }
            });
        }
        None => tracing::warn!("KAFKA_BROKERS is not set: no notifications will be created"),
    }

    let state = AppState {
        db: Arc::clone(&db),
        verifier: Arc::new(
            TokenVerifier::remote(config::required("TOKEN_ISSUER")?, config::required("JWKS_URL")?)?.with_revocations(redis),
        ),
        policy,
        allowed_origins: Arc::new(config::list("ALLOWED_ORIGINS")),
        unsubscribe,
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
