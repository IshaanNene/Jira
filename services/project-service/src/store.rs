//! SQL for projects, components and versions. Always inside a tenant-scoped transaction.

use error_handling::ApiResult;
use sqlx::{FromRow, PgConnection};
use time::{Date, OffsetDateTime};
use uuid::Uuid;

#[derive(Debug, Clone, FromRow)]
pub struct Project {
    pub id: Uuid,
    pub key: String,
    pub name: String,
    pub description: String,
    pub project_type: String,
    pub lead_user_id: Option<Uuid>,
    pub status: String,
    pub default_role: Option<String>,
    pub version: i64,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

const PROJECT: &str =
    "id, key, name, description, project_type, lead_user_id, status, default_role, version, created_at, updated_at";

pub struct NewProject<'a> {
    pub id: Uuid,
    pub key: &'a str,
    pub name: &'a str,
    pub description: &'a str,
    pub project_type: &'a str,
    pub default_role: Option<&'a str>,
    pub created_by: Uuid,
}

pub async fn insert_project(conn: &mut PgConnection, tenant_id: Uuid, p: &NewProject<'_>) -> Result<Project, sqlx::Error> {
    sqlx::query_as::<_, Project>(&format!(
        "INSERT INTO projects (id, tenant_id, key, name, description, project_type, lead_user_id, default_role, created_by) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $7) RETURNING {PROJECT}"
    ))
    .bind(p.id)
    .bind(tenant_id)
    .bind(p.key)
    .bind(p.name)
    .bind(p.description)
    .bind(p.project_type)
    .bind(p.created_by)
    .bind(p.default_role)
    .fetch_one(conn)
    .await
}

pub async fn project_by_key(conn: &mut PgConnection, tenant_id: Uuid, key: &str) -> ApiResult<Option<Project>> {
    Ok(sqlx::query_as::<_, Project>(&format!(
        "SELECT {PROJECT} FROM projects WHERE tenant_id = $1 AND key = $2 AND deleted_at IS NULL"
    ))
    .bind(tenant_id)
    .bind(key)
    .fetch_optional(conn)
    .await?)
}

/// `visible = None` means every project in the tenant (organization admins).
pub async fn list_projects(
    conn: &mut PgConnection,
    tenant_id: Uuid,
    status: &str,
    visible: Option<&[Uuid]>,
) -> ApiResult<Vec<Project>> {
    Ok(sqlx::query_as::<_, Project>(&format!(
        "SELECT {PROJECT} FROM projects \
         WHERE tenant_id = $1 AND status = $2 AND deleted_at IS NULL AND ($3::uuid[] IS NULL OR id = ANY($3)) \
         ORDER BY lower(name) LIMIT 500"
    ))
    .bind(tenant_id)
    .bind(status)
    .bind(visible)
    .fetch_all(conn)
    .await?)
}

pub struct ProjectPatch<'a> {
    pub name: Option<&'a str>,
    pub description: Option<&'a str>,
    pub project_type: Option<&'a str>,
    /// `Some(None)` clears the lead.
    pub lead_user_id: Option<Option<Uuid>>,
}

pub async fn update_project(
    conn: &mut PgConnection,
    tenant_id: Uuid,
    id: Uuid,
    expected: i64,
    p: &ProjectPatch<'_>,
) -> ApiResult<Option<Project>> {
    Ok(sqlx::query_as::<_, Project>(&format!(
        "UPDATE projects SET name = COALESCE($4, name), description = COALESCE($5, description), \
             project_type = COALESCE($6, project_type), \
             lead_user_id = CASE WHEN $7 THEN $8 ELSE lead_user_id END, \
             version = version + 1, updated_at = now() \
         WHERE tenant_id = $1 AND id = $2 AND version = $3 AND deleted_at IS NULL RETURNING {PROJECT}"
    ))
    .bind(tenant_id)
    .bind(id)
    .bind(expected)
    .bind(p.name)
    .bind(p.description)
    .bind(p.project_type)
    .bind(p.lead_user_id.is_some())
    .bind(p.lead_user_id.flatten())
    .fetch_optional(conn)
    .await?)
}

pub async fn set_status(conn: &mut PgConnection, tenant_id: Uuid, id: Uuid, status: &str) -> ApiResult<Project> {
    Ok(sqlx::query_as::<_, Project>(&format!(
        "UPDATE projects SET status = $3, version = version + 1, updated_at = now() WHERE tenant_id = $1 AND id = $2 RETURNING {PROJECT}"
    ))
    .bind(tenant_id)
    .bind(id)
    .bind(status)
    .fetch_one(conn)
    .await?)
}

pub async fn set_default_role(conn: &mut PgConnection, tenant_id: Uuid, id: Uuid, role: Option<&str>) -> ApiResult<Project> {
    Ok(sqlx::query_as::<_, Project>(&format!(
        "UPDATE projects SET default_role = $3, version = version + 1, updated_at = now() WHERE tenant_id = $1 AND id = $2 RETURNING {PROJECT}"
    ))
    .bind(tenant_id)
    .bind(id)
    .bind(role)
    .fetch_one(conn)
    .await?)
}

