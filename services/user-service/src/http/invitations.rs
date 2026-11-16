use auth_middleware::{AuthContext, OrgContext, OrgPermission, OrgRole};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use error_handling::{ApiError, ApiResult};
use event_contracts::{outbox, payloads::UserInvitedV1, Actor, EventEnvelope};
use rand::RngCore;
use sha2::{Digest, Sha256};
use uuid::Uuid;
use validator::Validate;

use super::{
    dto::{AcceptInvitationResponse, CreateInvitationRequest, InvitationDto, InvitationList, InvitationPreview},
    organizations::emit_membership_changed,
};
use crate::{domain::check_can_invite, invite_mail, store, AppState};

const INVITATION_TTL_DAYS: i32 = 7;
/// Unknown, revoked, accepted and malformed tokens all look the same from outside.
const NOT_FOUND: ApiError = ApiError::NotFound("invitation");

fn new_token() -> String {
    let mut bytes = [0_u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

fn hash(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}

#[utoipa::path(get, path = "/api/v1/organizations/current/invitations", security(("bearer" = [])),
    responses((status = 200, body = InvitationList)))]
pub async fn list(State(state): State<AppState>, ctx: OrgContext) -> ApiResult<Json<InvitationList>> {
    ctx.require(OrgPermission::InviteMembers)?;
    let mut tx = state.db.begin(ctx.tenant_id).await?;
    let rows = store::open_invitations(tx.conn(), ctx.tenant_id).await?;
    tx.commit().await?;
    Ok(Json(InvitationList { items: rows.into_iter().map(|i| InvitationDto::new(i, None)).collect() }))
}

#[utoipa::path(post, path = "/api/v1/organizations/current/invitations", request_body = CreateInvitationRequest,
    security(("bearer" = [])),
    responses((status = 201, body = InvitationDto), (status = 409, description = "Already a member or already invited"), (status = 422)))]
pub async fn create(
    State(state): State<AppState>,
    ctx: OrgContext,
    Json(mut body): Json<CreateInvitationRequest>,
) -> ApiResult<(StatusCode, Json<InvitationDto>)> {
    ctx.require(OrgPermission::InviteMembers)?;
    body.email = body.email.trim().to_lowercase();
    body.validate()?;
    let role = OrgRole::parse(&body.org_role)
        .ok_or_else(|| ApiError::invalid("org_role", "unknown_role", "Role must be admin, member or guest."))?;
    check_can_invite(ctx.role, role)?;

    let mut tx = state.db.begin(ctx.tenant_id).await?;
    let org = store::organization(tx.conn(), ctx.tenant_id).await?.ok_or(ApiError::NotFound("organization"))?;
    if store::is_member_email(tx.conn(), ctx.tenant_id, &body.email).await? {
        return Err(ApiError::Conflict("This person is already a member of the organization.".into()));
    }
    let (invitation, token) =
        issue(tx.conn(), &state, ctx.tenant_id, org.id, &body.email, role, ctx.user_id, Actor::User(ctx.user_id)).await?;
    tx.commit().await?;

    let accept_url = accept_url(&state, &token);
    email_invitation(&state, &ctx.name, &org.name, &invitation, &accept_url);
    Ok((StatusCode::CREATED, Json(InvitationDto::new(invitation, Some(accept_url)))))
}

/// Create an invitation (role already checked) and its `user.invited` event; returns it with
/// its secret token. A pending invitation to the same address is a 409.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn issue(
    conn: &mut sqlx::PgConnection,
    state: &AppState,
    tenant_id: Uuid,
    organization_id: Uuid,
    email: &str,
    role: OrgRole,
    invited_by: Uuid,
    actor: Actor,
) -> ApiResult<(store::Invitation, String)> {
    let token = new_token();
    let id = platform_common::new_id();
    store::revoke_expired_invitations(conn, tenant_id, email).await?;
    match store::insert_invitation(
        conn,
        id,
        tenant_id,
        organization_id,
        email,
        role.as_str(),
        invited_by,
        INVITATION_TTL_DAYS,
        &hash(&token),
    )
    .await
    {
        Ok(()) => {}
        Err(sqlx::Error::Database(db)) if db.is_unique_violation() => {
            return Err(ApiError::Conflict("An invitation for this address is already pending.".into()));
        }
        Err(err) => return Err(err.into()),
    }
    if state.mail.is_some() {
        store::mark_invitation_emailed(conn, tenant_id, id).await?;
    }
    let (_, seq) = store::next_membership_seq(conn, tenant_id).await?;
    outbox::enqueue(
        conn,
        &EventEnvelope::new(
            Some(tenant_id),
            organization_id,
            seq,
            actor,
            UserInvitedV1 {
                invitation_id: id,
                organization_id,
                email: email.to_owned(),
                org_role: role.as_str().to_owned(),
                invited_by,
            },
        ),
    )
    .await?;
    let invitation = store::invitation(conn, tenant_id, id, false).await?.ok_or(NOT_FOUND)?;
    Ok((invitation, token))
}

