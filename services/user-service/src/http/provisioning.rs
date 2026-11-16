//! Admin side of SCIM provisioning: the workspace's provisioning tokens, who has been
//! provisioned, and the roles given to the provider's groups. The protocol endpoints
//! themselves are in `scim.rs`.

use auth_middleware::{OrgContext, OrgPermission, OrgRole};
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

use super::scim::{sync_roles, token_hash, Acting, TOKEN_PREFIX};
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
    /// The role their groups give them, when any of their groups has one.
    pub group_role: Option<String>,
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
    group_role: Option<String>,
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
        "SELECT s.id, s.user_name::text AS user_name, s.email::text AS email, s.display_name, s.active, s.user_id, s.group_role, s.updated_at, \
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
                group_role: r.group_role,
                updated_at: r.updated_at,
            }
        })
        .collect();
    Ok(Json(ProvisionedUserList { items, total }))
}

/// A group the identity provider pushed, and the role a workspace admin gave it.
#[derive(Debug, Serialize, ToSchema, FromRow)]
pub struct ProvisionedGroup {
    pub id: Uuid,
    pub display_name: String,
    pub members: i64,
    /// `admin`, `member`, `guest`, or none: membership of the group then changes nobody's role.
    pub org_role: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ProvisionedGroupList {
    pub items: Vec<ProvisionedGroup>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SetGroupRole {
    /// `admin`, `member`, `guest`, or null to stop the group deciding roles.
    pub org_role: Option<String>,
}

const GROUP_SELECT: &str = "SELECT g.id, g.display_name::text AS display_name, \
     (SELECT count(*) FROM scim_group_members gm WHERE gm.group_id = g.id) AS members, g.org_role, g.updated_at \
     FROM scim_groups g";

#[utoipa::path(get, path = "/api/v1/organizations/current/provisioning/groups", security(("bearer" = [])),
    responses((status = 200, body = ProvisionedGroupList), (status = 403)))]
pub async fn list_groups(State(state): State<AppState>, ctx: OrgContext) -> ApiResult<Json<ProvisionedGroupList>> {
    ctx.require(OrgPermission::ManageMembers)?;
    let mut tx = state.db.begin(ctx.tenant_id).await?;
    let items = sqlx::query_as::<_, ProvisionedGroup>(&format!(
        "{GROUP_SELECT} WHERE g.tenant_id = $1 ORDER BY g.display_name LIMIT 1000"
    ))
    .bind(ctx.tenant_id)
    .fetch_all(tx.conn())
    .await?;
    tx.commit().await?;
    Ok(Json(ProvisionedGroupList { items }))
}

/// Give a group a role (or take it away). Its members' roles change at once, under the app's
/// rules for the admin doing it; owners and the admin themselves are left alone.
#[utoipa::path(put, path = "/api/v1/organizations/current/provisioning/groups/{id}/role", request_body = SetGroupRole,
    security(("bearer" = [])), params(("id" = Uuid, Path)),
    responses((status = 200, body = ProvisionedGroup), (status = 403), (status = 404), (status = 422)))]
pub async fn set_group_role(
    State(state): State<AppState>,
    ctx: OrgContext,
    Path(id): Path<Uuid>,
    Json(body): Json<SetGroupRole>,
) -> ApiResult<Json<ProvisionedGroup>> {
    ctx.require(OrgPermission::ManageMembers)?;
    let role = match body.org_role.as_deref() {
        None => None,
        Some(r) => match OrgRole::parse(r) {
            Some(role @ (OrgRole::Admin | OrgRole::Member | OrgRole::Guest)) => Some(role),
            _ => return Err(ApiError::invalid("org_role", "unknown_role", "A group's role is admin, member or guest.")),
        },
    };
    // Giving a group a role is giving it to everyone in it: no stronger than your own.
    if role.is_some_and(|r| r > ctx.role) {
        return Err(ApiError::Forbidden);
    }
    let mut tx = state.db.begin(ctx.tenant_id).await?;
    // Serialised with provisioning requests, which lock the organization row too.
    sqlx::query("SELECT 1 FROM organizations WHERE tenant_id = $1 FOR UPDATE").bind(ctx.tenant_id).execute(tx.conn()).await?;
    let updated: Option<(String, Option<String>)> = sqlx::query_as(
        "UPDATE scim_groups g SET org_role = $3, updated_at = now() FROM scim_groups old \
         WHERE old.id = g.id AND g.tenant_id = $1 AND g.id = $2 RETURNING g.display_name::text, old.org_role",
    )
    .bind(ctx.tenant_id)
    .bind(id)
    .bind(role.map(OrgRole::as_str))
    .fetch_optional(tx.conn())
    .await?;
    let Some((name, previous)) = updated else { return Err(ApiError::NotFound("group")) };
    if previous.as_deref() != role.map(OrgRole::as_str) {
        let members: Vec<(Uuid,)> =
            sqlx::query_as("SELECT scim_user_id FROM scim_group_members WHERE tenant_id = $1 AND group_id = $2")
                .bind(ctx.tenant_id)
                .bind(id)
                .fetch_all(tx.conn())
                .await?;
        let members: Vec<Uuid> = members.into_iter().map(|(m,)| m).collect();
        let acting =
            Acting { tenant_id: ctx.tenant_id, actor_id: ctx.user_id, actor_role: ctx.role, actor: Actor::User(ctx.user_id) };
        sync_roles(tx.conn(), &acting, &members).await?;
        outbox::enqueue(
            tx.conn(),
            &EventEnvelope::new(
                Some(ctx.tenant_id),
                id,
                0,
                Actor::User(ctx.user_id),
                AuditRecordedV1 {
                    action: "provisioning.group_role_set".into(),
                    resource_type: "provisioning_group".into(),
                    resource_id: id.to_string(),
                    resource_label: name,
                    details: [("org_role".to_owned(), role.map_or("none", OrgRole::as_str).to_owned())].into(),
                },
            ),
        )
        .await?;
    }
    let group = sqlx::query_as::<_, ProvisionedGroup>(&format!("{GROUP_SELECT} WHERE g.tenant_id = $1 AND g.id = $2"))
        .bind(ctx.tenant_id)
        .bind(id)
        .fetch_one(tx.conn())
        .await?;
    tx.commit().await?;
    Ok(Json(group))
}
