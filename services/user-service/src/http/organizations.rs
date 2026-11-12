use auth_middleware::{AuthContext, OrgContext, OrgPermission, OrgRole};
use axum::{
    extract::State,
    http::{header::ETAG, HeaderMap, HeaderValue, StatusCode},
    Json,
};
use error_handling::{require_if_match, ApiError, ApiResult};
use event_contracts::{
    outbox,
    payloads::{MembershipChangedV1, OrganizationCreatedV1, OrganizationUpdatedV1},
    Actor, EventEnvelope,
};
use uuid::Uuid;
use validator::Validate;

use super::dto::{CreateOrganizationRequest, OrganizationDto, OrganizationList, UpdateOrganizationRequest};
use crate::{
    domain::{slugify, validate_slug},
    store, AppState,
};

fn etag(version: i64) -> HeaderMap {
    let mut headers = HeaderMap::new();
    if let Ok(value) = HeaderValue::from_str(&format!("\"{version}\"")) {
        headers.insert(ETAG, value);
    }
    headers
}

#[utoipa::path(get, path = "/api/v1/organizations", security(("bearer" = [])), responses((status = 200, body = OrganizationList)))]
pub async fn list_mine(State(state): State<AppState>, auth: AuthContext) -> ApiResult<Json<OrganizationList>> {
    let mut items = Vec::new();
    for (_, tenant_id) in store::directory_for_user(state.db.global()?, auth.user_id).await? {
        // Each organization is read inside its own tenant scope. One unavailable tenant
        // (migrating, suspended) must not hide the others.
        let mut tx = match state.db.begin(tenant_id).await {
            Ok(tx) => tx,
            Err(ApiError::TenantUnavailable(_) | ApiError::NotFound(_)) => continue,
            Err(err) => return Err(err),
        };
        let org = store::organization(tx.conn(), tenant_id).await?;
        let membership = store::membership(tx.conn(), tenant_id, auth.user_id).await?;
        tx.commit().await?;
        if let (Some(org), Some(m)) = (org, membership) {
            if m.status == "active" {
                items.push(OrganizationDto::new(org, &m.org_role));
            }
        }
    }
    Ok(Json(OrganizationList { items }))
}

#[utoipa::path(post, path = "/api/v1/organizations", request_body = CreateOrganizationRequest, security(("bearer" = [])),
    responses((status = 201, body = OrganizationDto), (status = 409, description = "URL name already taken"), (status = 422)))]
pub async fn create(
    State(state): State<AppState>,
    auth: AuthContext,
    Json(mut body): Json<CreateOrganizationRequest>,
) -> ApiResult<(StatusCode, HeaderMap, Json<OrganizationDto>)> {
    body.name = body.name.trim().to_owned();
    body.validate()?;
    let slug = body.slug.as_deref().map(|s| s.trim().to_lowercase()).unwrap_or_else(|| slugify(&body.name));
    validate_slug(&slug)?;
    store::ensure_profile(state.db.global()?, auth.user_id, &auth.email, &auth.name).await?;

    // Saga step 1: a tenant must exist before any tenant-owned row can be written.
    let organization_id = platform_common::new_id();
    let tenant_id = state.tenants.provision(organization_id).await?;

    // Saga step 2: organization, owner membership and event commit atomically.
    let created = async {
        let mut tx = state.db.begin(tenant_id).await?;
        let org = match store::insert_organization(tx.conn(), organization_id, tenant_id, &body.name, &slug).await {
            Ok(org) => org,
            Err(sqlx::Error::Database(db)) if db.is_unique_violation() => {
                return Err(ApiError::Conflict("That URL name is already taken. Choose another.".into()));
            }
            Err(err) => return Err(err.into()),
        };
        store::insert_membership(tx.conn(), tenant_id, organization_id, auth.user_id, OrgRole::Owner.as_str()).await?;
        let (_, seq) = store::next_membership_seq(tx.conn(), tenant_id).await?;
        outbox::enqueue(
            tx.conn(),
            &EventEnvelope::new(
                Some(tenant_id),
                organization_id,
                org.version,
                Actor::User(auth.user_id),
                OrganizationCreatedV1 {
                    organization_id,
                    name: org.name.clone(),
                    slug: org.slug.clone(),
                    owner_user_id: auth.user_id,
                },
            ),
        )
        .await?;
        outbox::enqueue(
            tx.conn(),
            &EventEnvelope::new(
                Some(tenant_id),
                organization_id,
                seq,
                Actor::User(auth.user_id),
                MembershipChangedV1 {
                    organization_id,
                    user_id: auth.user_id,
                    change: "joined".into(),
                    org_role: Some("owner".into()),
                },
            ),
        )
        .await?;
        tx.commit().await?;
        Ok::<_, ApiError>(org)
    }
    .await;

    match created {
        Ok(org) => {
            tracing::info!(%organization_id, %tenant_id, "organization created");
            let headers = etag(org.version);
            Ok((StatusCode::CREATED, headers, Json(OrganizationDto::new(org, OrgRole::Owner.as_str()))))
        }
        Err(err) => {
            // Compensate step 1 so no tenant is left without an organization.
            state.tenants.abandon(tenant_id).await;
            Err(err)
        }
    }
}