pub(crate) fn accept_url(state: &AppState, token: &str) -> String {
    format!("{}/invite/{token}", state.settings.web_base_url.trim_end_matches('/'))
}

pub(crate) fn email_invitation(
    state: &AppState,
    inviter_name: &str,
    organization_name: &str,
    invitation: &store::Invitation,
    accept_url: &str,
) {
    let Some(mail) = &state.mail else { return };
    let rendered = invite_mail::render(&invite_mail::InviteEmail {
        inviter_name,
        organization_name,
        role_label: invite_mail::role_label(&invitation.org_role),
        accept_url,
        expires_at: invitation.expires_at,
    });
    invite_mail::send(mail, invitation.id, invitation.email.clone(), rendered);
}

/// No new link within this long of the last email.
const RESEND_INTERVAL: time::Duration = time::Duration::seconds(60);

/// Give an open (or expired) invitation a new link, valid for another 7 days, and email it. The
/// previous link stops working. The new link is in the response, as when the invitation was made.
#[utoipa::path(post, path = "/api/v1/organizations/current/invitations/{id}/resend", security(("bearer" = [])),
    params(("id" = Uuid, Path)),
    responses((status = 200, body = InvitationDto), (status = 404), (status = 409, description = "Already a member"),
        (status = 429, description = "Emailed less than a minute ago")))]
pub async fn resend(State(state): State<AppState>, ctx: OrgContext, Path(id): Path<Uuid>) -> ApiResult<Json<InvitationDto>> {
    ctx.require(OrgPermission::InviteMembers)?;
    let token = new_token();
    let mut tx = state.db.begin(ctx.tenant_id).await?;
    let current = store::invitation(tx.conn(), ctx.tenant_id, id, true).await?.ok_or(NOT_FOUND)?;
    if !matches!(current.state(), "pending" | "expired") {
        return Err(NOT_FOUND);
    }
    // The sender must still be allowed to grant the role.
    check_can_invite(ctx.role, OrgRole::parse(&current.org_role).ok_or(NOT_FOUND)?)?;
    if let Some(last) = current.last_sent_at {
        let wait = RESEND_INTERVAL - (time::OffsetDateTime::now_utc() - last);
        if wait > time::Duration::ZERO {
            return Err(ApiError::RateLimited { retry_after_secs: wait.whole_seconds().max(1) as u64 });
        }
    }
    if store::is_member_email(tx.conn(), ctx.tenant_id, &current.email).await? {
        return Err(ApiError::Conflict("This person is already a member of the organization.".into()));
    }
    store::renew_invitation(tx.conn(), ctx.tenant_id, id, INVITATION_TTL_DAYS, &hash(&token), state.mail.is_some()).await?;
    let org = store::organization(tx.conn(), ctx.tenant_id).await?.ok_or(ApiError::NotFound("organization"))?;
    let invitation = store::invitation(tx.conn(), ctx.tenant_id, id, false).await?.ok_or(NOT_FOUND)?;
    tx.commit().await?;

    let accept_url = accept_url(&state, &token);
    email_invitation(&state, &ctx.name, &org.name, &invitation, &accept_url);
    tracing::info!(invitation_id = %id, "invitation renewed");
    Ok(Json(InvitationDto::new(invitation, Some(accept_url))))
}

#[utoipa::path(delete, path = "/api/v1/organizations/current/invitations/{id}", security(("bearer" = [])),
    params(("id" = Uuid, Path)), responses((status = 204), (status = 404)))]
pub async fn revoke(State(state): State<AppState>, ctx: OrgContext, Path(id): Path<Uuid>) -> ApiResult<StatusCode> {
    ctx.require(OrgPermission::InviteMembers)?;
    let mut tx = state.db.begin(ctx.tenant_id).await?;
    let revoked = store::revoke_invitation(tx.conn(), ctx.tenant_id, id).await?;
    tx.commit().await?;
    if revoked {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(NOT_FOUND)
    }
}

