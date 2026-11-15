//! Verifying an email address and resetting a forgotten password.

use auth_middleware::AuthContext;
use axum::{extract::State, http::StatusCode, Json};
use error_handling::{ApiError, ApiResult, FieldError};
use serde::Deserialize;
use utoipa::ToSchema;
use validator::Validate;

use super::auth::{normalize_email, publish_revocation, require_session};
use crate::{
    account_email::{self, Purpose},
    crypto, store, AppState,
};

const INVALID_LINK: &str = "This link is invalid, was already used or has expired. Ask for a new one.";
const MAIL_OFF: ApiError = ApiError::DependencyUnavailable("Email is not set up on this server.");

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct EmailTokenRequest {
    /// From the link in the email.
    pub token: String,
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ForgotPasswordRequest {
    #[validate(length(min = 1, max = 254, message = "Email is required."))]
    pub email: String,
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ResetPasswordRequest {
    /// From the link in the email.
    pub token: String,
    #[validate(length(min = 10, max = 128, message = "Password must be between 10 and 128 characters."))]
    pub password: String,
}

fn invalid_link() -> ApiError {
    ApiError::Validation(vec![FieldError::new("token", "invalid", INVALID_LINK)])
}

/// Send (or send again) the link that verifies the caller's address.
#[utoipa::path(post, path = "/api/v1/auth/email/verification", security(("bearer" = [])),
    responses((status = 202, description = "Sent"), (status = 204, description = "Already verified"),
        (status = 429, description = "Sent too recently"), (status = 503, description = "Email is not set up")))]
pub async fn send_verification(State(state): State<AppState>, auth: AuthContext) -> ApiResult<StatusCode> {
    require_session(&auth)?;
    let account = store::account_by_id(&state.pool, auth.user_id).await?.ok_or(ApiError::NotFound("account"))?;
    if account.email_verified_at.is_some() {
        return Ok(StatusCode::NO_CONTENT);
    }
    let mail = state.mail.as_ref().ok_or(MAIL_OFF)?;
    match account_email::issue(&state.pool, &account, Purpose::VerifyEmail).await? {
        Ok(token) => {
            account_email::send(mail, &account, Purpose::VerifyEmail, token);
            Ok(StatusCode::ACCEPTED)
        }
        Err(wait) => Err(ApiError::RateLimited { retry_after_secs: wait.whole_seconds().max(1) as u64 }),
    }
}

/// Mark the address of the link's account verified. Works signed out; the person's next access
/// token carries `email_verified`.
#[utoipa::path(post, path = "/api/v1/auth/email/verify", request_body = EmailTokenRequest,
    responses((status = 204), (status = 422, description = "Invalid, used or expired link")))]
pub async fn verify(State(state): State<AppState>, Json(body): Json<EmailTokenRequest>) -> ApiResult<StatusCode> {
    let mut tx = state.pool.begin().await?;
    let account_id = account_email::consume(&mut tx, body.token.trim(), Purpose::VerifyEmail).await?.ok_or_else(invalid_link)?;
    sqlx::query("UPDATE accounts SET email_verified_at = now(), updated_at = now() WHERE id = $1 AND email_verified_at IS NULL")
        .bind(account_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    tracing::info!(user_id = %account_id, "email address verified");
    Ok(StatusCode::NO_CONTENT)
}

/// Email a password reset link if the address has an active account. The answer is the same
/// either way, and nothing about the account is looked up before it is sent.
#[utoipa::path(post, path = "/api/v1/auth/password/forgot", request_body = ForgotPasswordRequest,
    responses((status = 202, description = "If the address has an account, a link is on its way"), (status = 503)))]
pub async fn forgot(State(state): State<AppState>, Json(mut body): Json<ForgotPasswordRequest>) -> ApiResult<StatusCode> {
    body.email = normalize_email(&body.email);
    body.validate()?;
    let mail = state.mail.clone().ok_or(MAIL_OFF)?;
    let pool = state.pool.clone();
    tokio::spawn(async move {
        let account = match store::account_by_email(&pool, &body.email).await {
            Ok(Some(account)) if account.status == "active" => account,
            Ok(_) => return,
            Err(error) => return tracing::warn!(%error, "password reset lookup failed"),
        };
        match account_email::issue(&pool, &account, Purpose::ResetPassword).await {
            Ok(Ok(token)) => account_email::send(&mail, &account, Purpose::ResetPassword, token),
            Ok(Err(_)) => tracing::info!(user_id = %account.id, "password reset link not sent: one was sent recently"),
            Err(error) => tracing::warn!(user_id = %account.id, %error, "password reset link not issued"),
        }
    });
    Ok(StatusCode::ACCEPTED)
}

/// Set a new password with a reset link. Every session ends, the lockout is cleared and the
/// address counts as verified (the link proved it). Two-step verification still applies at the
/// next sign-in; API tokens are separate credentials and keep working.
#[utoipa::path(post, path = "/api/v1/auth/password/reset", request_body = ResetPasswordRequest,
    responses((status = 204), (status = 422, description = "Invalid link or password")))]
pub async fn reset(State(state): State<AppState>, Json(body): Json<ResetPasswordRequest>) -> ApiResult<StatusCode> {
    body.validate()?;
    let password_hash = crypto::hash_password(body.password).await?;
    let mut tx = state.pool.begin().await?;
    let account_id =
        account_email::consume(&mut tx, body.token.trim(), Purpose::ResetPassword).await?.ok_or_else(invalid_link)?;
    sqlx::query(
        "UPDATE accounts SET password_hash = $2, failed_login_count = 0, locked_until = NULL, \
             email_verified_at = COALESCE(email_verified_at, now()), updated_at = now() \
         WHERE id = $1",
    )
    .bind(account_id)
    .bind(&password_hash)
    .execute(&mut *tx)
    .await?;
    let sessions: Vec<(uuid::Uuid,)> =
        sqlx::query_as("SELECT id FROM sessions WHERE account_id = $1 AND revoked_at IS NULL AND expires_at > now()")
            .bind(account_id)
            .fetch_all(&mut *tx)
            .await?;
    for (session_id,) in &sessions {
        super::auth::revoke_everywhere(&state, &mut tx, *session_id, account_id, "password_reset").await?;
    }
    tx.commit().await?;
    // Again after commit, in case a token was minted while the transaction ran.
    for (session_id,) in &sessions {
        publish_revocation(&state, *session_id).await;
    }
    tracing::info!(user_id = %account_id, sessions = sessions.len(), "password reset");
    Ok(StatusCode::NO_CONTENT)
}
