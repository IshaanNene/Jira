//! Two-step verification: turning it on and off, recovery codes, and the second step of
//! signing in.

use std::net::SocketAddr;

use auth_middleware::AuthContext;
use axum::{
    extract::{ConnectInfo, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use axum_extra::extract::cookie::CookieJar;
use error_handling::{ApiError, ApiResult};
use event_contracts::{outbox, payloads::LoginSucceededV1, Actor, EventEnvelope};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgConnection, PgPool};
use time::OffsetDateTime;
use utoipa::ToSchema;
use uuid::Uuid;

use super::{
    auth::{client_info, issue_access_token, refresh_cookie, revoke_everywhere, start_session, ClientInfo, SignInMethod},
    dto::TokenResponse,
};
use crate::{
    crypto, mfa,
    store::{self, Account},
    AppState,
};

/// How long the second step may take after a correct password.
pub const CHALLENGE_TTL_SECS: i64 = 300;
/// Wrong codes allowed per sign-in attempt (each also counts towards the account lockout).
const CHALLENGE_ATTEMPTS: i32 = 5;
/// Wrong codes allowed while setting up, before the secret must be generated again.
const SETUP_ATTEMPTS: i32 = 5;
const CHALLENGE_ENDED: ApiError = ApiError::Unauthenticated("This sign-in attempt has expired. Sign in again.");

#[derive(Debug, Serialize, ToSchema)]
pub struct MfaChallenge {
    /// Present it with a code to `POST /auth/login/mfa`. Single use.
    pub mfa_token: String,
    pub expires_in: i64,
    /// `totp` and `recovery_code`.
    pub methods: Vec<&'static str>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct MfaLoginRequest {
    pub mfa_token: String,
    /// A six-digit code from the authenticator app, or a recovery code.
    pub code: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct MfaStatus {
    pub totp_enabled: bool,
    #[serde(with = "time::serde::rfc3339::option")]
    pub enabled_at: Option<OffsetDateTime>,
    pub recovery_codes_remaining: i64,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PasswordConfirmation {
    pub password: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TotpSetup {
    /// Base32 secret, for typing into the app by hand.
    pub secret: String,
    pub otpauth_uri: String,
    /// The same link as a QR code (an SVG document).
    pub qr_svg: String,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CodeConfirmation {
    pub code: String,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PasswordAndCode {
    pub password: String,
    /// A code from the authenticator app, or a recovery code.
    pub code: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct RecoveryCodes {
    /// Shown once. Each works one time in place of an authenticator code.
    pub codes: Vec<String>,
}

#[derive(FromRow)]
struct Factor {
    secret_enc: Vec<u8>,
    nonce: Vec<u8>,
    confirmed_at: Option<OffsetDateTime>,
    last_used_step: i64,
    setup_attempts: i32,
}

fn secret_context(account_id: Uuid) -> Vec<u8> {
    [b"totp:".as_slice(), account_id.as_bytes()].concat()
}

fn recovery_hash(account_id: Uuid, normalized: &str) -> Vec<u8> {
    crypto::sha256(&[account_id.as_bytes().as_slice(), normalized.as_bytes()].concat())
}

fn wrong_code() -> ApiError {
    ApiError::invalid("code", "incorrect", "That code didn't work. Check the app and try again.")
}

fn now_step() -> i64 {
    mfa::step_at(OffsetDateTime::now_utc().unix_timestamp())
}

pub async fn totp_enabled(pool: &PgPool, account_id: Uuid) -> ApiResult<bool> {
    let (enabled,): (bool,) =
        sqlx::query_as("SELECT EXISTS (SELECT 1 FROM totp_factors WHERE account_id = $1 AND confirmed_at IS NOT NULL)")
            .bind(account_id)
            .fetch_one(pool)
            .await?;
    Ok(enabled)
}

/// Start the second step after a correct password.
pub async fn issue_challenge(pool: &PgPool, account_id: Uuid) -> ApiResult<MfaChallenge> {
    let token = crypto::random_token();
    sqlx::query(
        "INSERT INTO mfa_challenges (id, account_id, token_hash, expires_at) VALUES ($1, $2, $3, now() + make_interval(secs => $4))",
    )
    .bind(platform_common::new_id())
    .bind(account_id)
    .bind(crypto::sha256(token.as_bytes()))
    .bind(CHALLENGE_TTL_SECS as f64)
    .execute(pool)
    .await?;
    // Old challenges are useless; keep the table small.
    sqlx::query("DELETE FROM mfa_challenges WHERE expires_at < now() - interval '1 day'").execute(pool).await?;
    Ok(MfaChallenge { mfa_token: token, expires_in: CHALLENGE_TTL_SECS, methods: vec!["totp", "recovery_code"] })
}

/// Check an authenticator or recovery code for an account with two-step verification on.
/// Consumes what it matched (the time step, or the recovery code). Locks the factor row,
/// so two requests cannot both use the same code.
async fn check_second_factor(conn: &mut PgConnection, state: &AppState, account_id: Uuid, code: &str) -> ApiResult<bool> {
    let Some(factor) = sqlx::query_as::<_, Factor>(
        "SELECT secret_enc, nonce, confirmed_at, last_used_step, setup_attempts FROM totp_factors \
         WHERE account_id = $1 AND confirmed_at IS NOT NULL FOR UPDATE",
    )
    .bind(account_id)
    .fetch_optional(&mut *conn)
    .await?
    else {
        return Ok(false);
    };
    if mfa::as_totp(code).is_some() {
        let secret =
            state.sealer.open_for(&factor.secret_enc, &factor.nonce, &secret_context(account_id)).map_err(ApiError::internal)?;
        let Some(step) = mfa::verify(&secret, code, now_step(), factor.last_used_step) else {
            return Ok(false);
        };
        sqlx::query("UPDATE totp_factors SET last_used_step = $2 WHERE account_id = $1")
            .bind(account_id)
            .bind(step)
            .execute(&mut *conn)
            .await?;
        return Ok(true);
    }
    let Some(normalized) = mfa::normalize_recovery_code(code) else {
        return Ok(false);
    };
    let used =
        sqlx::query("UPDATE recovery_codes SET used_at = now() WHERE account_id = $1 AND code_hash = $2 AND used_at IS NULL")
            .bind(account_id)
            .bind(recovery_hash(account_id, &normalized))
            .execute(&mut *conn)
            .await?;
    if used.rows_affected() == 1 {
        tracing::info!(user_id = %account_id, "recovery code used");
    }
    Ok(used.rows_affected() == 1)
}

async fn replace_recovery_codes(conn: &mut PgConnection, account_id: Uuid) -> ApiResult<Vec<String>> {
    sqlx::query("DELETE FROM recovery_codes WHERE account_id = $1").bind(account_id).execute(&mut *conn).await?;
    let codes = mfa::new_recovery_codes();
    for code in &codes {
        let normalized =
            mfa::normalize_recovery_code(code).ok_or_else(|| ApiError::internal(anyhow::anyhow!("bad recovery code")))?;
        sqlx::query("INSERT INTO recovery_codes (id, account_id, code_hash) VALUES ($1, $2, $3)")
            .bind(platform_common::new_id())
            .bind(account_id)
            .bind(recovery_hash(account_id, &normalized))
            .execute(&mut *conn)
            .await?;
    }
    Ok(codes)
}

/// Ends every other session of the account: whoever else was signed in has to pass the
/// new rules (or lose the old ones) to come back.
async fn end_other_sessions(state: &AppState, conn: &mut PgConnection, auth: &AuthContext, reason: &str) -> ApiResult<()> {
    let others: Vec<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM sessions WHERE account_id = $1 AND id <> $2 AND revoked_at IS NULL AND expires_at > now()",
    )
    .bind(auth.user_id)
    .bind(auth.session_id)
    .fetch_all(&mut *conn)
    .await?;
    for (id,) in others {
        revoke_everywhere(state, conn, id, auth.user_id, reason).await?;
    }
    Ok(())
}

/// Re-check the password before a security change. Failures count towards the lockout,
/// so a stolen session cannot be used to guess the password.
pub(crate) async fn confirm_password(state: &AppState, account: &Account, password: String) -> ApiResult<()> {
    let incorrect = || ApiError::invalid("password", "incorrect", "That password isn't right.");
    if account.locked_until.is_some_and(|until| until > OffsetDateTime::now_utc()) {
        return Err(incorrect());
    }
    if !crypto::verify_password(password, account.password_hash.clone()).await? {
        store::record_failed_login(&state.pool, account.id).await?;
        return Err(incorrect());
    }
    Ok(())
}

pub(crate) async fn current_account(state: &AppState, auth: &AuthContext) -> ApiResult<Account> {
    super::auth::require_session(auth)?;
    if !store::session_is_active(&state.pool, auth.session_id, auth.user_id).await? {
        return Err(ApiError::Unauthenticated("Your session has ended. Sign in again."));
    }
    let account = store::account_by_id(&state.pool, auth.user_id).await?.ok_or(ApiError::NotFound("account"))?;
    if account.status != "active" {
        return Err(ApiError::Forbidden);
    }
    Ok(account)
}

#[utoipa::path(post, path = "/api/v1/auth/login/mfa", request_body = MfaLoginRequest,
    responses((status = 200, body = TokenResponse), (status = 401, description = "The sign-in attempt expired or ran out of tries"),
        (status = 422, description = "Wrong code")))]
pub async fn login(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(body): Json<MfaLoginRequest>,
) -> ApiResult<(CookieJar, Json<TokenResponse>)> {
    let client = client_info(&state, &headers, peer);
    let mut tx = state.pool.begin().await?;
    let found: Option<(Uuid, Uuid, i32)> = sqlx::query_as(
        "SELECT id, account_id, attempts FROM mfa_challenges \
         WHERE token_hash = $1 AND used_at IS NULL AND expires_at > now() FOR UPDATE",
    )
    .bind(crypto::sha256(body.mfa_token.as_bytes()))
    .fetch_optional(&mut *tx)
    .await?;
    let Some((challenge_id, account_id, attempts)) = found else {
        return Err(CHALLENGE_ENDED);
    };
    if attempts >= CHALLENGE_ATTEMPTS {
        return Err(CHALLENGE_ENDED);
    }
    let account = store::account_by_id(&state.pool, account_id).await?.ok_or(CHALLENGE_ENDED)?;
    if account.status != "active" {
        return Err(ApiError::Forbidden);
    }
    if account.locked_until.is_some_and(|until| until > OffsetDateTime::now_utc()) {
        return Err(CHALLENGE_ENDED);
    }
    let email_hash = crypto::sha256(account.email.to_lowercase().as_bytes());

    if !check_second_factor(&mut tx, &state, account.id, &body.code).await? {
        sqlx::query("UPDATE mfa_challenges SET attempts = attempts + 1 WHERE id = $1")
            .bind(challenge_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        store::record_failed_login(&state.pool, account.id).await?;
        log_event(&state.pool, account.id, &email_hash, "mfa_failed", &client).await;
        return Err(if attempts + 1 >= CHALLENGE_ATTEMPTS { CHALLENGE_ENDED } else { wrong_code() });
    }

    sqlx::query("UPDATE mfa_challenges SET used_at = now() WHERE id = $1").bind(challenge_id).execute(&mut *tx).await?;
    store::reset_failed_logins(&mut tx, account.id).await?;
    let (session_id, refresh) = start_session(&mut tx, &state, &account, &client, SignInMethod::TwoStep).await?;
    outbox::enqueue(
        &mut tx,
        &EventEnvelope::new(None, account.id, 1, Actor::User(account.id), LoginSucceededV1 { user_id: account.id, session_id }),
    )
    .await?;
    tx.commit().await?;
    log_event(&state.pool, account.id, &email_hash, "success", &client).await;

    let response = issue_access_token(&state, &account, session_id, None)?;
    Ok((jar.add(refresh_cookie(&state, refresh)), Json(response)))
}

pub(crate) async fn log_event(pool: &PgPool, account_id: Uuid, email_hash: &[u8], outcome: &str, client: &ClientInfo) {
    let (ip, agent) = (client.ip.as_deref(), client.user_agent.as_deref());
    if let Err(err) = store::insert_login_event(pool, Some(account_id), email_hash, outcome, ip, agent).await {
        tracing::warn!(error = %err, "could not record login event");
    }
}

#[utoipa::path(get, path = "/api/v1/auth/mfa", security(("bearer" = [])), responses((status = 200, body = MfaStatus)))]
pub async fn status(State(state): State<AppState>, auth: AuthContext) -> ApiResult<Json<MfaStatus>> {
    super::auth::require_session(&auth)?;
    let (enabled_at, remaining): (Option<OffsetDateTime>, i64) = sqlx::query_as(
        "SELECT (SELECT confirmed_at FROM totp_factors WHERE account_id = $1), \
                (SELECT count(*) FROM recovery_codes WHERE account_id = $1 AND used_at IS NULL)",
    )
    .bind(auth.user_id)
    .fetch_one(&state.pool)
    .await?;
    Ok(Json(MfaStatus { totp_enabled: enabled_at.is_some(), enabled_at, recovery_codes_remaining: remaining }))
}

#[utoipa::path(post, path = "/api/v1/auth/mfa/totp/setup", request_body = PasswordConfirmation, security(("bearer" = [])),
    responses((status = 200, body = TotpSetup), (status = 409, description = "Already on"), (status = 422, description = "Wrong password")))]
pub async fn setup(
    State(state): State<AppState>,
    auth: AuthContext,
    Json(body): Json<PasswordConfirmation>,
) -> ApiResult<Json<TotpSetup>> {
    let account = current_account(&state, &auth).await?;
    confirm_password(&state, &account, body.password).await?;
    if totp_enabled(&state.pool, account.id).await? {
        return Err(ApiError::Conflict("Two-step verification is already on. Turn it off first to use a new app.".into()));
    }
    let secret = mfa::new_secret();
    let (sealed, nonce) = state.sealer.seal_for(&secret, &secret_context(account.id)).map_err(ApiError::internal)?;
    // A new setup replaces an unfinished one.
    sqlx::query(
        "INSERT INTO totp_factors (account_id, secret_enc, nonce) VALUES ($1, $2, $3) \
         ON CONFLICT (account_id) DO UPDATE SET secret_enc = $2, nonce = $3, setup_attempts = 0, created_at = now() \
         WHERE totp_factors.confirmed_at IS NULL",
    )
    .bind(account.id)
    .bind(sealed)
    .bind(nonce)
    .execute(&state.pool)
    .await?;
    let encoded = mfa::base32(&secret);
    let uri = mfa::otpauth_uri(&account.email, &encoded);
    let qr_svg = mfa::qr_svg(&uri).map_err(ApiError::internal)?;
    Ok(Json(TotpSetup { secret: encoded, otpauth_uri: uri, qr_svg }))
}

#[utoipa::path(post, path = "/api/v1/auth/mfa/totp/confirm", request_body = CodeConfirmation, security(("bearer" = [])),
    responses((status = 200, body = RecoveryCodes), (status = 409, description = "No setup in progress"), (status = 422, description = "Wrong code")))]
pub async fn confirm(
    State(state): State<AppState>,
    auth: AuthContext,
    Json(body): Json<CodeConfirmation>,
) -> ApiResult<Json<RecoveryCodes>> {
    let account = current_account(&state, &auth).await?;
    let restart = || ApiError::Conflict("Set-up has expired. Start again to get a new code.".into());
    let mut tx = state.pool.begin().await?;
    let factor = sqlx::query_as::<_, Factor>(
        "SELECT secret_enc, nonce, confirmed_at, last_used_step, setup_attempts FROM totp_factors \
         WHERE account_id = $1 AND created_at > now() - interval '15 minutes' FOR UPDATE",
    )
    .bind(account.id)
    .fetch_optional(&mut *tx)
    .await?
    .filter(|f| f.confirmed_at.is_none() && f.setup_attempts < SETUP_ATTEMPTS)
    .ok_or_else(restart)?;

    let secret =
        state.sealer.open_for(&factor.secret_enc, &factor.nonce, &secret_context(account.id)).map_err(ApiError::internal)?;
    let Some(step) = mfa::verify(&secret, &body.code, now_step(), factor.last_used_step) else {
        sqlx::query("UPDATE totp_factors SET setup_attempts = setup_attempts + 1 WHERE account_id = $1")
            .bind(account.id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        return Err(if factor.setup_attempts + 1 >= SETUP_ATTEMPTS { restart() } else { wrong_code() });
    };
    sqlx::query("UPDATE totp_factors SET confirmed_at = now(), last_used_step = $2 WHERE account_id = $1")
        .bind(account.id)
        .bind(step)
        .execute(&mut *tx)
        .await?;
    let codes = replace_recovery_codes(&mut tx, account.id).await?;
    end_other_sessions(&state, &mut tx, &auth, "mfa_enabled").await?;
    tx.commit().await?;
    tracing::info!(user_id = %account.id, "two-step verification turned on");
    Ok(Json(RecoveryCodes { codes }))
}

#[utoipa::path(post, path = "/api/v1/auth/mfa/recovery-codes", request_body = PasswordAndCode, security(("bearer" = [])),
    responses((status = 200, body = RecoveryCodes), (status = 409, description = "Two-step verification is off"), (status = 422)))]
