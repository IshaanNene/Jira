//! Identity service: accounts, sessions, access tokens and (from Phase 6) federation.

pub mod crypto;
pub mod http;
pub mod keys;
pub mod membership;
pub mod mfa;
pub mod store;

use std::{net::SocketAddr, sync::Arc};

use auth_middleware::{JwkSet, RedisHandle, TokenSigner, TokenVerifier};
use axum::{extract::FromRef, Router};
use observability::{HttpLayerConfig, Telemetry};
use platform_common::{config, server};
use sqlx::{migrate::Migrator, PgPool};
use tenant_context::{connect_pool, PoolSettings};

use crate::membership::{GrpcMembership, MembershipChecker};

pub const SERVICE_NAME: &str = "identity-service";
pub static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

#[derive(Debug, Clone)]
pub struct Settings {
    pub issuer: String,
    pub allowed_origins: Vec<String>,
    pub cookie_secure: bool,
    pub session_ttl_days: i32,
    pub trust_forwarded_for: bool,
}

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub settings: Arc<Settings>,
    pub signer: Arc<TokenSigner>,
    pub verifier: Arc<TokenVerifier>,
    pub jwks: Arc<JwkSet>,
    pub membership: Arc<dyn MembershipChecker>,
    pub redis: RedisHandle,
    /// A valid hash of a random password, verified against when the email is unknown.
    pub dummy_hash: Arc<String>,
    /// Seals and opens authenticator secrets.
    pub sealer: Arc<crypto::KeySealer>,
}

impl FromRef<AppState> for Arc<TokenVerifier> {
    fn from_ref(state: &AppState) -> Self {
        Arc::clone(&state.verifier)
    }
}

impl AppState {
    /// Assemble state from already-built parts (used by `run` and by tests).
    pub async fn build(
        pool: PgPool,
        settings: Settings,
        sealer: crypto::KeySealer,
        membership: Arc<dyn MembershipChecker>,
        redis: RedisHandle,
    ) -> anyhow::Result<Self> {
        let material = keys::load_or_create(&pool, &sealer).await?;
        let verifier = TokenVerifier::local(settings.issuer.clone()).with_revocations(redis.clone());
        for jwk in &material.jwks.keys {
            verifier.add_key(jwk)?;
        }
        let dummy_hash = crypto::hash_password(crypto::random_token()).await.map_err(|e| anyhow::anyhow!("{e}"))?;
        Ok(Self {
            pool,
            settings: Arc::new(settings),
            signer: Arc::new(material.signer),
            verifier: Arc::new(verifier),
            jwks: Arc::new(material.jwks),
            membership,
            redis,
            dummy_hash: Arc::new(dummy_hash),
            sealer: Arc::new(sealer),
        })
    }
}

pub struct Config {
    pub http_addr: SocketAddr,
    pub database_url: String,
    pub redis_url: String,
    pub user_service_grpc_url: String,
    pub internal_secret: String,
    pub key_encryption_key: String,
    pub settings: Settings,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        Ok(Self {
            http_addr: config::or("HTTP_ADDR", "127.0.0.1:8101").parse()?,
            database_url: config::required("DATABASE_URL")?,
            redis_url: config::required("REDIS_URL")?,
            user_service_grpc_url: config::or("USER_SERVICE_GRPC_URL", "http://127.0.0.1:9103"),
            internal_secret: config::required("INTERNAL_SERVICE_SECRET")?,
            key_encryption_key: config::required("IDENTITY_KEY_ENCRYPTION_KEY")?,
            settings: Settings {
                issuer: config::required("TOKEN_ISSUER")?,
                allowed_origins: config::list("ALLOWED_ORIGINS"),
                cookie_secure: config::parse_or("COOKIE_SECURE", true)?,
                session_ttl_days: config::parse_or("SESSION_TTL_DAYS", 30)?,
                trust_forwarded_for: config::parse_or("TRUST_FORWARDED_FOR", false)?,
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

/// The full HTTP application: API routes with the standard middleware, plus health and metrics.
pub fn app(state: AppState, telemetry: &Telemetry) -> Router {
    let pool = state.pool.clone();
    let health = server::health_router(move || {
        let pool = pool.clone();
        async move { sqlx::query("SELECT 1").execute(&pool).await.is_ok() }
    });
    observability::apply_http_layers(http::router(state), &HttpLayerConfig::default())
        .merge(health)
        .merge(observability::metrics_router(telemetry.metrics.clone()))
}

pub async fn run(config: Config, telemetry: Telemetry) -> anyhow::Result<Telemetry> {
    let pool = connect_pool(&config.database_url, &PoolSettings::from_env()?)?;
    event_bus::spawn_relay(SERVICE_NAME, vec![pool.clone()])?;
    let sealer = crypto::KeySealer::from_base64(&config.key_encryption_key)?;
    let membership = Arc::new(GrpcMembership::new(&config.user_service_grpc_url, &config.internal_secret)?);
    let redis = RedisHandle::connect_in_background(&config.redis_url)?;
    let state = AppState::build(pool, config.settings, sealer, membership, redis).await?;
    server::serve_http(config.http_addr, app(state, &telemetry)).await?;
    Ok(telemetry)
}