#[utoipa::path(get, path = "/api/v1/organizations/current", security(("bearer" = [])), responses((status = 200, body = OrganizationDto)))]
pub async fn current(State(state): State<AppState>, ctx: OrgContext) -> ApiResult<(HeaderMap, Json<OrganizationDto>)> {
    ctx.require(OrgPermission::ViewOrganization)?;
    let mut tx = state.db.begin(ctx.tenant_id).await?;
    let org = store::organization(tx.conn(), ctx.tenant_id).await?.ok_or(ApiError::NotFound("organization"))?;
    tx.commit().await?;
    Ok((etag(org.version), Json(OrganizationDto::new(org, ctx.role.as_str()))))
}

#[utoipa::path(patch, path = "/api/v1/organizations/current", request_body = UpdateOrganizationRequest, security(("bearer" = [])),
    params(("If-Match" = String, Header, description = "Version the edit is based on")),
    responses((status = 200, body = OrganizationDto), (status = 409, description = "Edited by someone else"), (status = 428)))]
pub async fn update_current(
    State(state): State<AppState>,
    ctx: OrgContext,
    headers: HeaderMap,
    Json(mut body): Json<UpdateOrganizationRequest>,
) -> ApiResult<(HeaderMap, Json<OrganizationDto>)> {
    ctx.require(OrgPermission::ManageOrganization)?;
    let expected = require_if_match(&headers)?;
    body.name = body.name.trim().to_owned();
    body.validate()?;

    let mut tx = state.db.begin(ctx.tenant_id).await?;
    let Some(org) = store::rename_organization(tx.conn(), ctx.tenant_id, &body.name, expected).await? else {
        let current = store::organization(tx.conn(), ctx.tenant_id).await?.ok_or(ApiError::NotFound("organization"))?;
        let current = serde_json::to_value(OrganizationDto::new(current, ctx.role.as_str())).ok();
        return Err(ApiError::VersionConflict { current });
    };
    outbox::enqueue(
        tx.conn(),
        &EventEnvelope::new(
            Some(ctx.tenant_id),
            org.id,
            org.version,
            Actor::User(ctx.user_id),
            OrganizationUpdatedV1 { organization_id: org.id, name: org.name.clone() },
        ),
    )
    .await?;
    tx.commit().await?;
    Ok((etag(org.version), Json(OrganizationDto::new(org, ctx.role.as_str()))))
}

/// Shared by member and invitation handlers: emit `membership.changed` with the next
/// membership version. Must run inside the caller's tenant transaction.
pub(super) async fn emit_membership_changed(
    conn: &mut sqlx::PgConnection,
    tenant_id: Uuid,
    actor: Uuid,
    user_id: Uuid,
    change: &str,
    role: Option<&str>,
) -> ApiResult<()> {
    let (organization_id, seq) = store::next_membership_seq(conn, tenant_id).await?;
    outbox::enqueue(
        conn,
        &EventEnvelope::new(
            Some(tenant_id),
            organization_id,
            seq,
            Actor::User(actor),
            MembershipChangedV1 { organization_id, user_id, change: change.to_owned(), org_role: role.map(str::to_owned) },
        ),
    )
    .await?;
    Ok(())
}