pub async fn regenerate(
    State(state): State<AppState>,
    auth: AuthContext,
    Json(body): Json<PasswordAndCode>,
) -> ApiResult<Json<RecoveryCodes>> {
    let account = current_account(&state, &auth).await?;
    confirm_password(&state, &account, body.password).await?;
    let mut tx = state.pool.begin().await?;
    if !totp_enabled(&state.pool, account.id).await? {
        return Err(ApiError::Conflict("Two-step verification is off.".into()));
    }
    if !check_second_factor(&mut tx, &state, account.id, &body.code).await? {
        store::record_failed_login(&state.pool, account.id).await?;
        return Err(wrong_code());
    }
    let codes = replace_recovery_codes(&mut tx, account.id).await?;
    tx.commit().await?;
    Ok(Json(RecoveryCodes { codes }))
}

#[utoipa::path(post, path = "/api/v1/auth/mfa/totp/disable", request_body = PasswordAndCode, security(("bearer" = [])),
    responses((status = 204), (status = 409, description = "Already off"), (status = 422)))]
pub async fn disable(
    State(state): State<AppState>,
    auth: AuthContext,
    Json(body): Json<PasswordAndCode>,
) -> ApiResult<StatusCode> {
    let account = current_account(&state, &auth).await?;
    confirm_password(&state, &account, body.password).await?;
    let mut tx = state.pool.begin().await?;
    if !totp_enabled(&state.pool, account.id).await? {
        return Err(ApiError::Conflict("Two-step verification is already off.".into()));
    }
    if !check_second_factor(&mut tx, &state, account.id, &body.code).await? {
        store::record_failed_login(&state.pool, account.id).await?;
        return Err(wrong_code());
    }
    sqlx::query("DELETE FROM totp_factors WHERE account_id = $1").bind(account.id).execute(&mut *tx).await?;
    sqlx::query("DELETE FROM recovery_codes WHERE account_id = $1").bind(account.id).execute(&mut *tx).await?;
    end_other_sessions(&state, &mut tx, &auth, "mfa_disabled").await?;
    tx.commit().await?;
    tracing::info!(user_id = %account.id, "two-step verification turned off");
    Ok(StatusCode::NO_CONTENT)
}
