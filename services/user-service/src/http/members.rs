use auth_middleware::{OrgContext, OrgPermission, OrgRole};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use error_handling::{ApiError, ApiResult};
use platform_common::pagination::{Page, PageParams};
use time::OffsetDateTime;
use uuid::Uuid;

use super::{
    dto::{MemberDto, MemberPage, MemberQuery, UpdateMemberRequest},
    organizations::emit_membership_changed,
};
use crate::{
    domain::{check_member_change, MemberChange, MemberChangeContext, MemberStatus},
    store, AppState,
};

#[utoipa::path(get, path = "/api/v1/organizations/current/members", security(("bearer" = [])),
    params(("limit" = Option<u32>, Query), ("cursor" = Option<String>, Query), ("query" = Option<String>, Query)),
    responses((status = 200, body = MemberPage)))]
pub async fn list(State(state): State<AppState>, ctx: OrgContext, Query(q): Query<MemberQuery>) -> ApiResult<Json<MemberPage>> {
    ctx.require(OrgPermission::ViewMembers)?;
    let params = PageParams { limit: q.limit, cursor: q.cursor };
    let limit = params.limit();
    let after = params
        .decode_cursor::<(i128, Uuid)>()
        .map_err(|_| ApiError::invalid("cursor", "invalid_cursor", "The cursor is not valid."))?
        .map(|(nanos, id)| OffsetDateTime::from_unix_timestamp_nanos(nanos).map(|at| (at, id)))
        .transpose()
        .map_err(|_| ApiError::invalid("cursor", "invalid_cursor", "The cursor is not valid."))?;
    let search = q.query.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(|s| s.chars().take(100).collect::<String>());

    let mut tx = state.db.begin(ctx.tenant_id).await?;
    let rows = store::list_members(tx.conn(), ctx.tenant_id, search.as_deref(), after, limit).await?;
    tx.commit().await?;

    let page = Page::from_overfetch(rows, limit, |m| (m.joined_at.unix_timestamp_nanos(), m.user_id));
    Ok(Json(MemberPage { items: page.items.into_iter().map(MemberDto::from).collect(), next_cursor: page.next_cursor }))
}

/// Load everything the rules need, under the organization row lock so that two
/// concurrent demotions cannot both pass the "last owner" check.
async fn change_context(conn: &mut sqlx::PgConnection, ctx: &OrgContext, target: Uuid) -> ApiResult<MemberChangeContext> {
    sqlx::query("SELECT 1 FROM organizations WHERE tenant_id = $1 FOR UPDATE").bind(ctx.tenant_id).execute(&mut *conn).await?;
    // The actor's role is re-read from the database: the token may be minutes old.
    let actor =
        store::membership(conn, ctx.tenant_id, ctx.user_id).await?.filter(|m| m.status == "active").ok_or(ApiError::Forbidden)?;
    let target_row = store::membership(conn, ctx.tenant_id, target).await?.ok_or(ApiError::NotFound("member"))?;
    Ok(MemberChangeContext {
        actor_role: OrgRole::parse(&actor.org_role).ok_or(ApiError::Forbidden)?,
        actor_is_target: ctx.user_id == target,
        target_role: OrgRole::parse(&target_row.org_role)
            .ok_or_else(|| ApiError::internal(anyhow::anyhow!("unknown stored role")))?,
        target_status: MemberStatus::parse(&target_row.status)
            .ok_or_else(|| ApiError::internal(anyhow::anyhow!("unknown stored status")))?,
        active_owner_count: store::active_owner_count(conn, ctx.tenant_id).await?,
    })
}

#[utoipa::path(patch, path = "/api/v1/organizations/current/members/{user_id}", request_body = UpdateMemberRequest,
    security(("bearer" = [])), params(("user_id" = Uuid, Path)),
    responses((status = 200, body = MemberDto), (status = 403), (status = 404), (status = 409)))]
pub async fn update(
    State(state): State<AppState>,
    ctx: OrgContext,
    Path(user_id): Path<Uuid>,
    Json(body): Json<UpdateMemberRequest>,
) -> ApiResult<Json<MemberDto>> {
    ctx.require(OrgPermission::ManageMembers)?;
    let role = body
        .org_role
        .as_deref()
        .map(|r| {
            OrgRole::parse(r)
                .ok_or_else(|| ApiError::invalid("org_role", "unknown_role", "Role must be owner, admin, member or guest."))
        })
        .transpose()?;
    let status = body
        .status
        .as_deref()
        .map(|s| {
            MemberStatus::parse(s)
                .ok_or_else(|| ApiError::invalid("status", "unknown_status", "Status must be active or suspended."))
        })
        .transpose()?;
    if role.is_none() && status.is_none() {
        return Err(ApiError::invalid("body", "empty_update", "Provide org_role or status."));
    }

    let mut tx = state.db.begin(ctx.tenant_id).await?;
    let mut rules = change_context(tx.conn(), &ctx, user_id).await?;
    if let Some(role) = role.filter(|r| *r != rules.target_role) {
        check_member_change(rules, MemberChange::SetRole(role))?;
        store::update_membership(tx.conn(), ctx.tenant_id, user_id, Some(role.as_str()), None).await?;
        emit_membership_changed(tx.conn(), ctx.tenant_id, ctx.user_id, user_id, "role_changed", Some(role.as_str())).await?;
        // The second rule check below must see the role this request just set.
        if rules.target_role == OrgRole::Owner && rules.target_status == MemberStatus::Active {
            rules.active_owner_count -= 1;
        }
        rules.target_role = role;
        if role == OrgRole::Owner && rules.target_status == MemberStatus::Active {
            rules.active_owner_count += 1;
        }
    }
    if let Some(status) = status.filter(|s| *s != rules.target_status) {
        check_member_change(rules, MemberChange::SetStatus(status))?;
        store::update_membership(tx.conn(), ctx.tenant_id, user_id, None, Some(status.as_str())).await?;
        let change = if status == MemberStatus::Suspended { "suspended" } else { "reactivated" };
        emit_membership_changed(tx.conn(), ctx.tenant_id, ctx.user_id, user_id, change, None).await?;
    }
    let member = store::member(tx.conn(), ctx.tenant_id, user_id).await?.ok_or(ApiError::NotFound("member"))?;
    tx.commit().await?;
    Ok(Json(member.into()))
}

#[utoipa::path(delete, path = "/api/v1/organizations/current/members/{user_id}", security(("bearer" = [])),
    params(("user_id" = Uuid, Path)), responses((status = 204), (status = 403), (status = 404), (status = 409)))]
pub async fn remove(State(state): State<AppState>, ctx: OrgContext, Path(user_id): Path<Uuid>) -> ApiResult<StatusCode> {
    // No org-level permission gate here: leaving is allowed for everyone, and
    // `check_member_change` requires ManageMembers for removing anyone else.
    let mut tx = state.db.begin(ctx.tenant_id).await?;
    let rules = change_context(tx.conn(), &ctx, user_id).await?;
    check_member_change(rules, MemberChange::Remove)?;
    let org = store::organization(tx.conn(), ctx.tenant_id).await?.ok_or(ApiError::NotFound("organization"))?;
    store::delete_membership(tx.conn(), ctx.tenant_id, org.id, user_id).await?;
    emit_membership_changed(tx.conn(), ctx.tenant_id, ctx.user_id, user_id, "removed", None).await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}
