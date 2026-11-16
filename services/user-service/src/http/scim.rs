//! The SCIM 2.0 endpoints an identity provider calls, under `/scim/v2`, authenticated with a
//! workspace provisioning token (see `provisioning.rs` for how admins make one).
//!
//! What provisioning does to the workspace:
//! * A new active user whose address belongs to a member is linked to that member. Otherwise
//!   they are invited as a member (the usual invitation email; they join by accepting it with
//!   that verified address), or an open invitation to the address is adopted.
//! * `active: false` suspends the member, or withdraws the invitation. `active: true` reactivates
//!   or invites again. `DELETE` removes the member, or withdraws the invitation.
//! * Every change to a membership follows the same rules as in the app, with the role of the
//!   admin who made the token (re-read on each request): only an owner's token can touch an
//!   owner, and the last active owner cannot be suspended or removed.
//! * Names from the provider are kept on the provisioned user; the person's own profile, which
//!   their other workspaces share, is not changed. Nor is anyone's account.

use auth_middleware::OrgRole;
use axum::{
    body::Bytes,
    extract::{Path, Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use error_handling::ApiError;
use event_contracts::Actor;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::{FromRow, PgConnection};
use std::collections::HashMap;
use tenant_context::TenantTx;
use time::OffsetDateTime;
use uuid::Uuid;

use super::{invitations, organizations::emit_membership_changed_by};
use crate::{
    domain::{check_can_invite, check_member_change, MemberChange, MemberChangeContext, MemberStatus},
    scim::{parse_filter, scim_json, Filter, Person, ScimError, Stored, LIST_SCHEMA},
    store, AppState,
};

pub const TOKEN_PREFIX: &str = "mscim_";
/// Most users one page returns.
const MAX_COUNT: i64 = 200;

impl From<ApiError> for ScimError {
    fn from(error: ApiError) -> Self {
        match error {
            ApiError::NotFound(_) => ScimError::not_found(),
            ApiError::Conflict(message) => ScimError::new(StatusCode::CONFLICT, None, message),
            ApiError::Forbidden | ApiError::ForbiddenBecause { .. } => ScimError::new(
                StatusCode::FORBIDDEN,
                None,
                "The admin who created this token is not allowed to make this change (only an owner can change an owner).",
            ),
            ApiError::TenantUnavailable(_) | ApiError::DependencyUnavailable(_) => {
                ScimError::new(StatusCode::SERVICE_UNAVAILABLE, None, "The workspace is briefly unavailable. Try again.")
            }
            other => {
                tracing::error!(error = %other, "provisioning request failed");
                ScimError::new(StatusCode::INTERNAL_SERVER_ERROR, None, "Something went wrong. Try again.")
            }
        }
    }
}

impl From<sqlx::Error> for ScimError {
    fn from(error: sqlx::Error) -> Self {
        ApiError::from(error).into()
    }
}

pub fn token_hash(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}

/// Tokens are the prefix and 32 random bytes in base64url.
fn plausible(token: &str) -> bool {
    token
        .strip_prefix(TOKEN_PREFIX)
        .is_some_and(|rest| rest.len() == 43 && rest.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'))
}

/// Who is calling: a workspace's token, acting with its creator's current role.
struct Caller {
    tenant_id: Uuid,
    organization_id: Uuid,
    organization_name: String,
    token_id: Uuid,
    admin: Uuid,
    admin_role: OrgRole,
    admin_name: String,
}

impl Caller {
    fn actor(&self) -> Actor {
        Actor::System(format!("scim:{}", self.token_id))
    }
}

/// Check the bearer token and open the workspace's transaction, with the organization row
/// locked so concurrent provisioning requests apply one at a time.
async fn authenticate(state: &AppState, headers: &HeaderMap) -> Result<(TenantTx, Caller), ScimError> {
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.get(..7).filter(|p| p.eq_ignore_ascii_case("bearer ")).map(|_| v[7..].trim()))
        .filter(|t| plausible(t))
        .ok_or_else(ScimError::unauthorized)?;
    let found: Option<(Uuid, Uuid)> = sqlx::query_as("SELECT tenant_id, token_id FROM scim_token_index WHERE token_hash = $1")
        .bind(token_hash(token))
        .fetch_optional(state.db.global()?)
        .await?;
    let (tenant_id, token_id) = found.ok_or_else(ScimError::unauthorized)?;

    let mut tx = state.db.begin(tenant_id).await?;
    let (organization_id, organization_name): (Uuid, String) =
        sqlx::query_as("SELECT id, name FROM organizations WHERE tenant_id = $1 FOR UPDATE")
            .bind(tenant_id)
            .fetch_optional(tx.conn())
            .await?
            .ok_or_else(ScimError::unauthorized)?;
    let admin: Option<(Uuid,)> = sqlx::query_as(
        "UPDATE scim_tokens SET last_used_at = now() WHERE tenant_id = $1 AND id = $2 AND revoked_at IS NULL RETURNING created_by",
    )
    .bind(tenant_id)
    .bind(token_id)
    .fetch_optional(tx.conn())
    .await?;
    let (admin,) = admin.ok_or_else(ScimError::unauthorized)?;
    // The token is only as strong as its creator is now.
    let admin_role = store::membership(tx.conn(), tenant_id, admin)
        .await?
        .filter(|m| m.status == "active")
        .and_then(|m| OrgRole::parse(&m.org_role))
        .filter(|role| *role >= OrgRole::Admin)
        .ok_or_else(|| {
            ScimError::new(
                StatusCode::UNAUTHORIZED,
                None,
                "The admin who created this provisioning token is no longer an admin of the workspace. Create a new token.",
            )
        })?;
    let (admin_name,): (String,) =
        sqlx::query_as("SELECT COALESCE((SELECT display_name FROM user_profiles WHERE user_id = $1), 'An admin')")
            .bind(admin)
            .fetch_one(tx.conn())
            .await?;
    Ok((tx, Caller { tenant_id, organization_id, organization_name, token_id, admin, admin_role, admin_name }))
}

fn parse_body(body: &Bytes) -> Result<Value, ScimError> {
    serde_json::from_slice(body)
        .map_err(|_| ScimError::new(StatusCode::BAD_REQUEST, Some("invalidSyntax"), "The body is not valid JSON."))
}

#[derive(Debug, FromRow)]
struct Row {
    id: Uuid,
    user_name: String,
    email: String,
    external_id: Option<String>,
    display_name: Option<String>,
    given_name: Option<String>,
    family_name: Option<String>,
    active: bool,
    user_id: Option<Uuid>,
    invitation_id: Option<Uuid>,
    version: i64,
    created_at: OffsetDateTime,
    updated_at: OffsetDateTime,
}

const SELECT: &str = "SELECT id, user_name::text AS user_name, email::text AS email, external_id, display_name, given_name, \
     family_name, active, user_id, invitation_id, version, created_at, updated_at FROM scim_users";

impl Row {
    fn person(&self) -> Person {
        Person {
            user_name: self.user_name.clone(),
            email: (!self.email.eq_ignore_ascii_case(&self.user_name)).then(|| self.email.clone()),
            external_id: self.external_id.clone(),
            display_name: self.display_name.clone(),
            given_name: self.given_name.clone(),
            family_name: self.family_name.clone(),
            active: self.active,
        }
    }

    fn resource(&self, state: &AppState) -> Value {
        let stored = Stored { id: self.id, version: self.version, created_at: self.created_at, updated_at: self.updated_at };
        self.person().to_resource(&stored, &location(state, self.id))
    }
}

fn base(state: &AppState) -> &str {
    state.settings.scim_base_url.trim_end_matches('/')
}

fn location(state: &AppState, id: Uuid) -> String {
    format!("{}/Users/{id}", base(state))
}

async fn load(conn: &mut PgConnection, tenant_id: Uuid, id: Uuid) -> Result<Row, ScimError> {
    sqlx::query_as::<_, Row>(&format!("{SELECT} WHERE tenant_id = $1 AND id = $2 FOR UPDATE"))
        .bind(tenant_id)
        .bind(id)
        .fetch_optional(conn)
        .await?
        .ok_or_else(ScimError::not_found)
}

/// `userName` is unique in the workspace, ignoring case.
async fn ensure_unique(conn: &mut PgConnection, tenant_id: Uuid, user_name: &str, except: Option<Uuid>) -> Result<(), ScimError> {
    let (taken,): (bool,) = sqlx::query_as(
        "SELECT EXISTS (SELECT 1 FROM scim_users WHERE tenant_id = $1 AND user_name = $2::citext AND id IS DISTINCT FROM $3)",
    )
    .bind(tenant_id)
    .bind(user_name)
    .bind(except)
    .fetch_one(conn)
    .await?;
    if taken {
        return Err(ScimError::uniqueness("A user with this userName is already provisioned."));
    }
    Ok(())
}

/// Who a provisioned user is in the workspace.
#[derive(Debug, Clone, Copy, Default)]
struct Link {
    user_id: Option<Uuid>,
    invitation_id: Option<Uuid>,
}

/// An invitation to email once the transaction has committed.
type Outgoing = Option<(store::Invitation, String)>;

/// Suspend or reactivate a member under the app's rules.
async fn set_status(
    conn: &mut PgConnection,
    caller: &Caller,
    user_id: Uuid,
    membership: &store::Membership,
    active: bool,
) -> Result<(), ScimError> {
    let want = if active { MemberStatus::Active } else { MemberStatus::Suspended };
    let current = MemberStatus::parse(&membership.status).ok_or_else(|| ApiError::internal(anyhow::anyhow!("unknown status")))?;
    if current == want {
        return Ok(());
    }
    let rules = MemberChangeContext {
        actor_role: caller.admin_role,
        actor_is_target: caller.admin == user_id,
        target_role: OrgRole::parse(&membership.org_role).ok_or_else(|| ApiError::internal(anyhow::anyhow!("unknown role")))?,
        target_status: current,
        active_owner_count: store::active_owner_count(conn, caller.tenant_id).await?,
    };
    check_member_change(rules, MemberChange::SetStatus(want))?;
    store::update_membership(conn, caller.tenant_id, user_id, None, Some(want.as_str())).await?;
    let change = if active { "reactivated" } else { "suspended" };
    emit_membership_changed_by(conn, caller.tenant_id, caller.actor(), user_id, change, None).await?;
    Ok(())
}

/// Bring the workspace in line with what the provider says about one person.
async fn reconcile(
    conn: &mut PgConnection,
    state: &AppState,
    caller: &Caller,
    id: Uuid,
    mut link: Link,
    person: &Person,
) -> Result<(Link, Outgoing), ScimError> {
    let address = person.address()?;
    let tenant_id = caller.tenant_id;

    // Already a member: only their status follows the provider. (If they were removed in the
    // app since, they start over below.)
    if let Some(user_id) = link.user_id {
        if let Some(membership) = store::membership(conn, tenant_id, user_id).await? {
            set_status(conn, caller, user_id, &membership, person.active).await?;
            return Ok((link, None));
        }
        link.user_id = None;
    }

    // An open invitation stands while the person is active and the address is the same.
    if let Some(invitation_id) = link.invitation_id {
        let invitation = store::invitation(conn, tenant_id, invitation_id, true).await?;
        let keep = invitation
            .as_ref()
            .is_some_and(|i| i.state() == "pending" && i.email.eq_ignore_ascii_case(&address) && person.active);
        if !keep {
            store::revoke_invitation(conn, tenant_id, invitation_id).await?;
            link.invitation_id = None;
        }
    }

    // The address belongs to a member: link them.
    if link.invitation_id.is_none() {
        let member: Option<(Uuid,)> = sqlx::query_as(
            "SELECT m.user_id FROM memberships m JOIN user_profiles p ON p.user_id = m.user_id \
             WHERE m.tenant_id = $1 AND p.email = $2::citext",
        )
        .bind(tenant_id)
        .bind(&address)
        .fetch_optional(&mut *conn)
        .await?;
        if let Some((user_id,)) = member {
            let (taken,): (bool,) =
                sqlx::query_as("SELECT EXISTS (SELECT 1 FROM scim_users WHERE tenant_id = $1 AND user_id = $2 AND id <> $3)")
                    .bind(tenant_id)
                    .bind(user_id)
                    .bind(id)
                    .fetch_one(&mut *conn)
                    .await?;
            if taken {
                return Err(ScimError::uniqueness("This person is already provisioned as another user."));
            }
            let membership = store::membership(conn, tenant_id, user_id).await?.ok_or_else(ScimError::not_found)?;
            set_status(conn, caller, user_id, &membership, person.active).await?;
            return Ok((Link { user_id: Some(user_id), invitation_id: None }, None));
        }
    }

    if !person.active || link.invitation_id.is_some() {
        return Ok((link, None));
    }

    // An invitation someone already sent to the address is adopted rather than duplicated.
    let open: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM invitations WHERE tenant_id = $1 AND email = $2::citext \
         AND accepted_at IS NULL AND revoked_at IS NULL AND expires_at > now()",
    )
    .bind(tenant_id)
    .bind(&address)
    .fetch_optional(&mut *conn)
    .await?;
    if let Some((invitation_id,)) = open {
        return Ok((Link { user_id: None, invitation_id: Some(invitation_id) }, None));
    }

    check_can_invite(caller.admin_role, OrgRole::Member)?;
    let (invitation, token) = invitations::issue(
        conn,
        state,
        tenant_id,
        caller.organization_id,
        &address,
        OrgRole::Member,
        caller.admin,
        caller.actor(),
    )
    .await?;
    Ok((Link { user_id: None, invitation_id: Some(invitation.id) }, Some((invitation, token))))
}

fn send(state: &AppState, caller: &Caller, outgoing: Outgoing) {
    if let Some((invitation, token)) = outgoing {
        let url = invitations::accept_url(state, &token);
        invitations::email_invitation(state, &caller.admin_name, &caller.organization_name, &invitation, &url);
        tracing::info!(invitation_id = %invitation.id, token_id = %caller.token_id, "provisioning invited a person");
    }
}

/// Called when an invitation is accepted: a provisioned user waiting on it becomes that member.
pub(crate) async fn link_accepted(
    conn: &mut PgConnection,
    tenant_id: Uuid,
    invitation_id: Uuid,
    user_id: Uuid,
) -> Result<(), ApiError> {
    sqlx::query(
        "UPDATE scim_users SET user_id = $3, invitation_id = NULL, version = version + 1, updated_at = now() \
         WHERE tenant_id = $1 AND invitation_id = $2 AND user_id IS NULL \
           AND NOT EXISTS (SELECT 1 FROM scim_users WHERE tenant_id = $1 AND user_id = $3)",
    )
    .bind(tenant_id)
    .bind(invitation_id)
    .bind(user_id)
    .execute(conn)
    .await?;
    Ok(())
}

// --- endpoints ------------------------------------------------------------------------------

pub async fn service_provider_config() -> Response {
    scim_json(StatusCode::OK, &crate::scim::service_provider_config())
}

pub async fn resource_types(State(state): State<AppState>) -> Response {
    scim_json(StatusCode::OK, &crate::scim::resource_types(base(&state)))
}

pub async fn schemas() -> Response {
    scim_json(StatusCode::OK, &crate::scim::schemas())
}

pub async fn groups() -> Response {
    ScimError::new(StatusCode::NOT_FOUND, None, "Groups are not supported. Provision users only.").into_response()
}

pub async fn list_users(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> Result<Response, ScimError> {
    let get = |name: &str| query.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str());
    let number = |name: &str, default: i64| -> Result<i64, ScimError> {
        get(name)
            .map_or(Ok(default), |v| v.trim().parse().map_err(|_| ScimError::invalid_value(format!("{name} must be a number."))))
    };
    let start = number("startIndex", 1)?.max(1);
    let count = number("count", 100)?.clamp(0, MAX_COUNT);
    let filter = get("filter").filter(|f| !f.trim().is_empty()).map(parse_filter).transpose()?;

    let (mut tx, caller) = authenticate(&state, &headers).await?;
    let (mut user_name, mut external_id, mut email, mut id) = (None, None, None, None);
    match filter {
        None => {}
        Some(Filter::UserName(v)) => user_name = Some(v),
        Some(Filter::ExternalId(v)) => external_id = Some(v),
        Some(Filter::Email(v)) => email = Some(v),
        Some(Filter::Id(v)) => id = Some(v),
        // An id that cannot exist matches nothing.
        Some(Filter::Nothing) => id = Some(Uuid::nil()),
    }
    let conditions =
        "tenant_id = $1 AND ($2::text IS NULL OR user_name = $2::citext) AND ($3::text IS NULL OR external_id = $3) \
         AND ($4::text IS NULL OR email = $4::citext) AND ($5::uuid IS NULL OR id = $5)";
    let (total,): (i64,) = sqlx::query_as(&format!("SELECT count(*) FROM scim_users WHERE {conditions}"))
        .bind(caller.tenant_id)
        .bind(&user_name)
        .bind(&external_id)
        .bind(&email)
        .bind(id)
        .fetch_one(tx.conn())
        .await?;
    let rows = sqlx::query_as::<_, Row>(&format!("{SELECT} WHERE {conditions} ORDER BY created_at, id OFFSET $6 LIMIT $7"))
        .bind(caller.tenant_id)
        .bind(&user_name)
        .bind(&external_id)
        .bind(&email)
        .bind(id)
        .bind(start - 1)
        .bind(count)
        .fetch_all(tx.conn())
        .await?;
    tx.commit().await?;
    let resources: Vec<Value> = rows.iter().map(|r| r.resource(&state)).collect();
    Ok(scim_json(
        StatusCode::OK,
        &json!({
            "schemas": [LIST_SCHEMA],
            "totalResults": total,
            "itemsPerPage": resources.len(),
            "startIndex": start,
            "Resources": resources,
        }),
    ))
}

pub async fn get_user(State(state): State<AppState>, headers: HeaderMap, Path(id): Path<String>) -> Result<Response, ScimError> {
    let (mut tx, caller) = authenticate(&state, &headers).await?;
    let id: Uuid = id.parse().map_err(|_| ScimError::not_found())?;
    let row = load(tx.conn(), caller.tenant_id, id).await?;
    tx.commit().await?;
    Ok(scim_json(StatusCode::OK, &row.resource(&state)))
}

pub async fn create_user(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Result<Response, ScimError> {
    let (mut tx, caller) = authenticate(&state, &headers).await?;
    let person = Person::from_resource(&parse_body(&body)?)?;
    ensure_unique(tx.conn(), caller.tenant_id, &person.user_name, None).await?;
    let id = platform_common::new_id();
    let (link, outgoing) = reconcile(tx.conn(), &state, &caller, id, Link::default(), &person).await?;
    let row = sqlx::query_as::<_, Row>(
        "INSERT INTO scim_users (id, tenant_id, organization_id, user_name, email, external_id, display_name, given_name, \
             family_name, active, user_id, invitation_id) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12) \
         RETURNING id, user_name::text AS user_name, email::text AS email, external_id, display_name, given_name, family_name, \
             active, user_id, invitation_id, version, created_at, updated_at",
    )
    .bind(id)
    .bind(caller.tenant_id)
    .bind(caller.organization_id)
    .bind(&person.user_name)
    .bind(person.address()?)
    .bind(&person.external_id)
    .bind(&person.display_name)
    .bind(&person.given_name)
    .bind(&person.family_name)
    .bind(person.active)
    .bind(link.user_id)
    .bind(link.invitation_id)
    .fetch_one(tx.conn())
    .await?;
    tx.commit().await?;
    send(&state, &caller, outgoing);
    let mut response = scim_json(StatusCode::CREATED, &row.resource(&state));
    if let Ok(value) = HeaderValue::from_str(&location(&state, id)) {
        response.headers_mut().insert(header::LOCATION, value);
    }
    Ok(response)
}

/// PUT and PATCH: compute the new person from the stored one, then reconcile.
async fn change(
    state: &AppState,
    headers: &HeaderMap,
    id: &str,
    body: &Bytes,
    next: impl FnOnce(&Person, &Value) -> Result<Person, ScimError>,
) -> Result<Response, ScimError> {
    let (mut tx, caller) = authenticate(state, headers).await?;
    let id: Uuid = id.parse().map_err(|_| ScimError::not_found())?;
    let row = load(tx.conn(), caller.tenant_id, id).await?;
    let person = next(&row.person(), &parse_body(body)?)?;
    ensure_unique(tx.conn(), caller.tenant_id, &person.user_name, Some(id)).await?;
    let link = Link { user_id: row.user_id, invitation_id: row.invitation_id };
    let (link, outgoing) = reconcile(tx.conn(), state, &caller, id, link, &person).await?;
    let row = sqlx::query_as::<_, Row>(
        "UPDATE scim_users SET user_name = $3, email = $4, external_id = $5, display_name = $6, given_name = $7, \
             family_name = $8, active = $9, user_id = $10, invitation_id = $11, version = version + 1, updated_at = now() \
         WHERE tenant_id = $1 AND id = $2 \
         RETURNING id, user_name::text AS user_name, email::text AS email, external_id, display_name, given_name, family_name, \
             active, user_id, invitation_id, version, created_at, updated_at",
    )
    .bind(caller.tenant_id)
    .bind(id)
    .bind(&person.user_name)
    .bind(person.address()?)
    .bind(&person.external_id)
    .bind(&person.display_name)
    .bind(&person.given_name)
    .bind(&person.family_name)
    .bind(person.active)
    .bind(link.user_id)
    .bind(link.invitation_id)
    .fetch_one(tx.conn())
    .await?;
    tx.commit().await?;
    send(state, &caller, outgoing);
    Ok(scim_json(StatusCode::OK, &row.resource(state)))
}

pub async fn replace_user(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<Response, ScimError> {
    change(&state, &headers, &id, &body, |_, body| Person::from_resource(body)).await
}

pub async fn patch_user(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<Response, ScimError> {
    change(&state, &headers, &id, &body, |current, body| current.patched(body)).await
}

pub async fn delete_user(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, ScimError> {
    let (mut tx, caller) = authenticate(&state, &headers).await?;
    let id: Uuid = id.parse().map_err(|_| ScimError::not_found())?;
    let row = load(tx.conn(), caller.tenant_id, id).await?;
    if let Some(user_id) = row.user_id {
        if let Some(membership) = store::membership(tx.conn(), caller.tenant_id, user_id).await? {
            let rules = MemberChangeContext {
                actor_role: caller.admin_role,
                actor_is_target: caller.admin == user_id,
                target_role: OrgRole::parse(&membership.org_role)
                    .ok_or_else(|| ApiError::internal(anyhow::anyhow!("unknown role")))?,
                target_status: MemberStatus::parse(&membership.status)
                    .ok_or_else(|| ApiError::internal(anyhow::anyhow!("unknown status")))?,
                active_owner_count: store::active_owner_count(tx.conn(), caller.tenant_id).await?,
            };
            // Removing yourself is "leaving" in the app; a token removing its own creator is not.
            if rules.actor_is_target {
                return Err(ScimError::new(
                    StatusCode::CONFLICT,
                    None,
                    "This provisioning token was created by this person and cannot remove them.",
                ));
            }
            check_member_change(rules, MemberChange::Remove)?;
            store::delete_membership(tx.conn(), caller.tenant_id, caller.organization_id, user_id).await?;
            emit_membership_changed_by(tx.conn(), caller.tenant_id, caller.actor(), user_id, "removed", None).await?;
        }
    }
    if let Some(invitation_id) = row.invitation_id {
        store::revoke_invitation(tx.conn(), caller.tenant_id, invitation_id).await?;
    }
    sqlx::query("DELETE FROM scim_users WHERE tenant_id = $1 AND id = $2")
        .bind(caller.tenant_id)
        .bind(id)
        .execute(tx.conn())
        .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_well_formed_tokens_are_looked_up() {
        assert!(plausible(&format!("{TOKEN_PREFIX}{}", "a".repeat(43))));
        assert!(!plausible(&"a".repeat(49)));
        assert!(!plausible(&format!("{TOKEN_PREFIX}{}", "a".repeat(42))));
        assert!(!plausible(&format!("{TOKEN_PREFIX}{}!", "a".repeat(42))));
    }
}
