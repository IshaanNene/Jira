//! SQL for role bindings. Every function runs inside a tenant-scoped transaction.

use error_handling::ApiResult;
use event_contracts::{outbox, payloads::PermissionUpdatedV1, Actor, EventEnvelope};
use sqlx::PgConnection;
use uuid::Uuid;

/// Advance the tenant's policy version and record the change event. Call once per
/// mutating transaction, after the change itself.
pub async fn bump_version(conn: &mut PgConnection, tenant_id: Uuid, reason: &str) -> ApiResult<i64> {
    let (version,): (i64,) = sqlx::query_as(
        "INSERT INTO policy_versions (tenant_id, version) VALUES ($1, 1) \
         ON CONFLICT (tenant_id) DO UPDATE SET version = policy_versions.version + 1 RETURNING version",
    )
    .bind(tenant_id)
    .fetch_one(&mut *conn)
    .await?;
    outbox::enqueue(
        conn,
        &EventEnvelope::new(
            Some(tenant_id),
            tenant_id,
            version,
            Actor::System("permission-service".into()),
            PermissionUpdatedV1 { version, reason: reason.to_owned() },
        ),
    )
    .await?;
    Ok(version)
}

pub async fn version(conn: &mut PgConnection, tenant_id: Uuid) -> ApiResult<i64> {
    let row: Option<(i64,)> =
        sqlx::query_as("SELECT version FROM policy_versions WHERE tenant_id = $1").bind(tenant_id).fetch_optional(conn).await?;
    Ok(row.map_or(0, |(v,)| v))
}

pub async fn roles_for_user(conn: &mut PgConnection, tenant_id: Uuid, user_id: Uuid) -> ApiResult<Vec<(Uuid, String)>> {
    Ok(sqlx::query_as("SELECT project_id, role FROM project_role_bindings WHERE tenant_id = $1 AND user_id = $2")
        .bind(tenant_id)
        .bind(user_id)
        .fetch_all(conn)
        .await?)
}

pub async fn default_roles(conn: &mut PgConnection, tenant_id: Uuid) -> ApiResult<Vec<(Uuid, String)>> {
    Ok(sqlx::query_as("SELECT project_id, role FROM project_default_roles WHERE tenant_id = $1")
        .bind(tenant_id)
        .fetch_all(conn)
        .await?)
}

/// Returns whether anything changed (so unchanged writes do not churn the version).
pub async fn set_role(conn: &mut PgConnection, tenant_id: Uuid, project_id: Uuid, user_id: Uuid, role: &str) -> ApiResult<bool> {
    let changed = sqlx::query(
        "INSERT INTO project_role_bindings (tenant_id, project_id, user_id, role) VALUES ($1, $2, $3, $4) \
         ON CONFLICT (tenant_id, project_id, user_id) DO UPDATE SET role = EXCLUDED.role, updated_at = now() \
         WHERE project_role_bindings.role <> EXCLUDED.role",
    )
    .bind(tenant_id)
    .bind(project_id)
    .bind(user_id)
    .bind(role)
    .execute(conn)
    .await?;
    Ok(changed.rows_affected() > 0)
}

pub async fn remove_role(conn: &mut PgConnection, tenant_id: Uuid, project_id: Uuid, user_id: Uuid) -> ApiResult<bool> {
    let done = sqlx::query("DELETE FROM project_role_bindings WHERE tenant_id = $1 AND project_id = $2 AND user_id = $3")
        .bind(tenant_id)
        .bind(project_id)
        .bind(user_id)
        .execute(conn)
        .await?;
    Ok(done.rows_affected() > 0)
}

pub async fn set_default_role(conn: &mut PgConnection, tenant_id: Uuid, project_id: Uuid, role: Option<&str>) -> ApiResult<bool> {
    let done = match role {
        Some(role) => {
            sqlx::query(
                "INSERT INTO project_default_roles (tenant_id, project_id, role) VALUES ($1, $2, $3) \
                 ON CONFLICT (tenant_id, project_id) DO UPDATE SET role = EXCLUDED.role \
                 WHERE project_default_roles.role <> EXCLUDED.role",
            )
            .bind(tenant_id)
            .bind(project_id)
            .bind(role)
            .execute(conn)
            .await?
        }
        None => {
            sqlx::query("DELETE FROM project_default_roles WHERE tenant_id = $1 AND project_id = $2")
                .bind(tenant_id)
                .bind(project_id)
                .execute(conn)
                .await?
        }
    };
    Ok(done.rows_affected() > 0)
}

pub async fn members(
    conn: &mut PgConnection,
    tenant_id: Uuid,
    project_id: Uuid,
) -> ApiResult<(Vec<(Uuid, String)>, Option<String>)> {
    let members = sqlx::query_as(
        "SELECT user_id, role FROM project_role_bindings WHERE tenant_id = $1 AND project_id = $2 ORDER BY created_at, user_id LIMIT 1000",
    )
    .bind(tenant_id)
    .bind(project_id)
    .fetch_all(&mut *conn)
    .await?;
    let default: Option<(String,)> =
        sqlx::query_as("SELECT role FROM project_default_roles WHERE tenant_id = $1 AND project_id = $2")
            .bind(tenant_id)
            .bind(project_id)
            .fetch_optional(conn)
            .await?;
    Ok((members, default.map(|(r,)| r)))
}

pub async fn delete_project(conn: &mut PgConnection, tenant_id: Uuid, project_id: Uuid) -> ApiResult<bool> {
    let a = sqlx::query("DELETE FROM project_role_bindings WHERE tenant_id = $1 AND project_id = $2")
        .bind(tenant_id)
        .bind(project_id)
        .execute(&mut *conn)
        .await?;
    let b = sqlx::query("DELETE FROM project_default_roles WHERE tenant_id = $1 AND project_id = $2")
        .bind(tenant_id)
        .bind(project_id)
        .execute(conn)
        .await?;
    Ok(a.rows_affected() + b.rows_affected() > 0)
}
