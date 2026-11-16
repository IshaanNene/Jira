//! API tokens: long-lived secrets for scripts and integrations.
//!
//! A token belongs to one person in one organization and is created from a signed-in session
//! (never with another API token). It is not accepted by the services directly: a client
//! exchanges it at `POST /api/v1/auth/token/api` for an ordinary 10-minute access token, so
//! every service keeps verifying one kind of credential. The exchange reads the account and the
//! membership afresh, so a removed member, a suspended account or a changed role takes effect
//! at the next exchange. Revoking a token also revokes the access tokens issued for it.

use auth_middleware::{AuthContext, TokenKind};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use error_handling::{ApiError, ApiResult};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use time::{Duration, OffsetDateTime};
use utoipa::ToSchema;
use uuid::Uuid;

use super::{
    auth::{issue_token, publish_revocation, require_admitted, require_session},
    dto::TokenResponse,
};
use crate::{crypto, store, AppState};

/// Every secret starts with this, so leaked tokens are easy to recognise (and to scan for).
const PREFIX: &str = "mrd_";
/// Active tokens per person per organization.
const MAX_ACTIVE: i64 = 25;
const MAX_DAYS: u16 = 365;
const INVALID: ApiError = ApiError::Unauthenticated("This API token is not valid.");

#[derive(Debug, FromRow)]
struct Row {
    id: Uuid,
    name: String,
    hint: String,
    created_at: OffsetDateTime,
    expires_at: OffsetDateTime,
    last_used_at: Option<OffsetDateTime>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ApiTokenDto {
    pub id: Uuid,
    pub name: String,
    /// The first characters of the secret.
    pub hint: String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_used_at: Option<OffsetDateTime>,
}

impl From<Row> for ApiTokenDto {
    fn from(r: Row) -> Self {
        Self {
            id: r.id,
            name: r.name,
            hint: r.hint,
            created_at: r.created_at,
            expires_at: r.expires_at,
            last_used_at: r.last_used_at,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ApiTokenList {
    pub items: Vec<ApiTokenDto>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateApiToken {
    /// What the token is for, 1 to 100 characters.
    pub name: String,
    /// 1 to 365.
    pub expires_in_days: u16,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct CreatedApiToken {
    pub token: ApiTokenDto,
    /// Shown once; only its hash is kept.
    pub secret: String,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ApiTokenExchange {
    pub token: String,
}

/// The organization the caller is signed in to, from a session token.
fn organization(auth: &AuthContext) -> ApiResult<Uuid> {
    require_session(auth)?;
    match (auth.claims.typ, auth.claims.org) {
        (TokenKind::Org, Some(org)) => Ok(org),
        _ => Err(ApiError::Unauthenticated("Select an organization to continue.")),
    }
}

const COLUMNS: &str = "id, name, hint, created_at, expires_at, last_used_at";

#[utoipa::path(get, path = "/api/v1/auth/api-tokens", security(("bearer" = [])),
    responses((status = 200, body = ApiTokenList)))]
pub async fn list(State(state): State<AppState>, auth: AuthContext) -> ApiResult<Json<ApiTokenList>> {
    let org = organization(&auth)?;
    let rows: Vec<Row> = sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM api_tokens \
         WHERE account_id = $1 AND organization_id = $2 AND revoked_at IS NULL AND expires_at > now() \
         ORDER BY created_at DESC"
    ))
    .bind(auth.user_id)
    .bind(org)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(ApiTokenList { items: rows.into_iter().map(Into::into).collect() }))
}

#[utoipa::path(post, path = "/api/v1/auth/api-tokens", request_body = CreateApiToken, security(("bearer" = [])),
    responses((status = 201, body = CreatedApiToken), (status = 409, description = "Too many active tokens"),
              (status = 422, description = "Invalid name or lifetime")))]
