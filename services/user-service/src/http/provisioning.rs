//! Admin side of SCIM provisioning: the workspace's provisioning tokens, and who has been
//! provisioned. The protocol endpoints themselves are in `scim.rs`.

use auth_middleware::{OrgContext, OrgPermission};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use error_handling::{ApiError, ApiResult};
use event_contracts::{outbox, payloads::AuditRecordedV1, Actor, EventEnvelope};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use time::OffsetDateTime;
use utoipa::ToSchema;
use uuid::Uuid;

use super::scim::{token_hash, TOKEN_PREFIX};
use crate::AppState;

/// Most active provisioning tokens per workspace.
const MAX_TOKENS: i64 = 5;

#[derive(Debug, Serialize, ToSchema, FromRow)]
pub struct ProvisioningToken {
    pub id: Uuid,
    pub name: String,
    /// The start of the token, to tell tokens apart.
    pub prefix: String,
    pub created_by: Uuid,
    pub created_by_name: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_used_at: Option<OffsetDateTime>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ProvisioningTokenList {
    pub items: Vec<ProvisioningToken>,
    /// The SCIM base URL to give the identity provider.
    pub base_url: String,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateProvisioningToken {
    pub name: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct CreatedProvisioningToken {
    pub token: ProvisioningToken,
    /// Shown once; only a hash is kept.
    pub secret: String,
    pub base_url: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ProvisionedUser {
    pub id: Uuid,
    pub user_name: String,
    pub email: String,
    pub display_name: Option<String>,
    /// What the identity provider says.
    pub active: bool,
    /// Where they are in the workspace: `member`, `suspended`, `invited`, `invitation_expired`,
    /// `not_invited` (active, but their invitation was withdrawn in the app) or `deactivated`.
    pub state: String,
    pub user_id: Option<Uuid>,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ProvisionedUserList {
    pub items: Vec<ProvisionedUser>,
    pub total: i64,
}

const TOKEN_SELECT: &str = "SELECT t.id, t.name, t.prefix, t.created_by, p.display_name AS created_by_name, t.created_at, \
     t.last_used_at FROM scim_tokens t LEFT JOIN user_profiles p ON p.user_id = t.created_by";

fn base_url(state: &AppState) -> String {
    state.settings.scim_base_url.trim_end_matches('/').to_owned()
}

#[utoipa::path(get, path = "/api/v1/organizations/current/provisioning/tokens", security(("bearer" = [])),
    responses((status = 200, body = ProvisioningTokenList), (status = 403)))]
pub async fn list_tokens(State(state): State<AppState>, ctx: OrgContext) -> ApiResult<Json<ProvisioningTokenList>> {
    ctx.require(OrgPermission::ManageMembers)?;
    let mut tx = state.db.begin(ctx.tenant_id).await?;
    let items = sqlx::query_as::<_, ProvisioningToken>(&format!(
        "{TOKEN_SELECT} WHERE t.tenant_id = $1 AND t.revoked_at IS NULL ORDER BY t.created_at DESC"
    ))
    .bind(ctx.tenant_id)
    .fetch_all(tx.conn())
    .await?;
    tx.commit().await?;
    Ok(Json(ProvisioningTokenList { items, base_url: base_url(&state) }))
}

async fn audit(conn: &mut sqlx::PgConnection, ctx: &OrgContext, action: &str, id: Uuid, name: &str) -> ApiResult<()> {
    outbox::enqueue(
        conn,
        &EventEnvelope::new(
            Some(ctx.tenant_id),
            id,
            0,
            Actor::User(ctx.user_id),
            AuditRecordedV1 {
                action: action.into(),
                resource_type: "provisioning_token".into(),
                resource_id: id.to_string(),
                resource_label: name.to_owned(),
                details: Default::default(),
            },
        ),
    )
    .await?;
    Ok(())
}

#[utoipa::path(post, path = "/api/v1/organizations/current/provisioning/tokens", request_body = CreateProvisioningToken,
    security(("bearer" = [])),
    responses((status = 201, body = CreatedProvisioningToken), (status = 403), (status = 409, description = "Five tokens already"), (status = 422)))]
pub async fn create_token(
    State(state): State<AppState>,
    ctx: OrgContext,
    Json(body): Json<CreateProvisioningToken>,
) -> ApiResult<(StatusCode, Json<CreatedProvisioningToken>)> {
    ctx.require(OrgPermission::ManageMembers)?;
    let name = body.name.trim().to_owned();
    if name.is_empty() || name.chars().count() > 80 || name.chars().any(char::is_control) {
        return Err(ApiError::invalid("name", "invalid_name", "Give the token a name of up to 80 characters."));
    }
    let mut bytes = [0_u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    let secret = format!("{TOKEN_PREFIX}{}", URL_SAFE_NO_PAD.encode(bytes));
    let id = platform_common::new_id();

    let mut tx = state.db.begin(ctx.tenant_id).await?;
    // One at a time per workspace, so the limit holds.
    sqlx::query("SELECT 1 FROM organizations WHERE tenant_id = $1 FOR UPDATE").bind(ctx.tenant_id).execute(tx.conn()).await?;
    let (active,): (i64,) = sqlx::query_as("SELECT count(*) FROM scim_tokens WHERE tenant_id = $1 AND revoked_at IS NULL")
        .bind(ctx.tenant_id)
        .fetch_one(tx.conn())
        .await?;
    if active >= MAX_TOKENS {
        return Err(ApiError::Conflict(format!("A workspace can have {MAX_TOKENS} provisioning tokens. Revoke one first.")));
    }
    sqlx::query(
        "INSERT INTO scim_tokens (id, tenant_id, organization_id, name, prefix, created_by) VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(id)
    .bind(ctx.tenant_id)
    .bind(ctx.organization_id)
    .bind(&name)
    .bind(&secret[..TOKEN_PREFIX.len() + 4])
    .bind(ctx.user_id)
    .execute(tx.conn())
    .await?;
    sqlx::query("INSERT INTO scim_token_index (token_hash, tenant_id, token_id) VALUES ($1, $2, $3)")
        .bind(token_hash(&secret))
        .bind(ctx.tenant_id)
        .bind(id)
        .execute(tx.conn())
        .await?;
    audit(tx.conn(), &ctx, "provisioning.token_created", id, &name).await?;
    let token = sqlx::query_as::<_, ProvisioningToken>(&format!("{TOKEN_SELECT} WHERE t.tenant_id = $1 AND t.id = $2"))
        .bind(ctx.tenant_id)
        .bind(id)
        .fetch_one(tx.conn())
        .await?;
    tx.commit().await?;
    tracing::info!(token_id = %id, "provisioning token created");
    Ok((StatusCode::CREATED, Json(CreatedProvisioningToken { token, secret, base_url: base_url(&state) })))
}

#[utoipa::path(delete, path = "/api/v1/organizations/current/provisioning/tokens/{id}", security(("bearer" = [])),
    params(("id" = Uuid, Path)), responses((status = 204), (status = 403), (status = 404)))]
pub async fn revoke_token(State(state): State<AppState>, ctx: OrgContext, Path(id): Path<Uuid>) -> ApiResult<StatusCode> {
    ctx.require(OrgPermission::ManageMembers)?;
    let mut tx = state.db.begin(ctx.tenant_id).await?;
    let revoked: Option<(String,)> = sqlx::query_as(
        "UPDATE scim_tokens SET revoked_at = now() WHERE tenant_id = $1 AND id = $2 AND revoked_at IS NULL RETURNING name",
    )
    .bind(ctx.tenant_id)
    .bind(id)
    .fetch_optional(tx.conn())
    .await?;
    let Some((name,)) = revoked else { return Err(ApiError::NotFound("provisioning token")) };
    sqlx::query("DELETE FROM scim_token_index WHERE tenant_id = $1 AND token_id = $2")
        .bind(ctx.tenant_id)
        .bind(id)
        .execute(tx.conn())
        .await?;
    audit(tx.conn(), &ctx, "provisioning.token_revoked", id, &name).await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(FromRow)]
struct ProvisionedRow {
    id: Uuid,
    user_name: String,
    email: String,
    display_name: Option<String>,
    active: bool,
    user_id: Option<Uuid>,
    updated_at: OffsetDateTime,
    member_status: Option<String>,
    invitation_open: Option<bool>,
    invitation_expired: Option<bool>,
}

#[utoipa::path(get, path = "/api/v1/organizations/current/provisioning/users", security(("bearer" = [])),
    responses((status = 200, body = ProvisionedUserList), (status = 403)))]
