//! SQL for profiles, organizations, memberships and invitations.
//!
//! Functions taking `&mut PgConnection` for tenant-owned tables must be called with a
//! connection from `DbRouter::begin(tenant_id)`; row-level security then guarantees
//! they only see that tenant. The explicit `tenant_id` predicates are kept as well:
//! they drive index usage and are a second, independent check.

use error_handling::ApiResult;
use sqlx::{FromRow, PgConnection, PgPool};
use time::OffsetDateTime;
use uuid::Uuid;

// --- profiles (global) -------------------------------------------------------

#[derive(Debug, Clone, FromRow)]
pub struct Profile {
    pub user_id: Uuid,
    pub email: String,
    pub display_name: String,
    pub timezone: String,
    pub locale: String,
    pub version: i64,
    pub created_at: OffsetDateTime,
}

const PROFILE_COLUMNS: &str = "user_id, email::text AS email, display_name, timezone, locale, version, created_at";

/// Create the profile on first sight of a verified token. Later edits made in this
/// service win over the token's claims (`DO NOTHING`).
pub async fn ensure_profile(pool: &PgPool, user_id: Uuid, email: &str, display_name: &str) -> ApiResult<Profile> {
    sqlx::query("INSERT INTO user_profiles (user_id, email, display_name) VALUES ($1, $2, $3) ON CONFLICT (user_id) DO NOTHING")
        .bind(user_id)
        .bind(email)
        .bind(display_name)
        .execute(pool)
        .await?;
    Ok(sqlx::query_as::<_, Profile>(&format!("SELECT {PROFILE_COLUMNS} FROM user_profiles WHERE user_id = $1"))
        .bind(user_id)
        .fetch_one(pool)
        .await?)
}

pub async fn update_profile(
    pool: &PgPool,
    user_id: Uuid,
    display_name: Option<&str>,
    timezone: Option<&str>,
    locale: Option<&str>,
) -> ApiResult<Profile> {
    Ok(sqlx::query_as::<_, Profile>(&format!(
        "UPDATE user_profiles SET display_name = COALESCE($2, display_name), timezone = COALESCE($3, timezone), \
             locale = COALESCE($4, locale), version = version + 1, updated_at = now() \
         WHERE user_id = $1 RETURNING {PROFILE_COLUMNS}"
    ))
    .bind(user_id)
    .bind(display_name)
    .bind(timezone)
    .bind(locale)
    .fetch_one(pool)
    .await?)
}

// --- directory (global) ------------------------------------------------------

