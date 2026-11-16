//! `/api/v1/auth/admin/…`: what a workspace's admins and owners may see and do about its
//! members' sign-in security. Everything is limited to the caller's own organization:
//! two-step status only for its active members, and only the API tokens issued for it.
//!
//! Deliberately absent: resetting someone's two-step verification. An account is global and
//! may belong to other workspaces, so one workspace's admin must not be able to weaken it.

use auth_middleware::{AuthContext, OrgContext, OrgPermission};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use error_handling::{ApiError, ApiResult};
use event_contracts::{outbox, payloads::AuditRecordedV1, Actor, EventEnvelope};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use utoipa::ToSchema;
use uuid::Uuid;

use super::auth::{publish_revocation, require_session};
use crate::AppState;

pub const MAX_LOOKUP: usize = 500;

/// A signed-in admin or owner (not an API token) acting in their organization.
fn admin(auth: &AuthContext, ctx: &OrgContext) -> ApiResult<()> {
    require_session(auth)?;
    ctx.require(OrgPermission::ManageOrganization)?;
    Ok(())
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct TwoStepLookup {
    /// At most 500.
    pub user_ids: Vec<Uuid>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct MemberTwoStep {
    pub user_id: Uuid,
    pub enabled: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct MemberTwoStepList {
    /// Only the ids that are active members of the caller's workspace.
    pub items: Vec<MemberTwoStep>,
}

/// Whether each member has turned on two-step verification. Ids that are not active members
/// of the caller's workspace are left out, so this reveals nothing about other accounts.
#[utoipa::path(post, path = "/api/v1/auth/admin/two-step", request_body = TwoStepLookup, security(("bearer" = [])),
    responses((status = 200, body = MemberTwoStepList), (status = 403), (status = 422)))]
pub async fn two_step(
    State(state): State<AppState>,
    auth: AuthContext,
    ctx: OrgContext,
    Json(body): Json<TwoStepLookup>,
) -> ApiResult<Json<MemberTwoStepList>> {
    admin(&auth, &ctx)?;
    if body.user_ids.len() > MAX_LOOKUP {
        return Err(ApiError::invalid("user_ids", "too_many", "Ask about at most 500 people at a time."));
    }
    let mut ids = body.user_ids;
    ids.sort();
    ids.dedup();
    let members = state.membership.active_members(ctx.tenant_id, &ids).await?;
    let enabled: Vec<Uuid> =
        sqlx::query_scalar("SELECT account_id FROM totp_factors WHERE account_id = ANY($1) AND confirmed_at IS NOT NULL")
            .bind(&members)
            .fetch_all(&state.pool)
            .await?;
    Ok(Json(MemberTwoStepList {
        items: members.into_iter().map(|user_id| MemberTwoStep { user_id, enabled: enabled.contains(&user_id) }).collect(),
    }))
}

#[derive(Debug, Serialize, ToSchema, sqlx::FromRow)]
pub struct WorkspaceApiToken {
    pub id: Uuid,
    /// Whose token it is.
    pub owner_id: Uuid,
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

#[derive(Debug, Serialize, ToSchema)]
pub struct WorkspaceApiTokenList {
    pub items: Vec<WorkspaceApiToken>,
}

/// Every active API token issued for this workspace, newest first.
#[utoipa::path(get, path = "/api/v1/auth/admin/api-tokens", security(("bearer" = [])),
    responses((status = 200, body = WorkspaceApiTokenList), (status = 403)))]
pub async fn api_tokens(
    State(state): State<AppState>,
    auth: AuthContext,
    ctx: OrgContext,
) -> ApiResult<Json<WorkspaceApiTokenList>> {
    admin(&auth, &ctx)?;
    let items: Vec<WorkspaceApiToken> = sqlx::query_as(
        "SELECT id, account_id AS owner_id, name, hint, created_at, expires_at, last_used_at FROM api_tokens \
         WHERE organization_id = $1 AND revoked_at IS NULL AND expires_at > now() ORDER BY created_at DESC LIMIT 1000",
    )
    .bind(ctx.organization_id)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(WorkspaceApiTokenList { items }))
}

/// Revoke any API token issued for this workspace; recorded in the audit log.
#[utoipa::path(delete, path = "/api/v1/auth/admin/api-tokens/{id}", security(("bearer" = [])), params(("id" = Uuid, Path)),
    responses((status = 204), (status = 403), (status = 404)))]
pub async fn revoke_api_token(
    State(state): State<AppState>,
    auth: AuthContext,
    ctx: OrgContext,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    admin(&auth, &ctx)?;
    let mut tx = state.pool.begin().await?;
    let revoked: Option<(Uuid, String)> = sqlx::query_as(
        "UPDATE api_tokens SET revoked_at = now() WHERE id = $1 AND organization_id = $2 AND revoked_at IS NULL \
         RETURNING account_id, name",
    )
    .bind(id)
    .bind(ctx.organization_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((owner, name)) = revoked else { return Err(ApiError::NotFound("API token")) };
    outbox::enqueue(
        &mut tx,
        &EventEnvelope::new(
            Some(ctx.tenant_id),
            id,
            0,
            Actor::User(ctx.user_id),
            AuditRecordedV1 {
                action: "api_token.revoked".into(),
                resource_type: "api_token".into(),
                resource_id: id.to_string(),
                resource_label: name,
                details: [("owner_id".to_owned(), owner.to_string())].into(),
            },
        ),
    )
    .await?;
    tx.commit().await?;
    publish_revocation(&state, id).await;
    Ok(StatusCode::NO_CONTENT)
}
