//! SQL for accounts, sessions and refresh tokens. All statements use bind parameters.

use error_handling::ApiResult;
use sqlx::{FromRow, PgConnection, PgPool};
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Debug, Clone, FromRow)]
pub struct Account {
    pub id: Uuid,
    pub email: String,
    pub display_name: String,
    pub password_hash: String,
    pub email_verified_at: Option<OffsetDateTime>,
    pub status: String,
    pub locked_until: Option<OffsetDateTime>,
    pub created_at: OffsetDateTime,
}

const ACCOUNT_COLUMNS: &str =
    "id, email::text AS email, display_name, password_hash, email_verified_at, status, locked_until, created_at";

pub async fn insert_account(
    conn: &mut PgConnection,
    id: Uuid,
    email: &str,
    display_name: &str,
    password_hash: &str,
) -> Result<Account, sqlx::Error> {
    sqlx::query_as::<_, Account>(&format!(
        "INSERT INTO accounts (id, email, display_name, password_hash) VALUES ($1, $2, $3, $4) RETURNING {ACCOUNT_COLUMNS}"
    ))
    .bind(id)
    .bind(email)
    .bind(display_name)
    .bind(password_hash)
    .fetch_one(conn)
    .await
}

pub async fn account_by_email(pool: &PgPool, email: &str) -> ApiResult<Option<Account>> {
    Ok(sqlx::query_as::<_, Account>(&format!("SELECT {ACCOUNT_COLUMNS} FROM accounts WHERE email = $1"))
        .bind(email)
        .fetch_optional(pool)
        .await?)
}

pub async fn account_by_id(pool: &PgPool, id: Uuid) -> ApiResult<Option<Account>> {
    Ok(sqlx::query_as::<_, Account>(&format!("SELECT {ACCOUNT_COLUMNS} FROM accounts WHERE id = $1"))
        .bind(id)
        .fetch_optional(pool)
        .await?)
}