/// Tokens are 32 random bytes, base64url: anything else cannot be one of ours.
fn plausible(token: &str) -> bool {
    token.len() == 43 && token.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

#[utoipa::path(get, path = "/api/v1/invitations/{token}", params(("token" = String, Path)),
    responses((status = 200, body = InvitationPreview), (status = 404)))]
pub async fn preview(State(state): State<AppState>, Path(token): Path<String>) -> ApiResult<Json<InvitationPreview>> {
    if !plausible(&token) {
        return Err(NOT_FOUND);
    }
    let (tenant_id, invitation_id) = store::invitation_location(state.db.global()?, &hash(&token)).await?.ok_or(NOT_FOUND)?;
    let mut tx = state.db.begin(tenant_id).await?;
    let invitation = store::invitation(tx.conn(), tenant_id, invitation_id, false).await?.ok_or(NOT_FOUND)?;
    let org = store::organization(tx.conn(), tenant_id).await?.ok_or(NOT_FOUND)?;
    tx.commit().await?;
    let state_name = invitation.state();
    if !matches!(state_name, "pending" | "expired") {
        return Err(NOT_FOUND);
    }
    Ok(Json(InvitationPreview {
        organization_name: org.name,
        email: invitation.email,
        org_role: invitation.org_role,
        invited_by_name: invitation.invited_by_name,
        state: state_name.to_owned(),
        expires_at: invitation.expires_at,
    }))
}

const EMAIL_UNVERIFIED: ApiError = ApiError::ForbiddenBecause {
    code: "email_unverified",
    message: "Verify your email address first: use the link we emailed you, then try again.",
};

#[utoipa::path(post, path = "/api/v1/invitations/{token}/accept", security(("bearer" = [])), params(("token" = String, Path)),
    responses((status = 200, body = AcceptInvitationResponse), (status = 403, description = "`email_unverified`: the caller's address is not verified"),
        (status = 404), (status = 409, description = "Expired or sent to another address")))]
pub async fn accept(
    State(state): State<AppState>,
    auth: AuthContext,
    Path(token): Path<String>,
) -> ApiResult<Json<AcceptInvitationResponse>> {
    if !plausible(&token) {
        return Err(NOT_FOUND);
    }
    let pool = state.db.global()?;
    let (tenant_id, invitation_id) = store::invitation_location(pool, &hash(&token)).await?.ok_or(NOT_FOUND)?;
    store::ensure_profile(pool, auth.user_id, &auth.email, &auth.name).await?;

    let mut tx = state.db.begin(tenant_id).await?;
    let invitation = store::invitation(tx.conn(), tenant_id, invitation_id, true).await?.ok_or(NOT_FOUND)?;
    match invitation.state() {
        "pending" => {}
        "expired" => return Err(ApiError::Conflict("This invitation has expired. Ask for a new one.".into())),
        _ => return Err(NOT_FOUND),
    }
    // Possession of the link is not enough: it must be redeemed by the invited address, and the
    // person must have proved they receive mail there (otherwise anyone could register the
    // address first and take the seat).
    if !invitation.email.eq_ignore_ascii_case(&auth.email) {
        return Err(ApiError::Conflict("This invitation was sent to a different email address.".into()));
    }
    if !auth.claims.email_verified {
        return Err(EMAIL_UNVERIFIED);
    }
    let org = store::organization(tx.conn(), tenant_id).await?.ok_or(NOT_FOUND)?;
    let joined = store::insert_membership(tx.conn(), tenant_id, org.id, auth.user_id, &invitation.org_role).await?;
    store::mark_invitation_accepted(tx.conn(), tenant_id, invitation_id, auth.user_id).await?;
    // A person provisioned by the workspace's identity provider is now that member.
    crate::http::scim::link_accepted(tx.conn(), tenant_id, invitation_id, auth.user_id).await?;
    if joined {
        emit_membership_changed(tx.conn(), tenant_id, auth.user_id, auth.user_id, "joined", Some(&invitation.org_role)).await?;
    }
    tx.commit().await?;
    tracing::info!(organization_id = %org.id, user_id = %auth.user_id, "invitation accepted");
    Ok(Json(AcceptInvitationResponse { organization_id: org.id, organization_name: org.name }))
}