pub async fn list_users(State(state): State<AppState>, ctx: OrgContext) -> ApiResult<Json<ProvisionedUserList>> {
    ctx.require(OrgPermission::ManageMembers)?;
    let mut tx = state.db.begin(ctx.tenant_id).await?;
    let (total,): (i64,) =
        sqlx::query_as("SELECT count(*) FROM scim_users WHERE tenant_id = $1").bind(ctx.tenant_id).fetch_one(tx.conn()).await?;
    let rows = sqlx::query_as::<_, ProvisionedRow>(
        "SELECT s.id, s.user_name::text AS user_name, s.email::text AS email, s.display_name, s.active, s.user_id, s.updated_at, \
                m.status AS member_status, \
                (i.id IS NOT NULL AND i.accepted_at IS NULL AND i.revoked_at IS NULL) AS invitation_open, \
                (i.expires_at <= now()) AS invitation_expired \
         FROM scim_users s \
         LEFT JOIN memberships m ON m.tenant_id = s.tenant_id AND m.user_id = s.user_id \
         LEFT JOIN invitations i ON i.tenant_id = s.tenant_id AND i.id = s.invitation_id \
         WHERE s.tenant_id = $1 ORDER BY s.updated_at DESC, s.id LIMIT 500",
    )
    .bind(ctx.tenant_id)
    .fetch_all(tx.conn())
    .await?;
    tx.commit().await?;
    let items = rows
        .into_iter()
        .map(|r| {
            let state = match (r.member_status.as_deref(), r.invitation_open, r.invitation_expired, r.active) {
                (Some("active"), ..) => "member",
                (Some(_), ..) => "suspended",
                (None, _, _, false) => "deactivated",
                (None, Some(true), Some(true), true) => "invitation_expired",
                (None, Some(true), _, true) => "invited",
                (None, _, _, true) => "not_invited",
            };
            ProvisionedUser {
                id: r.id,
                user_name: r.user_name,
                email: r.email,
                display_name: r.display_name,
                active: r.active,
                state: state.to_owned(),
                user_id: r.user_id,
                updated_at: r.updated_at,
            }
        })
        .collect();
    Ok(Json(ProvisionedUserList { items, total }))
}
