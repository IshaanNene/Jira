//! Notification service: in-app notifications for issue activity (assignments, comments,
//! mentions, status changes), delivered live through realtime-service.

pub mod directory;
pub mod domain;
pub mod email;
pub mod http;
pub mod http_source;
pub mod http_webhooks;
/// Re-exported so callers keep `notification_service::mailer::…`.
pub use smtp_mailer as mailer;
pub mod notifier;
pub mod projects;
pub mod sender;
pub mod source;
pub mod store;
pub mod suppression;
pub mod unsubscribe;
pub mod webhook_sender;
pub mod webhooks;

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
    pub webhooks: Arc<webhooks::Settings>,
    pub projects: Arc<dyn projects::Projects>,
    /// Checks the signature of bounce and complaint reports; `None`: not accepted.
    pub email_events: Option<Arc<ring::hmac::Key>>,
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
    let project = config::or("PROJECT_SERVICE_GRPC_URL", "http://127.0.0.1:9104");
    // Links in Slack messages; optional.
    let app_url = config::optional("APP_URL").map(|u| u.trim_end_matches('/').to_owned());

    let db = Arc::new(DbRouter::new(
        pools_from_env(&PoolSettings::from_env()?)?,
        Arc::new(RegistryPlacement::new(&registry, &secret, SERVICE_NAME)?),
    ));
    event_bus::spawn_relay(SERVICE_NAME, db.pools())?;
    let redis = RedisHandle::connect_in_background(&config::required("REDIS_URL")?)?;
    let policy = Arc::new(PolicyClient::new(Arc::new(GrpcPolicySource::new(&permission, &secret, SERVICE_NAME)?), redis.clone()));

    let unsubscribe = Arc::new(unsubscribe::UnsubscribeKey::derive(&secret));
    let webhook_settings =
        Arc::new(webhooks::Settings { secrets: webhooks::SecretBox::derive(&secret), reach: webhooks::Reach::from_env() });
    webhook_sender::spawn(db.pools(), Arc::clone(&webhook_settings));
    // Email is optional: without SMTP_URL notifications are in-app only.
    let email = match config::optional("SMTP_URL") {
        Some(url) => {
            let mailer = mailer::SmtpMailer::new(&url, &config::or("EMAIL_FROM", "Meridian <notifications@meridian.localhost>"))?;
            let app_url = config::required("APP_URL")?.trim_end_matches('/').to_owned();
            sender::spawn(
                db.pools(),
                Arc::new(mailer),
                sender::Settings { app_url: app_url.clone(), suppressions: db.global()?.clone() },
            );
            Some(notifier::EmailSettings {
                app_url,
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
            let dispatcher = Arc::new(webhooks::Dispatcher { db: Arc::clone(&db), app_url: app_url.clone() });
            let webhook_consumer = settings.clone();
            tokio::spawn(async move {
                if let Err(error) = run_consumer(&settings, notifier::CONSUMER, &[topics::ISSUE, topics::SEARCH], handler).await {
                    tracing::error!(%error, "notification consumer stopped");
                }
            });
            tokio::spawn(async move {
                if let Err(error) = run_consumer(&webhook_consumer, webhooks::CONSUMER, &[topics::ISSUE], dispatcher).await {
                    tracing::error!(%error, "webhook consumer stopped");
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
        webhooks: webhook_settings,
        projects: Arc::new(projects::GrpcProjects::new(&project, &secret)?),
        // Bounce and complaint reports from the mail provider; optional.
        email_events: config::optional("EMAIL_EVENTS_SECRET").map(|s| Arc::new(suppression::events_key(&s))),
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