pub async fn directory_for_user(pool: &PgPool, user_id: Uuid) -> ApiResult<Vec<(Uuid, Uuid)>> {
    Ok(sqlx::query_as(
        "SELECT organization_id, tenant_id FROM user_org_directory WHERE user_id = $1 ORDER BY created_at LIMIT 100",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?)
}

pub async fn directory_tenant(pool: &PgPool, user_id: Uuid, organization_id: Uuid) -> ApiResult<Option<Uuid>> {
    let row: Option<(Uuid,)> =
        sqlx::query_as("SELECT tenant_id FROM user_org_directory WHERE user_id = $1 AND organization_id = $2")
            .bind(user_id)
            .bind(organization_id)
            .fetch_optional(pool)
            .await?;
    Ok(row.map(|(t,)| t))
}

async fn directory_add(conn: &mut PgConnection, user_id: Uuid, organization_id: Uuid, tenant_id: Uuid) -> ApiResult<()> {
    sqlx::query(
        "INSERT INTO user_org_directory (user_id, organization_id, tenant_id) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING",
    )
    .bind(user_id)
    .bind(organization_id)
    .bind(tenant_id)
    .execute(conn)
    .await?;
    Ok(())
}

// --- organizations -----------------------------------------------------------

#[derive(Debug, Clone, FromRow)]
pub struct Organization {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub name: String,
    pub slug: String,
    pub version: i64,
    pub created_at: OffsetDateTime,
}

const ORG_COLUMNS: &str = "id, tenant_id, name, slug::text AS slug, version, created_at";

pub async fn insert_organization(
    conn: &mut PgConnection,
    id: Uuid,
    tenant_id: Uuid,
    name: &str,
    slug: &str,
) -> Result<Organization, sqlx::Error> {
    sqlx::query_as::<_, Organization>(&format!(
        "INSERT INTO organizations (id, tenant_id, name, slug) VALUES ($1, $2, $3, $4) RETURNING {ORG_COLUMNS}"
    ))
    .bind(id)
    .bind(tenant_id)
    .bind(name)
    .bind(slug)
    .fetch_one(conn)
    .await
}

pub async fn organization(conn: &mut PgConnection, tenant_id: Uuid) -> ApiResult<Option<Organization>> {
    Ok(sqlx::query_as::<_, Organization>(&format!("SELECT {ORG_COLUMNS} FROM organizations WHERE tenant_id = $1"))
        .bind(tenant_id)
        .fetch_optional(conn)
        .await?)
}

/// Optimistic update: succeeds only if the row is still at `expected_version`.
pub async fn rename_organization(
    conn: &mut PgConnection,
    tenant_id: Uuid,
    name: &str,
    expected_version: i64,
) -> ApiResult<Option<Organization>> {
    Ok(sqlx::query_as::<_, Organization>(&format!(
        "UPDATE organizations SET name = $2, version = version + 1, updated_at = now() \
         WHERE tenant_id = $1 AND version = $3 RETURNING {ORG_COLUMNS}"
    ))
    .bind(tenant_id)
    .bind(name)
    .bind(expected_version)
    .fetch_optional(conn)
    .await?)
}

/// Advance and return the membership aggregate version (used on membership events).
/// Also takes a row lock on the organization, serialising membership changes per tenant.
pub async fn next_membership_seq(conn: &mut PgConnection, tenant_id: Uuid) -> ApiResult<(Uuid, i64)> {
    Ok(sqlx::query_as(
        "UPDATE organizations SET membership_seq = membership_seq + 1 WHERE tenant_id = $1 RETURNING id, membership_seq",
    )
    .bind(tenant_id)
    .fetch_one(conn)
    .await?)
}

// --- memberships -------------------------------------------------------------

#[derive(Debug, Clone, FromRow)]
pub struct Membership {
    pub user_id: Uuid,
    pub org_role: String,
    pub status: String,
}

pub async fn insert_membership(
    conn: &mut PgConnection,
    tenant_id: Uuid,
    organization_id: Uuid,
    user_id: Uuid,
    role: &str,
) -> ApiResult<bool> {
    let inserted = sqlx::query(
        "INSERT INTO memberships (tenant_id, user_id, organization_id, org_role) VALUES ($1, $2, $3, $4) ON CONFLICT DO NOTHING",
    )
    .bind(tenant_id)
    .bind(user_id)
    .bind(organization_id)
    .bind(role)
    .execute(&mut *conn)
    .await?
    .rows_affected()
        == 1;
    directory_add(conn, user_id, organization_id, tenant_id).await?;
    Ok(inserted)
}

pub async fn membership(conn: &mut PgConnection, tenant_id: Uuid, user_id: Uuid) -> ApiResult<Option<Membership>> {
    Ok(sqlx::query_as::<_, Membership>("SELECT user_id, org_role, status FROM memberships WHERE tenant_id = $1 AND user_id = $2")
        .bind(tenant_id)
        .bind(user_id)
        .fetch_optional(conn)
        .await?)
}

pub async fn active_owner_count(conn: &mut PgConnection, tenant_id: Uuid) -> ApiResult<i64> {
    let (count,): (i64,) =
        sqlx::query_as("SELECT count(*) FROM memberships WHERE tenant_id = $1 AND org_role = 'owner' AND status = 'active'")
            .bind(tenant_id)
            .fetch_one(conn)
            .await?;
    Ok(count)
}

pub async fn update_membership(
    conn: &mut PgConnection,
    tenant_id: Uuid,
    user_id: Uuid,
    role: Option<&str>,
    status: Option<&str>,
) -> ApiResult<()> {
    sqlx::query(
        "UPDATE memberships SET org_role = COALESCE($3, org_role), status = COALESCE($4, status), updated_at = now() \
         WHERE tenant_id = $1 AND user_id = $2",
    )
    .bind(tenant_id)
    .bind(user_id)
    .bind(role)
    .bind(status)
    .execute(conn)
    .await?;
    Ok(())
}

pub async fn delete_membership(conn: &mut PgConnection, tenant_id: Uuid, organization_id: Uuid, user_id: Uuid) -> ApiResult<()> {
    sqlx::query("DELETE FROM memberships WHERE tenant_id = $1 AND user_id = $2")
        .bind(tenant_id)
        .bind(user_id)
        .execute(&mut *conn)
        .await?;
    sqlx::query("DELETE FROM user_org_directory WHERE user_id = $1 AND organization_id = $2")
        .bind(user_id)
        .bind(organization_id)
        .execute(conn)
        .await?;
    Ok(())
}

#[derive(Debug, Clone, FromRow)]
pub struct MemberRow {
    pub user_id: Uuid,
    pub display_name: String,
    pub email: String,
    pub org_role: String,
    pub status: String,
    pub joined_at: OffsetDateTime,
}

const MEMBER_SELECT: &str = "SELECT m.user_id, p.display_name, p.email::text AS email, m.org_role, m.status, m.joined_at \
     FROM memberships m JOIN user_profiles p ON p.user_id = m.user_id";

/// Keyset-paginated member list, oldest first. `search` matches name or email.
pub async fn list_members(
    conn: &mut PgConnection,
    tenant_id: Uuid,
    search: Option<&str>,
    after: Option<(OffsetDateTime, Uuid)>,
    limit: i64,
) -> ApiResult<Vec<MemberRow>> {
    // LIKE wildcards in user input are escaped; the pattern is still a bind parameter.
    let pattern = search.map(|s| format!("%{}%", s.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")));
    let (after_at, after_id) = after.unzip();
    Ok(sqlx::query_as::<_, MemberRow>(&format!(
        "{MEMBER_SELECT} WHERE m.tenant_id = $1 \
           AND ($2::text IS NULL OR p.display_name ILIKE $2 OR p.email::text ILIKE $2) \
           AND ($3::timestamptz IS NULL OR (m.joined_at, m.user_id) > ($3, $4)) \
         ORDER BY m.joined_at, m.user_id LIMIT $5"
    ))
    .bind(tenant_id)
    .bind(pattern)
    .bind(after_at)
    .bind(after_id)
    .bind(limit + 1)
    .fetch_all(conn)
    .await?)
}

pub async fn member(conn: &mut PgConnection, tenant_id: Uuid, user_id: Uuid) -> ApiResult<Option<MemberRow>> {
    Ok(sqlx::query_as::<_, MemberRow>(&format!("{MEMBER_SELECT} WHERE m.tenant_id = $1 AND m.user_id = $2"))
        .bind(tenant_id)
        .bind(user_id)
        .fetch_optional(conn)
        .await?)
}

pub async fn is_member_email(conn: &mut PgConnection, tenant_id: Uuid, email: &str) -> ApiResult<bool> {
    let (exists,): (bool,) = sqlx::query_as(
        "SELECT EXISTS (SELECT 1 FROM memberships m JOIN user_profiles p ON p.user_id = m.user_id \
                        WHERE m.tenant_id = $1 AND p.email = $2)",
    )
    .bind(tenant_id)
    .bind(email)
    .fetch_one(conn)
    .await?;
    Ok(exists)
}

// --- invitations -------------------------------------------------------------

#[derive(Debug, Clone, FromRow)]
pub struct Invitation {
    pub id: Uuid,
    pub organization_id: Uuid,
    pub email: String,
    pub org_role: String,
    pub invited_by: Uuid,
    pub invited_by_name: Option<String>,
    pub created_at: OffsetDateTime,
    pub expires_at: OffsetDateTime,
    pub accepted_at: Option<OffsetDateTime>,
    pub revoked_at: Option<OffsetDateTime>,
}

impl Invitation {
    pub fn state(&self) -> &'static str {
        if self.accepted_at.is_some() {
            "accepted"
        } else if self.revoked_at.is_some() {
            "revoked"
        } else if self.expires_at <= OffsetDateTime::now_utc() {
            "expired"
        } else {
            "pending"
        }
    }
}

const INVITATION_SELECT: &str = "SELECT i.id, i.organization_id, i.email::text AS email, i.org_role, i.invited_by, \
        p.display_name AS invited_by_name, i.created_at, i.expires_at, i.accepted_at, i.revoked_at \
     FROM invitations i LEFT JOIN user_profiles p ON p.user_id = i.invited_by";

#[allow(clippy::too_many_arguments)]
pub async fn insert_invitation(
    conn: &mut PgConnection,
    id: Uuid,
    tenant_id: Uuid,
    organization_id: Uuid,
    email: &str,
    role: &str,
    invited_by: Uuid,
    ttl_days: i32,
    token_hash: &[u8],
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO invitations (id, tenant_id, organization_id, email, org_role, invited_by, expires_at) \
         VALUES ($1, $2, $3, $4, $5, $6, now() + make_interval(days => $7))",
    )
    .bind(id)
    .bind(tenant_id)
    .bind(organization_id)
    .bind(email)
    .bind(role)
    .bind(invited_by)
    .bind(ttl_days)
    .execute(&mut *conn)
    .await?;
    sqlx::query("INSERT INTO invitation_tokens (token_hash, tenant_id, invitation_id) VALUES ($1, $2, $3)")
        .bind(token_hash)
        .bind(tenant_id)
        .bind(id)
        .execute(conn)
        .await?;
    Ok(())
}

/// Expired, never-accepted invitations no longer block a fresh invite to the same address.
pub async fn revoke_expired_invitations(conn: &mut PgConnection, tenant_id: Uuid, email: &str) -> ApiResult<()> {
    sqlx::query(
        "UPDATE invitations SET revoked_at = now() \
         WHERE tenant_id = $1 AND email = $2 AND accepted_at IS NULL AND revoked_at IS NULL AND expires_at <= now()",
    )
    .bind(tenant_id)
    .bind(email)
    .execute(conn)
    .await?;
    Ok(())
}

pub async fn invitation(conn: &mut PgConnection, tenant_id: Uuid, id: Uuid, lock: bool) -> ApiResult<Option<Invitation>> {
    let suffix = if lock { " FOR UPDATE OF i" } else { "" };
    Ok(sqlx::query_as::<_, Invitation>(&format!("{INVITATION_SELECT} WHERE i.tenant_id = $1 AND i.id = $2{suffix}"))
        .bind(tenant_id)
        .bind(id)
        .fetch_optional(conn)
        .await?)
}

pub async fn open_invitations(conn: &mut PgConnection, tenant_id: Uuid) -> ApiResult<Vec<Invitation>> {
    Ok(sqlx::query_as::<_, Invitation>(&format!(
        "{INVITATION_SELECT} WHERE i.tenant_id = $1 AND i.accepted_at IS NULL AND i.revoked_at IS NULL \
         ORDER BY i.created_at DESC LIMIT 200"
    ))
    .bind(tenant_id)
    .fetch_all(conn)
    .await?)
}

pub async fn revoke_invitation(conn: &mut PgConnection, tenant_id: Uuid, id: Uuid) -> ApiResult<bool> {
    let done = sqlx::query(
        "UPDATE invitations SET revoked_at = now() WHERE tenant_id = $1 AND id = $2 AND accepted_at IS NULL AND revoked_at IS NULL",
    )
    .bind(tenant_id)
    .bind(id)
    .execute(&mut *conn)
    .await?
    .rows_affected()
        == 1;
    if done {
        sqlx::query("DELETE FROM invitation_tokens WHERE invitation_id = $1").bind(id).execute(conn).await?;
    }
    Ok(done)
}

pub async fn mark_invitation_accepted(conn: &mut PgConnection, tenant_id: Uuid, id: Uuid, user_id: Uuid) -> ApiResult<()> {
    sqlx::query("UPDATE invitations SET accepted_at = now(), accepted_by = $3 WHERE tenant_id = $1 AND id = $2")
        .bind(tenant_id)
        .bind(id)
        .bind(user_id)
        .execute(&mut *conn)
        .await?;
    // Single use: the token stops resolving once accepted.
    sqlx::query("DELETE FROM invitation_tokens WHERE invitation_id = $1").bind(id).execute(conn).await?;
    Ok(())
}

/// Where an invitation token points. Global lookup by hash only.
pub async fn invitation_location(pool: &PgPool, token_hash: &[u8]) -> ApiResult<Option<(Uuid, Uuid)>> {
    Ok(sqlx::query_as("SELECT tenant_id, invitation_id FROM invitation_tokens WHERE token_hash = $1")
        .bind(token_hash)
        .fetch_optional(pool)
        .await?)
}