/// Count a failed attempt. From the 5th consecutive failure the account locks for
/// 1, 2, 4, 8 then 15 minutes (capped).
pub async fn record_failed_login(pool: &PgPool, account_id: Uuid) -> ApiResult<()> {
    sqlx::query(
        "UPDATE accounts SET failed_login_count = failed_login_count + 1, \
             locked_until = CASE WHEN failed_login_count + 1 >= 5 \
                 THEN now() + make_interval(mins => LEAST(15, power(2, LEAST(failed_login_count + 1 - 5, 10))::int)) \
                 ELSE locked_until END, \
             updated_at = now() \
         WHERE id = $1",
    )
    .bind(account_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn reset_failed_logins(conn: &mut PgConnection, account_id: Uuid) -> ApiResult<()> {
    sqlx::query("UPDATE accounts SET failed_login_count = 0, locked_until = NULL WHERE id = $1 AND failed_login_count > 0")
        .bind(account_id)
        .execute(conn)
        .await?;
    Ok(())
}

pub async fn insert_login_event(
    pool: &PgPool,
    account_id: Option<Uuid>,
    email_hash: &[u8],
    outcome: &str,
    ip: Option<&str>,
    user_agent: Option<&str>,
) -> ApiResult<()> {
    sqlx::query("INSERT INTO login_events (id, account_id, email_hash, outcome, ip, user_agent) VALUES ($1, $2, $3, $4, $5, $6)")
        .bind(platform_common::new_id())
        .bind(account_id)
        .bind(email_hash)
        .bind(outcome)
        .bind(ip)
        .bind(user_agent)
        .execute(pool)
        .await?;
    Ok(())
}

#[derive(Debug, Clone, FromRow)]
pub struct Session {
    pub id: Uuid,
    pub account_id: Uuid,
    pub created_at: OffsetDateTime,
    pub last_seen_at: OffsetDateTime,
    pub expires_at: OffsetDateTime,
    pub revoked_at: Option<OffsetDateTime>,
    pub ip: Option<String>,
    pub user_agent: Option<String>,
    /// `password`, `two_step` or `passkey`.
    pub auth_method: String,
}

const SESSION_COLUMNS: &str = "id, account_id, created_at, last_seen_at, expires_at, revoked_at, ip, user_agent, auth_method";

pub async fn insert_session(
    conn: &mut PgConnection,
    account_id: Uuid,
    ttl_days: i32,
    ip: Option<&str>,
    user_agent: Option<&str>,
    auth_method: &str,
    sso_organization_id: Option<Uuid>,
) -> ApiResult<Session> {
    Ok(sqlx::query_as::<_, Session>(&format!(
        "INSERT INTO sessions (id, account_id, expires_at, ip, user_agent, auth_method, sso_organization_id) \
         VALUES ($1, $2, now() + make_interval(days => $3), $4, $5, $6, $7) RETURNING {SESSION_COLUMNS}"
    ))
    .bind(platform_common::new_id())
    .bind(account_id)
    .bind(ttl_days)
    .bind(ip)
    .bind(user_agent)
    .bind(auth_method)
    .bind(sso_organization_id)
    .fetch_one(conn)
    .await?)
}

pub async fn insert_refresh_token(conn: &mut PgConnection, session_id: Uuid, token_hash: &[u8]) -> ApiResult<()> {
    // A refresh token never outlives its session.
    sqlx::query(
        "INSERT INTO refresh_tokens (id, session_id, token_hash, expires_at) \
         SELECT $1, s.id, $3, s.expires_at FROM sessions s WHERE s.id = $2",
    )
    .bind(platform_common::new_id())
    .bind(session_id)
    .bind(token_hash)
    .execute(conn)
    .await?;
    Ok(())
}

#[derive(Debug, Clone, FromRow)]
pub struct RefreshLookup {
    pub token_id: Uuid,
    pub used_at: Option<OffsetDateTime>,
    pub token_expired: bool,
    pub session_id: Uuid,
    pub account_id: Uuid,
    pub session_revoked: bool,
    pub session_expired: bool,
}

/// Lock the refresh token row so concurrent presentations of the same token are serialised.
pub async fn lock_refresh_token(conn: &mut PgConnection, token_hash: &[u8]) -> ApiResult<Option<RefreshLookup>> {
    Ok(sqlx::query_as::<_, RefreshLookup>(
        "SELECT rt.id AS token_id, rt.used_at, rt.expires_at <= now() AS token_expired, \
                s.id AS session_id, s.account_id, s.revoked_at IS NOT NULL AS session_revoked, \
                s.expires_at <= now() AS session_expired \
         FROM refresh_tokens rt JOIN sessions s ON s.id = rt.session_id \
         WHERE rt.token_hash = $1 FOR UPDATE OF rt",
    )
    .bind(token_hash)
    .fetch_optional(conn)
    .await?)
}

pub async fn mark_refresh_token_used(conn: &mut PgConnection, token_id: Uuid) -> ApiResult<()> {
    sqlx::query("UPDATE refresh_tokens SET used_at = now() WHERE id = $1").bind(token_id).execute(&mut *conn).await?;
    Ok(())
}

pub async fn touch_session(conn: &mut PgConnection, session_id: Uuid) -> ApiResult<()> {
    sqlx::query("UPDATE sessions SET last_seen_at = now() WHERE id = $1").bind(session_id).execute(conn).await?;
    Ok(())
}

/// Returns `true` if this call revoked the session (false if it was already revoked or unknown).
pub async fn revoke_session(conn: &mut PgConnection, session_id: Uuid, account_id: Uuid, reason: &str) -> ApiResult<bool> {
    let result = sqlx::query(
        "UPDATE sessions SET revoked_at = now(), revoked_reason = $3 WHERE id = $1 AND account_id = $2 AND revoked_at IS NULL",
    )
    .bind(session_id)
    .bind(account_id)
    .bind(reason)
    .execute(conn)
    .await?;
    Ok(result.rows_affected() == 1)
}

pub async fn session_is_active(pool: &PgPool, session_id: Uuid, account_id: Uuid) -> ApiResult<bool> {
    let (active,): (bool,) = sqlx::query_as(
        "SELECT EXISTS (SELECT 1 FROM sessions WHERE id = $1 AND account_id = $2 AND revoked_at IS NULL AND expires_at > now())",
    )
    .bind(session_id)
    .bind(account_id)
    .fetch_one(pool)
    .await?;
    Ok(active)
}

pub async fn active_sessions(pool: &PgPool, account_id: Uuid) -> ApiResult<Vec<Session>> {
    Ok(sqlx::query_as::<_, Session>(&format!(
        "SELECT {SESSION_COLUMNS} FROM sessions \
         WHERE account_id = $1 AND revoked_at IS NULL AND expires_at > now() ORDER BY last_seen_at DESC LIMIT 100"
    ))
    .bind(account_id)
    .fetch_all(pool)
    .await?)
}
