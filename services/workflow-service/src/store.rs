//! SQL for workflows, versions and assignments. Always inside a tenant-scoped transaction.

use error_handling::ApiResult;
use serde_json::Value;
use sqlx::{FromRow, PgConnection};
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Debug, Clone, FromRow)]
pub struct Workflow {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub draft: Option<Value>,
    pub published_version: Option<i32>,
    pub version: i64,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

const WORKFLOW: &str = "id, name, description, draft, published_version, version, created_at, updated_at";

pub async fn list(conn: &mut PgConnection, tenant_id: Uuid) -> ApiResult<Vec<Workflow>> {
    Ok(sqlx::query_as(&format!(
        "SELECT {WORKFLOW} FROM workflows WHERE tenant_id = $1 AND deleted_at IS NULL ORDER BY lower(name) LIMIT 500"
    ))
    .bind(tenant_id)
    .fetch_all(conn)
    .await?)
}

pub async fn get(conn: &mut PgConnection, tenant_id: Uuid, id: Uuid, lock: bool) -> ApiResult<Option<Workflow>> {
    let suffix = if lock { " FOR UPDATE" } else { "" };
    Ok(sqlx::query_as(&format!(
        "SELECT {WORKFLOW} FROM workflows WHERE tenant_id = $1 AND id = $2 AND deleted_at IS NULL{suffix}"
    ))
    .bind(tenant_id)
    .bind(id)
    .fetch_optional(conn)
    .await?)
}

pub async fn insert(
    conn: &mut PgConnection,
    tenant_id: Uuid,
    id: Uuid,
    name: &str,
    description: &str,
    draft: &Value,
    by: Uuid,
) -> Result<Workflow, sqlx::Error> {
    sqlx::query_as(&format!(
        "INSERT INTO workflows (id, tenant_id, name, description, draft, created_by) VALUES ($1, $2, $3, $4, $5, $6) RETURNING {WORKFLOW}"
    ))
    .bind(id)
    .bind(tenant_id)
    .bind(name)
    .bind(description)
    .bind(draft)
    .bind(by)
    .fetch_one(conn)
    .await
}

pub async fn rename(
    conn: &mut PgConnection,
    tenant_id: Uuid,
    id: Uuid,
    name: &str,
    description: &str,
) -> Result<Option<Workflow>, sqlx::Error> {
    sqlx::query_as(&format!(
        "UPDATE workflows SET name = $3, description = $4, version = version + 1, updated_at = now() \
         WHERE tenant_id = $1 AND id = $2 AND deleted_at IS NULL RETURNING {WORKFLOW}"
    ))
    .bind(tenant_id)
    .bind(id)
    .bind(name)
    .bind(description)
    .fetch_optional(conn)
    .await
}

/// Save (or with `None`, discard) the draft if the row is still at `expected` version.
pub async fn set_draft(
    conn: &mut PgConnection,
    tenant_id: Uuid,
    id: Uuid,
    expected: i64,
    draft: Option<&Value>,
) -> ApiResult<Option<Workflow>> {
    Ok(sqlx::query_as(&format!(
        "UPDATE workflows SET draft = $4, version = version + 1, updated_at = now() \
         WHERE tenant_id = $1 AND id = $2 AND version = $3 AND deleted_at IS NULL RETURNING {WORKFLOW}"
    ))
    .bind(tenant_id)
    .bind(id)
    .bind(expected)
    .bind(draft)
    .fetch_optional(conn)
    .await?)
}

/// Turn `definition` into the next immutable version and clear the draft. The caller
/// holds the row lock on the workflow.
pub async fn publish(
    conn: &mut PgConnection,
    tenant_id: Uuid,
    workflow: &Workflow,
    definition: &Value,
    by: Uuid,
) -> ApiResult<Workflow> {
    let next = workflow.published_version.unwrap_or(0) + 1;
    sqlx::query(
        "INSERT INTO workflow_versions (tenant_id, workflow_id, version, definition, published_by) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(tenant_id)
    .bind(workflow.id)
    .bind(next)
    .bind(definition)
    .bind(by)
    .execute(&mut *conn)
    .await?;
    Ok(sqlx::query_as(&format!(
        "UPDATE workflows SET draft = NULL, published_version = $3, version = version + 1, updated_at = now() \
         WHERE tenant_id = $1 AND id = $2 RETURNING {WORKFLOW}"
    ))
    .bind(tenant_id)
    .bind(workflow.id)
    .bind(next)
    .fetch_one(conn)
    .await?)
}

pub async fn soft_delete(conn: &mut PgConnection, tenant_id: Uuid, id: Uuid) -> ApiResult<()> {
    sqlx::query("UPDATE workflows SET deleted_at = now() WHERE tenant_id = $1 AND id = $2")
        .bind(tenant_id)
        .bind(id)
        .execute(conn)
        .await?;
    Ok(())
}

#[derive(Debug, Clone, FromRow)]
pub struct VersionRow {
    pub version: i32,
    pub definition: Value,
    pub published_by: Uuid,
    pub published_at: OffsetDateTime,
}

pub async fn version(conn: &mut PgConnection, tenant_id: Uuid, workflow_id: Uuid, version: i32) -> ApiResult<Option<VersionRow>> {
    Ok(sqlx::query_as(
        "SELECT version, definition, published_by, published_at FROM workflow_versions WHERE tenant_id = $1 AND workflow_id = $2 AND version = $3",
    )
    .bind(tenant_id)
    .bind(workflow_id)
    .bind(version)
    .fetch_optional(conn)
    .await?)
}

pub async fn versions(conn: &mut PgConnection, tenant_id: Uuid, workflow_id: Uuid) -> ApiResult<Vec<VersionRow>> {
    Ok(sqlx::query_as(
        "SELECT version, definition, published_by, published_at FROM workflow_versions \
         WHERE tenant_id = $1 AND workflow_id = $2 ORDER BY version DESC LIMIT 100",
    )
    .bind(tenant_id)
    .bind(workflow_id)
    .fetch_all(conn)
    .await?)
}

#[derive(Debug, Clone, FromRow)]
pub struct Assignment {
    pub id: Uuid,
    pub project_id: Option<Uuid>,
    pub issue_type: Option<String>,
    pub workflow_id: Uuid,
    pub workflow_name: String,
}

const ASSIGNMENT: &str = "a.id, a.project_id, a.issue_type, a.workflow_id, w.name AS workflow_name \
     FROM workflow_assignments a JOIN workflows w ON w.id = a.workflow_id";

/// Assignments for one project (`Some`) or the tenant-wide defaults (`None`).
pub async fn assignments(conn: &mut PgConnection, tenant_id: Uuid, project_id: Option<Uuid>) -> ApiResult<Vec<Assignment>> {
    Ok(sqlx::query_as(&format!(
        "SELECT {ASSIGNMENT} WHERE a.tenant_id = $1 AND a.project_id IS NOT DISTINCT FROM $2 ORDER BY a.issue_type NULLS FIRST"
    ))
    .bind(tenant_id)
    .bind(project_id)
    .fetch_all(conn)
    .await?)
}

pub async fn assignment_count(conn: &mut PgConnection, tenant_id: Uuid, workflow_id: Uuid) -> ApiResult<i64> {
    let (count,): (i64,) = sqlx::query_as("SELECT count(*) FROM workflow_assignments WHERE tenant_id = $1 AND workflow_id = $2")
        .bind(tenant_id)
        .bind(workflow_id)
        .fetch_one(conn)
        .await?;
    Ok(count)
}

pub async fn assign(
    conn: &mut PgConnection,
    tenant_id: Uuid,
    project_id: Option<Uuid>,
    issue_type: Option<&str>,
    workflow_id: Uuid,
    by: Uuid,
) -> ApiResult<()> {
    sqlx::query("DELETE FROM workflow_assignments WHERE tenant_id = $1 AND project_id IS NOT DISTINCT FROM $2 AND issue_type IS NOT DISTINCT FROM $3")
        .bind(tenant_id)
        .bind(project_id)
        .bind(issue_type)
        .execute(&mut *conn)
        .await?;
    sqlx::query("INSERT INTO workflow_assignments (id, tenant_id, project_id, issue_type, workflow_id, created_by) VALUES ($1, $2, $3, $4, $5, $6)")
        .bind(platform_common::new_id())
        .bind(tenant_id)
        .bind(project_id)
        .bind(issue_type)
        .bind(workflow_id)
        .bind(by)
        .execute(conn)
        .await?;
    Ok(())
}

pub async fn unassign(
    conn: &mut PgConnection,
    tenant_id: Uuid,
    project_id: Option<Uuid>,
    issue_type: Option<&str>,
) -> ApiResult<bool> {
    Ok(sqlx::query("DELETE FROM workflow_assignments WHERE tenant_id = $1 AND project_id IS NOT DISTINCT FROM $2 AND issue_type IS NOT DISTINCT FROM $3")
        .bind(tenant_id)
        .bind(project_id)
        .bind(issue_type)
        .execute(conn)
        .await?
        .rows_affected()
        > 0)
}

/// The published definition governing `(project, issue type)`, most specific assignment
/// first. `None` means the built-in default applies.
pub async fn resolve(
    conn: &mut PgConnection,
    tenant_id: Uuid,
    project_id: Uuid,
    issue_type: &str,
) -> ApiResult<Option<(Uuid, String, i32, Value)>> {
    Ok(sqlx::query_as(
        "SELECT w.id, w.name, v.version, v.definition \
         FROM workflow_assignments a \
         JOIN workflows w ON w.id = a.workflow_id AND w.deleted_at IS NULL \
         JOIN workflow_versions v ON v.workflow_id = w.id AND v.version = w.published_version \
         WHERE a.tenant_id = $1 AND (a.project_id = $2 OR a.project_id IS NULL) AND (a.issue_type = $3 OR a.issue_type IS NULL) \
         ORDER BY (a.project_id IS NOT NULL) DESC, (a.issue_type IS NOT NULL) DESC LIMIT 1",
    )
    .bind(tenant_id)
    .bind(project_id)
    .bind(issue_type)
    .fetch_optional(conn)
    .await?)
}

/// Every published definition that can apply to issues of a project (any issue type).
pub async fn definitions_for_project(
    conn: &mut PgConnection,
    tenant_id: Uuid,
    project_id: Uuid,
) -> ApiResult<Vec<(Option<String>, bool, Value)>> {
    Ok(sqlx::query_as(
        "SELECT a.issue_type, (a.project_id IS NOT NULL), v.definition \
         FROM workflow_assignments a \
         JOIN workflows w ON w.id = a.workflow_id AND w.deleted_at IS NULL \
         JOIN workflow_versions v ON v.workflow_id = w.id AND v.version = w.published_version \
         WHERE a.tenant_id = $1 AND (a.project_id = $2 OR a.project_id IS NULL) \
         ORDER BY (a.project_id IS NOT NULL) DESC, (a.issue_type IS NOT NULL) DESC",
    )
    .bind(tenant_id)
    .bind(project_id)
    .fetch_all(conn)
    .await?)
}
