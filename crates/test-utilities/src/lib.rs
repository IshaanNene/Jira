//! Helpers for integration tests: throwaway databases and signed test tokens.

use std::sync::Arc;

use auth_middleware::{generate_ed25519, Claims, Jwk, TokenKind, TokenSigner, TokenVerifier, API_AUDIENCE};
use sqlx::{
    migrate::Migrator,
    postgres::{PgConnectOptions, PgPoolOptions},
    Executor, PgPool,
};
use uuid::Uuid;

const APP_ROLE: &str = "platform_test_app";
const APP_PASSWORD: &str = "platform_test_app";
/// Runs migrations and owns the tables, like `<service>_owner` in every environment: not a
/// superuser, so FORCE ROW LEVEL SECURITY applies to it exactly as it does in production.
const OWNER_ROLE: &str = "platform_test_owner";
const OWNER_PASSWORD: &str = "platform_test_owner";
pub const TEST_ISSUER: &str = "http://identity.test";

/// A fresh database with the service's migrations applied.
///
/// `app` connects as an unprivileged role that does not own the tables and cannot bypass
/// row-level security — the same posture services run with — so RLS is really exercised.
/// Migrations run as a non-superuser table owner, as they do in deployments.
/// `owner` is the superuser connection, for arranging fixtures across tenants.
pub struct TestDb {
    pub app: PgPool,
    pub owner: PgPool,
    migrations: PgPool,
    name: String,
    admin: PgConnectOptions,
}

fn admin_options() -> anyhow::Result<PgConnectOptions> {
    let url = std::env::var("TEST_DATABASE_ADMIN_URL")
        .unwrap_or_else(|_| "postgres://postgres:postgres@127.0.0.1:54320/postgres".to_owned());
    Ok(url.parse()?)
}

impl TestDb {
    pub async fn create(migrator: &Migrator) -> anyhow::Result<Self> {
        let db = Self::empty().await?;
        db.migrate(migrator).await?;
        Ok(db)
    }

    /// A fresh database with only the migrations up to and including `version` applied.
    /// For testing what a later migration does to existing data.
    pub async fn create_until(migrator: &Migrator, version: i64) -> anyhow::Result<Self> {
        let partial = Migrator {
            migrations: migrator.migrations.iter().filter(|m| m.version <= version).cloned().collect::<Vec<_>>().into(),
            ..Migrator::DEFAULT
        };
        Self::create(&partial).await
    }

    /// Apply (the rest of) `migrator` as the table owner.
    pub async fn migrate(&self, migrator: &Migrator) -> anyhow::Result<()> {
        migrator.run(&self.migrations).await?;
        self.migrations
            .execute(
                format!(
                    "GRANT USAGE ON SCHEMA public TO {APP_ROLE}; \
                     GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO {APP_ROLE};"
                )
                .as_str(),
            )
            .await?;
        Ok(())
    }

    async fn empty() -> anyhow::Result<Self> {
        let admin = admin_options()?;
        let name = format!("test_{}", Uuid::new_v4().simple());
        let admin_pool = PgPoolOptions::new().max_connections(1).connect_with(admin.clone()).await?;
        // Role creation races between parallel tests; an advisory lock serialises it.
        let mut conn = admin_pool.acquire().await?;
        conn.execute("SELECT pg_advisory_lock(727274)").await?;
        for (role, password) in [(APP_ROLE, APP_PASSWORD), (OWNER_ROLE, OWNER_PASSWORD)] {
            conn.execute(
                format!(
                    "DO $$ BEGIN IF NOT EXISTS (SELECT FROM pg_roles WHERE rolname = '{role}') THEN \
                     CREATE ROLE {role} LOGIN PASSWORD '{password}' NOSUPERUSER NOBYPASSRLS; END IF; END $$"
                )
                .as_str(),
            )
            .await?;
        }
        conn.execute("SELECT pg_advisory_unlock(727274)").await?;
        conn.execute(format!("CREATE DATABASE {name} OWNER {OWNER_ROLE}").as_str()).await?;
        drop(conn);
        admin_pool.close().await;

        let owner = PgPoolOptions::new().max_connections(2).connect_with(admin.clone().database(&name)).await?;
        owner.execute("CREATE EXTENSION IF NOT EXISTS citext").await?;
        owner.execute(format!("ALTER SCHEMA public OWNER TO {OWNER_ROLE}").as_str()).await?;
        let migrations = PgPoolOptions::new()
            .max_connections(1)
            .connect_with(admin.clone().database(&name).username(OWNER_ROLE).password(OWNER_PASSWORD))
            .await?;
        let app = PgPoolOptions::new()
            .max_connections(5)
            .connect_with(admin.clone().database(&name).username(APP_ROLE).password(APP_PASSWORD))
            .await?;
        Ok(Self { app, owner, migrations, name, admin })
    }

    /// Drop the database. Call at the end of a test; a skipped call only leaks a test database.
    pub async fn cleanup(self) {
        self.app.close().await;
        self.migrations.close().await;
        self.owner.close().await;
        if let Ok(pool) = PgPoolOptions::new().max_connections(1).connect_with(self.admin).await {
            let _ = pool.execute(format!("DROP DATABASE IF EXISTS {} WITH (FORCE)", self.name).as_str()).await;
            pool.close().await;
        }
    }
}

/// A signing key plus a verifier that trusts it, for tests that call authenticated endpoints.
pub struct TestAuth {
    signer: TokenSigner,
    pub verifier: Arc<TokenVerifier>,
}

impl TestAuth {
    pub fn new() -> anyhow::Result<Self> {
        let key = generate_ed25519()?;
        let verifier = TokenVerifier::local(TEST_ISSUER);
        verifier.add_key(&Jwk::ed25519("test-key", key.public_x.clone()))?;
        Ok(Self { signer: TokenSigner::new("test-key", &key.pkcs8), verifier: Arc::new(verifier) })
    }

    fn base(user_id: Uuid, email: &str) -> Claims {
        let now = time::OffsetDateTime::now_utc().unix_timestamp();
        Claims {
            iss: TEST_ISSUER.to_owned(),
            aud: API_AUDIENCE.to_owned(),
            sub: user_id,
            sid: Uuid::now_v7(),
            jti: Uuid::now_v7(),
            iat: now,
            exp: now + 600,
            typ: TokenKind::Account,
            email: email.to_owned(),
            name: email.split('@').next().unwrap_or("user").to_owned(),
            tid: None,
            org: None,
            roles: Vec::new(),
            pv: 0,
        }
    }

    pub fn account_token(&self, user_id: Uuid, email: &str) -> anyhow::Result<String> {
        self.signer.sign(&Self::base(user_id, email))
    }

    pub fn org_token(&self, user_id: Uuid, email: &str, tenant_id: Uuid, org_id: Uuid, role: &str) -> anyhow::Result<String> {
        let mut claims = Self::base(user_id, email);
        claims.typ = TokenKind::Org;
        claims.tid = Some(tenant_id);
        claims.org = Some(org_id);
        claims.roles = vec![role.to_owned()];
        self.signer.sign(&claims)
    }

    pub fn sign(&self, claims: &Claims) -> anyhow::Result<String> {
        self.signer.sign(claims)
    }

    pub fn claims(user_id: Uuid, email: &str) -> Claims {
        Self::base(user_id, email)
    }
}