pub async fn create(
    State(state): State<AppState>,
    auth: AuthContext,
    Json(body): Json<CreateApiToken>,
) -> ApiResult<(StatusCode, Json<CreatedApiToken>)> {
    let org = organization(&auth)?;
    let name = body.name.trim();
    if name.is_empty() || name.chars().count() > 100 {
        return Err(ApiError::invalid("name", "length", "Name the token (up to 100 characters)."));
    }
    if !(1..=MAX_DAYS).contains(&body.expires_in_days) {
        return Err(ApiError::invalid("expires_in_days", "range", &format!("A token lasts 1 to {MAX_DAYS} days.")));
    }
    if !store::session_is_active(&state.pool, auth.session_id, auth.user_id).await? {
        return Err(ApiError::Unauthenticated("Your session has ended. Sign in again."));
    }
    // Still a member now, not just when the access token was issued.
    let membership = state.membership.check(auth.user_id, org).await?.ok_or(ApiError::Forbidden)?;
    require_admitted(&state, auth.user_id, org, Some(auth.session_id), &membership).await?;

    let secret = format!("{PREFIX}{}", crypto::random_token());
    let hint: String = secret.chars().take(PREFIX.len() + 4).collect();
    let mut tx = state.pool.begin().await?;
    // Serialize creations per person so the limit cannot be raced past.
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext('api_tokens:' || $1::text))").bind(auth.user_id).execute(&mut *tx).await?;
    let (active,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM api_tokens \
         WHERE account_id = $1 AND organization_id = $2 AND revoked_at IS NULL AND expires_at > now()",
    )
    .bind(auth.user_id)
    .bind(org)
    .fetch_one(&mut *tx)
    .await?;
    if active >= MAX_ACTIVE {
        return Err(ApiError::Conflict(format!("You have {MAX_ACTIVE} active API tokens. Revoke one first.")));
    }
    let row: Row = sqlx::query_as(&format!(
        "INSERT INTO api_tokens (id, account_id, organization_id, name, token_hash, hint, expires_at) \
         VALUES ($1, $2, $3, $4, $5, $6, now() + $7 * interval '1 day') RETURNING {COLUMNS}"
    ))
    .bind(platform_common::new_id())
    .bind(auth.user_id)
    .bind(org)
    .bind(name)
    .bind(crypto::sha256(secret.as_bytes()))
    .bind(&hint)
    .bind(i32::from(body.expires_in_days))
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    tracing::info!(token_id = %row.id, "api token created");
    Ok((StatusCode::CREATED, Json(CreatedApiToken { token: row.into(), secret })))
}

#[utoipa::path(delete, path = "/api/v1/auth/api-tokens/{id}", security(("bearer" = [])),
    params(("id" = Uuid, Path, description = "API token id")),
    responses((status = 204), (status = 404, description = "No such token for this account")))]
pub async fn revoke(State(state): State<AppState>, auth: AuthContext, Path(id): Path<Uuid>) -> ApiResult<StatusCode> {
    require_session(&auth)?;
    // Scoped to the caller's own tokens: anyone else's is simply "not found".
    let revoked =
        sqlx::query("UPDATE api_tokens SET revoked_at = now() WHERE id = $1 AND account_id = $2 AND revoked_at IS NULL")
            .bind(id)
            .bind(auth.user_id)
            .execute(&state.pool)
            .await?
            .rows_affected();
    if revoked == 0 {
        return Err(ApiError::NotFound("API token"));
    }
    publish_revocation(&state, id).await;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(post, path = "/api/v1/auth/token/api", request_body = ApiTokenExchange,
    responses((status = 200, body = TokenResponse), (status = 401, description = "Unknown, expired or revoked token"),
              (status = 403, description = "No longer a member of the token's organization")))]
pub async fn exchange(State(state): State<AppState>, Json(body): Json<ApiTokenExchange>) -> ApiResult<Json<TokenResponse>> {
    let secret = body.token.trim();
    if !secret.starts_with(PREFIX) || secret.len() > 100 {
        return Err(INVALID);
    }
    let found: Option<(Uuid, Uuid, Uuid, OffsetDateTime)> = sqlx::query_as(
        "SELECT id, account_id, organization_id, expires_at FROM api_tokens \
         WHERE token_hash = $1 AND revoked_at IS NULL",
    )
    .bind(crypto::sha256(secret.as_bytes()))
    .fetch_optional(&state.pool)
    .await?;
    let (id, account_id, organization_id, expires_at) = found.ok_or(INVALID)?;
    if expires_at <= OffsetDateTime::now_utc() {
        return Err(INVALID);
    }
    let account = store::account_by_id(&state.pool, account_id).await?.ok_or(INVALID)?;
    if account.status != "active" {
        return Err(ApiError::Forbidden);
    }
    let membership = state.membership.check(account.id, organization_id).await?.ok_or(ApiError::Forbidden)?;
    // A token does not get around a requirement the workspace added after it was made.
    require_admitted(&state, account.id, organization_id, None, &membership).await?;
    // Coarse on purpose: one write per token per minute at most.
    sqlx::query(
        "UPDATE api_tokens SET last_used_at = now() \
         WHERE id = $1 AND (last_used_at IS NULL OR last_used_at < now() - $2 * interval '1 second')",
    )
    .bind(id)
    .bind(Duration::MINUTE.whole_seconds() as i32)
    .execute(&state.pool)
    .await?;
    Ok(Json(issue_token(&state, &account, id, Some((organization_id, &membership)), true)?))
}