pub async fn soft_delete(conn: &mut PgConnection, tenant_id: Uuid, id: Uuid) -> ApiResult<()> {
    // A deleted project keeps its key reserved: old issue keys must never point at a
    // different project.
    sqlx::query("UPDATE projects SET deleted_at = now(), updated_at = now() WHERE tenant_id = $1 AND id = $2")
        .bind(tenant_id)
        .bind(id)
        .execute(conn)
        .await?;
    Ok(())
}

#[derive(Debug, Clone, FromRow)]
pub struct Component {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub lead_user_id: Option<Uuid>,
}

pub async fn components(conn: &mut PgConnection, tenant_id: Uuid, project_id: Uuid) -> ApiResult<Vec<Component>> {
    Ok(sqlx::query_as("SELECT id, name, description, lead_user_id FROM components WHERE tenant_id = $1 AND project_id = $2 ORDER BY lower(name)")
        .bind(tenant_id)
        .bind(project_id)
        .fetch_all(conn)
        .await?)
}

pub async fn insert_component(
    conn: &mut PgConnection,
    tenant_id: Uuid,
    project_id: Uuid,
    name: &str,
    description: &str,
    lead: Option<Uuid>,
) -> Result<Component, sqlx::Error> {
    sqlx::query_as(
        "INSERT INTO components (id, tenant_id, project_id, name, description, lead_user_id) VALUES ($1, $2, $3, $4, $5, $6) \
         RETURNING id, name, description, lead_user_id",
    )
    .bind(platform_common::new_id())
    .bind(tenant_id)
    .bind(project_id)
    .bind(name)
    .bind(description)
    .bind(lead)
    .fetch_one(conn)
    .await
}

pub async fn delete_component(conn: &mut PgConnection, tenant_id: Uuid, project_id: Uuid, id: Uuid) -> ApiResult<bool> {
    Ok(sqlx::query("DELETE FROM components WHERE tenant_id = $1 AND project_id = $2 AND id = $3")
        .bind(tenant_id)
        .bind(project_id)
        .bind(id)
        .execute(conn)
        .await?
        .rows_affected()
        == 1)
}

#[derive(Debug, Clone, FromRow)]
pub struct Version {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub start_date: Option<Date>,
    pub release_date: Option<Date>,
    pub status: String,
    pub released_at: Option<OffsetDateTime>,
}

const VERSION: &str = "id, name, description, start_date, release_date, status, released_at";

pub async fn versions(conn: &mut PgConnection, tenant_id: Uuid, project_id: Uuid) -> ApiResult<Vec<Version>> {
    Ok(sqlx::query_as(&format!(
        "SELECT {VERSION} FROM versions WHERE tenant_id = $1 AND project_id = $2 ORDER BY (status = 'released'), release_date NULLS LAST, created_at"
    ))
    .bind(tenant_id)
    .bind(project_id)
    .fetch_all(conn)
    .await?)
}

pub async fn insert_version(
    conn: &mut PgConnection,
    tenant_id: Uuid,
    project_id: Uuid,
    name: &str,
    description: &str,
    start_date: Option<Date>,
    release_date: Option<Date>,
) -> Result<Version, sqlx::Error> {
    sqlx::query_as(&format!(
        "INSERT INTO versions (id, tenant_id, project_id, name, description, start_date, release_date) \
         VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING {VERSION}"
    ))
    .bind(platform_common::new_id())
    .bind(tenant_id)
    .bind(project_id)
    .bind(name)
    .bind(description)
    .bind(start_date)
    .bind(release_date)
    .fetch_one(conn)
    .await
}

/// Release or un-release. Returns `None` when the version does not exist in this project.
pub async fn set_version_released(
    conn: &mut PgConnection,
    tenant_id: Uuid,
    project_id: Uuid,
    id: Uuid,
    released: bool,
) -> ApiResult<Option<Version>> {
    Ok(sqlx::query_as(&format!(
        "UPDATE versions SET status = CASE WHEN $4 THEN 'released' ELSE 'unreleased' END, \
             released_at = CASE WHEN $4 THEN now() END, \
             release_date = CASE WHEN $4 THEN COALESCE(release_date, current_date) ELSE release_date END \
         WHERE tenant_id = $1 AND project_id = $2 AND id = $3 RETURNING {VERSION}"
    ))
    .bind(tenant_id)
    .bind(project_id)
    .bind(id)
    .bind(released)
    .fetch_optional(conn)
    .await?)
}

pub async fn delete_version(conn: &mut PgConnection, tenant_id: Uuid, project_id: Uuid, id: Uuid) -> ApiResult<bool> {
    Ok(sqlx::query("DELETE FROM versions WHERE tenant_id = $1 AND project_id = $2 AND id = $3")
        .bind(tenant_id)
        .bind(project_id)
        .bind(id)
        .execute(conn)
        .await?
        .rows_affected()
        == 1)
}
