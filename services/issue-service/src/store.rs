//! SQL for issues and their satellites. Always inside a tenant-scoped transaction.

use error_handling::ApiResult;
use serde_json::Value;
use sqlx::{FromRow, PgConnection, Postgres, QueryBuilder};
use time::{Date, OffsetDateTime};
use uuid::Uuid;

#[derive(Debug, Clone, FromRow)]
pub struct Issue {
    pub id: Uuid,
    pub project_id: Uuid,
    pub project_key: String,
    pub number: i64,
    pub type_key: String,
    pub status: String,
    pub status_category: String,
    pub priority: String,
    pub summary: String,
    pub description: Option<Value>,
    pub assignee_id: Option<Uuid>,
    pub reporter_id: Uuid,
    pub parent_id: Option<Uuid>,
    pub labels: Vec<String>,
    pub component_ids: Vec<Uuid>,
    pub fix_version_ids: Vec<Uuid>,
    pub custom_fields: Value,
    pub story_points: Option<f64>,
    pub start_date: Option<Date>,
    pub due_date: Option<Date>,
    pub resolution: Option<String>,
    pub resolved_at: Option<OffsetDateTime>,
    pub version: i64,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
    pub parent_project_key: Option<String>,
    pub parent_number: Option<i64>,
    pub parent_summary: Option<String>,
    pub parent_type: Option<String>,
}

impl Issue {
    pub fn key(&self) -> String {
        format!("{}-{}", self.project_key, self.number)
    }
}

const ISSUE: &str = "i.id, i.project_id, i.project_key, i.number, i.type_key, i.status, i.status_category, i.priority, i.summary, i.description, \
    i.assignee_id, i.reporter_id, i.parent_id, i.labels, i.component_ids, i.fix_version_ids, i.custom_fields, i.story_points::float8 AS story_points, i.start_date, i.due_date, \
    i.resolution, i.resolved_at, i.version, i.created_at, i.updated_at, \
    p.project_key AS parent_project_key, p.number AS parent_number, p.summary AS parent_summary, p.type_key AS parent_type \
    FROM issues i LEFT JOIN issues p ON p.id = i.parent_id AND p.deleted_at IS NULL";

/// Take the next issue number for a project. The upsert locks the counter row until the
/// surrounding transaction ends, so concurrent creators queue up and a rollback returns
/// the number: no duplicates, no gaps.
pub async fn allocate_number(conn: &mut PgConnection, tenant_id: Uuid, project_id: Uuid) -> ApiResult<i64> {
    let (number,): (i64,) = sqlx::query_as(
        "INSERT INTO project_issue_counters (tenant_id, project_id, next_number) VALUES ($1, $2, 2) \
         ON CONFLICT (tenant_id, project_id) DO UPDATE SET next_number = project_issue_counters.next_number + 1 \
         RETURNING next_number - 1",
    )
    .bind(tenant_id)
    .bind(project_id)
    .fetch_one(conn)
    .await?;
    Ok(number)
}

pub struct NewIssue<'a> {
    pub id: Uuid,
    pub project_id: Uuid,
    pub project_key: &'a str,
    pub number: i64,
    pub type_key: &'a str,
    pub status: &'a str,
    pub status_category: &'a str,
    pub priority: &'a str,
    pub summary: &'a str,
    pub description: Option<&'a Value>,
    pub assignee_id: Option<Uuid>,
    pub reporter_id: Uuid,
    pub parent_id: Option<Uuid>,
    pub labels: &'a [String],
    pub component_ids: &'a [Uuid],
    pub fix_version_ids: &'a [Uuid],
    pub custom_fields: &'a Value,
    pub story_points: Option<f64>,
    pub start_date: Option<Date>,
    pub due_date: Option<Date>,
}

pub async fn insert_issue(conn: &mut PgConnection, tenant_id: Uuid, n: &NewIssue<'_>) -> ApiResult<()> {
    sqlx::query(
        "INSERT INTO issues (id, tenant_id, project_id, project_key, number, type_key, status, priority, summary, description, \
                             assignee_id, reporter_id, creator_id, parent_id, labels, story_points, start_date, due_date, \
                             component_ids, fix_version_ids, custom_fields, status_category) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $12, $13, $14, $15::float8::numeric(6,1), $16, $17, $18, $19, $20, $21)",
    )
    .bind(n.id)
    .bind(tenant_id)
    .bind(n.project_id)
    .bind(n.project_key)
    .bind(n.number)
    .bind(n.type_key)
    .bind(n.status)
    .bind(n.priority)
    .bind(n.summary)
    .bind(n.description)
    .bind(n.assignee_id)
    .bind(n.reporter_id)
    .bind(n.parent_id)
    .bind(n.labels)
    .bind(n.story_points)
    .bind(n.start_date)
    .bind(n.due_date)
    .bind(n.component_ids)
    .bind(n.fix_version_ids)
    .bind(n.custom_fields)
    .bind(n.status_category)
    .execute(conn)
    .await?;
    Ok(())
}

pub async fn issue_by_key(
    conn: &mut PgConnection,
    tenant_id: Uuid,
    project_key: &str,
    number: i64,
    lock: bool,
) -> ApiResult<Option<Issue>> {
    let suffix = if lock { " FOR UPDATE OF i" } else { "" };
    Ok(sqlx::query_as::<_, Issue>(&format!(
        "SELECT {ISSUE} WHERE i.tenant_id = $1 AND i.project_key = $2 AND i.number = $3 AND i.deleted_at IS NULL{suffix}"
    ))
    .bind(tenant_id)
    .bind(project_key)
    .bind(number)
    .fetch_optional(conn)
    .await?)
}

pub async fn issue_by_id(conn: &mut PgConnection, tenant_id: Uuid, id: Uuid) -> ApiResult<Option<Issue>> {
    issue_by_id_for(conn, tenant_id, id, false).await
}

pub async fn issue_by_id_for(conn: &mut PgConnection, tenant_id: Uuid, id: Uuid, lock: bool) -> ApiResult<Option<Issue>> {
    let suffix = if lock { " FOR UPDATE OF i" } else { "" };
    Ok(sqlx::query_as::<_, Issue>(&format!(
        "SELECT {ISSUE} WHERE i.tenant_id = $1 AND i.id = $2 AND i.deleted_at IS NULL{suffix}"
    ))
    .bind(tenant_id)
    .bind(id)
    .fetch_optional(conn)
    .await?)
}

/// Persist every mutable column of `issue` and advance its version. The caller holds the
/// row lock and has already verified the version it read.
pub async fn save_issue(conn: &mut PgConnection, tenant_id: Uuid, issue: &Issue) -> ApiResult<()> {
    sqlx::query(
        "UPDATE issues SET type_key = $3, status = $4, priority = $5, summary = $6, description = $7, assignee_id = $8, \
             parent_id = $9, labels = $10, story_points = $11::float8::numeric(6,1), start_date = $12, due_date = $13, \
             resolution = $14, resolved_at = $15, component_ids = $16, fix_version_ids = $17, custom_fields = $18, status_category = $19, \
             version = version + 1, updated_at = now() \
         WHERE tenant_id = $1 AND id = $2",
    )
    .bind(tenant_id)
    .bind(issue.id)
    .bind(&issue.type_key)
    .bind(&issue.status)
    .bind(&issue.priority)
    .bind(&issue.summary)
    .bind(&issue.description)
    .bind(issue.assignee_id)
    .bind(issue.parent_id)
    .bind(&issue.labels)
    .bind(issue.story_points)
    .bind(issue.start_date)
    .bind(issue.due_date)
    .bind(&issue.resolution)
    .bind(issue.resolved_at)
    .bind(&issue.component_ids)
    .bind(&issue.fix_version_ids)
    .bind(&issue.custom_fields)
    .bind(&issue.status_category)
    .execute(conn)
    .await?;
    Ok(())
}

pub async fn has_children(conn: &mut PgConnection, tenant_id: Uuid, id: Uuid) -> ApiResult<bool> {
    let (exists,): (bool,) =
        sqlx::query_as("SELECT EXISTS (SELECT 1 FROM issues WHERE tenant_id = $1 AND parent_id = $2 AND deleted_at IS NULL)")
            .bind(tenant_id)
            .bind(id)
            .fetch_one(conn)
            .await?;
    Ok(exists)
}

/// True when the issue has no children that are not done (so also when it has none).
pub async fn children_done(conn: &mut PgConnection, tenant_id: Uuid, id: Uuid) -> ApiResult<bool> {
    let (open,): (bool,) = sqlx::query_as(
        "SELECT EXISTS (SELECT 1 FROM issues WHERE tenant_id = $1 AND parent_id = $2 AND deleted_at IS NULL AND status_category <> 'done')",
    )
    .bind(tenant_id)
    .bind(id)
    .fetch_one(conn)
    .await?;
    Ok(!open)
}

pub async fn soft_delete(conn: &mut PgConnection, tenant_id: Uuid, id: Uuid) -> ApiResult<()> {
    sqlx::query(
        "UPDATE issues SET deleted_at = now(), version = version + 1, updated_at = now() WHERE tenant_id = $1 AND id = $2",
    )
    .bind(tenant_id)
    .bind(id)
    .execute(&mut *conn)
    .await?;
    // Links to a deleted issue would dangle in other issues' panels.
    sqlx::query("DELETE FROM issue_links WHERE tenant_id = $1 AND (source_id = $2 OR target_id = $2)")
        .bind(tenant_id)
        .bind(id)
        .execute(conn)
        .await?;
    Ok(())
}

pub enum AssigneeFilter {
    Unassigned,
    User(Uuid),
}

#[derive(Default)]
pub struct IssueFilter {
    pub statuses: Vec<String>,
    pub status_categories: Vec<String>,
    pub types: Vec<String>,
    pub priorities: Vec<String>,
    pub assignee: Option<AssigneeFilter>,
    pub label: Option<String>,
    pub parent_id: Option<Uuid>,
    pub text: Option<String>,
    /// Keyset cursor: return issues with a number below this.
    pub before_number: Option<i64>,
}

/// Newest first. Every value reaches the database as a bind parameter.
pub async fn list_issues(
    conn: &mut PgConnection,
    tenant_id: Uuid,
    project_id: Uuid,
    f: &IssueFilter,
    limit: i64,
) -> ApiResult<Vec<Issue>> {
    let mut qb: QueryBuilder<'_, Postgres> = QueryBuilder::new(format!("SELECT {ISSUE} WHERE i.tenant_id = "));
    qb.push_bind(tenant_id).push(" AND i.project_id = ").push_bind(project_id).push(" AND i.deleted_at IS NULL");
    if !f.statuses.is_empty() {
        qb.push(" AND i.status = ANY(").push_bind(&f.statuses).push(")");
    }
    if !f.status_categories.is_empty() {
        qb.push(" AND i.status_category = ANY(").push_bind(&f.status_categories).push(")");
    }
    if !f.types.is_empty() {
        qb.push(" AND i.type_key = ANY(").push_bind(&f.types).push(")");
    }
    if !f.priorities.is_empty() {
        qb.push(" AND i.priority = ANY(").push_bind(&f.priorities).push(")");
    }
    match &f.assignee {
        Some(AssigneeFilter::Unassigned) => {
            qb.push(" AND i.assignee_id IS NULL");
        }
        Some(AssigneeFilter::User(id)) => {
            qb.push(" AND i.assignee_id = ").push_bind(*id);
        }
        None => {}
    }
    if let Some(label) = &f.label {
        qb.push(" AND i.labels @> ARRAY[").push_bind(label).push("]::text[]");
    }
    if let Some(parent) = f.parent_id {
        qb.push(" AND i.parent_id = ").push_bind(parent);
    }
    if let Some(text) = &f.text {
        let pattern = format!("%{}%", text.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"));
        qb.push(" AND (i.summary ILIKE ")
            .push_bind(pattern.clone())
            .push(" OR (i.project_key || '-' || i.number) ILIKE ")
            .push_bind(pattern)
            .push(")");
    }
    if let Some(before) = f.before_number {
        qb.push(" AND i.number < ").push_bind(before);
    }
    qb.push(" ORDER BY i.number DESC LIMIT ").push_bind(limit + 1);
    Ok(qb.build_query_as::<Issue>().fetch_all(conn).await?)
}

/// Issues for board cards. With `ids`, exactly those; otherwise every issue of the
/// project that is unfinished or was resolved at or after `done_since`.
pub async fn cards(
    conn: &mut PgConnection,
    tenant_id: Uuid,
    project_id: Uuid,
    ids: &[Uuid],
    done_since: Option<OffsetDateTime>,
    exclude_types: &[String],
    limit: i64,
) -> ApiResult<Vec<Issue>> {
    let mut qb: QueryBuilder<'_, Postgres> = QueryBuilder::new(format!("SELECT {ISSUE} WHERE i.tenant_id = "));
    qb.push_bind(tenant_id).push(" AND i.project_id = ").push_bind(project_id).push(" AND i.deleted_at IS NULL");
    if ids.is_empty() {
        if let Some(since) = done_since {
            qb.push(" AND (i.status_category <> 'done' OR i.resolved_at >= ").push_bind(since).push(")");
        }
        if !exclude_types.is_empty() {
            qb.push(" AND NOT (i.type_key = ANY(").push_bind(exclude_types).push("))");
        }
    } else {
        qb.push(" AND i.id = ANY(").push_bind(ids).push(")");
    }
    qb.push(" ORDER BY i.number LIMIT ").push_bind(limit + 1);
    Ok(qb.build_query_as::<Issue>().fetch_all(conn).await?)
}

// --- history -----------------------------------------------------------------

pub async fn add_history(
    conn: &mut PgConnection,
    tenant_id: Uuid,
    issue_id: Uuid,
    actor: Uuid,
    version: i64,
    changes: &Value,
) -> ApiResult<()> {
    sqlx::query(
        "INSERT INTO issue_history (id, tenant_id, issue_id, actor_id, issue_version, changes) VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(platform_common::new_id())
    .bind(tenant_id)
    .bind(issue_id)
    .bind(actor)
    .bind(version)
    .bind(changes)
    .execute(conn)
    .await?;
    Ok(())
}

#[derive(Debug, Clone, FromRow)]
pub struct HistoryRow {
    pub id: Uuid,
    pub actor_id: Uuid,
    pub changes: Value,
    pub created_at: OffsetDateTime,
}

pub async fn history(conn: &mut PgConnection, tenant_id: Uuid, issue_id: Uuid) -> ApiResult<Vec<HistoryRow>> {
    Ok(sqlx::query_as(
        "SELECT id, actor_id, changes, created_at FROM issue_history WHERE tenant_id = $1 AND issue_id = $2 ORDER BY created_at DESC, id DESC LIMIT 500",
    )
    .bind(tenant_id)
    .bind(issue_id)
    .fetch_all(conn)
    .await?)
}

// --- comments ----------------------------------------------------------------

#[derive(Debug, Clone, FromRow)]
pub struct Comment {
    pub id: Uuid,
    pub issue_id: Uuid,
    pub author_id: Uuid,
    pub body: Value,
    pub created_at: OffsetDateTime,
    pub edited_at: Option<OffsetDateTime>,
    /// Set when an automation rule posted the comment (as `author_id`).
    pub rule_name: Option<String>,
}

const COMMENT: &str = "id, issue_id, author_id, body, created_at, edited_at, rule_name";

pub async fn comments(conn: &mut PgConnection, tenant_id: Uuid, issue_id: Uuid) -> ApiResult<Vec<Comment>> {
    Ok(sqlx::query_as(&format!(
        "SELECT {COMMENT} FROM comments WHERE tenant_id = $1 AND issue_id = $2 AND deleted_at IS NULL ORDER BY created_at, id LIMIT 1000"
    ))
    .bind(tenant_id)
    .bind(issue_id)
    .fetch_all(conn)
    .await?)
}

pub async fn insert_comment(
    conn: &mut PgConnection,
    tenant_id: Uuid,
    issue_id: Uuid,
    author: Uuid,
    body: &Value,
    rule_name: Option<&str>,
) -> ApiResult<Comment> {
    Ok(sqlx::query_as(&format!(
        "INSERT INTO comments (id, tenant_id, issue_id, author_id, body, rule_name) VALUES ($1, $2, $3, $4, $5, $6) RETURNING {COMMENT}"
    ))
    .bind(platform_common::new_id())
    .bind(tenant_id)
    .bind(issue_id)
    .bind(author)
    .bind(body)
    .bind(rule_name)
    .fetch_one(conn)
    .await?)
}

pub async fn comment(conn: &mut PgConnection, tenant_id: Uuid, issue_id: Uuid, id: Uuid) -> ApiResult<Option<Comment>> {
    Ok(sqlx::query_as(&format!(
        "SELECT {COMMENT} FROM comments WHERE tenant_id = $1 AND issue_id = $2 AND id = $3 AND deleted_at IS NULL FOR UPDATE"
    ))
    .bind(tenant_id)
    .bind(issue_id)
    .bind(id)
    .fetch_optional(conn)
    .await?)
}

/// A live comment's body, without locking it (for reading only).
pub async fn comment_body(conn: &mut PgConnection, tenant_id: Uuid, issue_id: Uuid, id: Uuid) -> ApiResult<Option<Value>> {
    Ok(sqlx::query_scalar("SELECT body FROM comments WHERE tenant_id = $1 AND issue_id = $2 AND id = $3 AND deleted_at IS NULL")
        .bind(tenant_id)
        .bind(issue_id)
        .bind(id)
        .fetch_optional(conn)
        .await?)
}

pub async fn update_comment(conn: &mut PgConnection, tenant_id: Uuid, id: Uuid, body: &Value) -> ApiResult<Comment> {
    Ok(sqlx::query_as(&format!(
        "UPDATE comments SET body = $3, edited_at = now() WHERE tenant_id = $1 AND id = $2 RETURNING {COMMENT}"
    ))
    .bind(tenant_id)
    .bind(id)
    .bind(body)
    .fetch_one(conn)
    .await?)
}

pub async fn delete_comment(conn: &mut PgConnection, tenant_id: Uuid, id: Uuid) -> ApiResult<()> {
    sqlx::query("UPDATE comments SET deleted_at = now() WHERE tenant_id = $1 AND id = $2")
        .bind(tenant_id)
        .bind(id)
        .execute(conn)
        .await?;
    Ok(())
}

// --- links -------------------------------------------------------------------

#[derive(Debug, Clone, FromRow)]
pub struct LinkRow {
    pub id: Uuid,
    pub link_type: String,
    /// true when the issue being viewed is the source of the link.
    pub outward: bool,
    pub other_project_id: Uuid,
    pub other_project_key: String,
    pub other_number: i64,
    pub other_summary: String,
    pub other_status: String,
    pub other_status_category: String,
    pub other_type: String,
}

pub async fn links(conn: &mut PgConnection, tenant_id: Uuid, issue_id: Uuid) -> ApiResult<Vec<LinkRow>> {
    Ok(sqlx::query_as(
        "SELECT l.id, l.link_type, (l.source_id = $2) AS outward, o.project_id AS other_project_id, \
                o.project_key AS other_project_key, o.number AS other_number, o.summary AS other_summary, \
                o.status AS other_status, o.status_category AS other_status_category, o.type_key AS other_type \
         FROM issue_links l \
         JOIN issues o ON o.id = CASE WHEN l.source_id = $2 THEN l.target_id ELSE l.source_id END AND o.deleted_at IS NULL \
         WHERE l.tenant_id = $1 AND (l.source_id = $2 OR l.target_id = $2) ORDER BY l.created_at LIMIT 500",
    )
    .bind(tenant_id)
    .bind(issue_id)
    .fetch_all(conn)
    .await?)
}

pub async fn insert_link(
    conn: &mut PgConnection,
    tenant_id: Uuid,
    source: Uuid,
    target: Uuid,
    link_type: &str,
    by: Uuid,
) -> Result<Uuid, sqlx::Error> {
    let id = platform_common::new_id();
    sqlx::query(
        "INSERT INTO issue_links (id, tenant_id, source_id, target_id, link_type, created_by) VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(id)
    .bind(tenant_id)
    .bind(source)
    .bind(target)
    .bind(link_type)
    .bind(by)
    .execute(conn)
    .await?;
    Ok(id)
}

pub async fn delete_link(conn: &mut PgConnection, tenant_id: Uuid, issue_id: Uuid, link_id: Uuid) -> ApiResult<bool> {
    Ok(sqlx::query("DELETE FROM issue_links WHERE tenant_id = $1 AND id = $2 AND (source_id = $3 OR target_id = $3)")
        .bind(tenant_id)
        .bind(link_id)
        .bind(issue_id)
        .execute(conn)
        .await?
        .rows_affected()
        == 1)
}

// --- watchers ----------------------------------------------------------------

pub async fn watch(conn: &mut PgConnection, tenant_id: Uuid, issue_id: Uuid, user_id: Uuid) -> ApiResult<()> {
    sqlx::query("INSERT INTO watchers (tenant_id, issue_id, user_id) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING")
        .bind(tenant_id)
        .bind(issue_id)
        .bind(user_id)
        .execute(conn)
        .await?;
    Ok(())
}

pub async fn unwatch(conn: &mut PgConnection, tenant_id: Uuid, issue_id: Uuid, user_id: Uuid) -> ApiResult<()> {
    sqlx::query("DELETE FROM watchers WHERE tenant_id = $1 AND issue_id = $2 AND user_id = $3")
        .bind(tenant_id)
        .bind(issue_id)
        .bind(user_id)
        .execute(conn)
        .await?;
    Ok(())
}

/// (is the user watching, total watchers)
pub async fn watch_state(conn: &mut PgConnection, tenant_id: Uuid, issue_id: Uuid, user_id: Uuid) -> ApiResult<(bool, i64)> {
    Ok(sqlx::query_as(
        "SELECT COALESCE(bool_or(user_id = $3), false), count(*) FROM watchers WHERE tenant_id = $1 AND issue_id = $2",
    )
    .bind(tenant_id)
    .bind(issue_id)
    .bind(user_id)
    .fetch_one(conn)
    .await?)
}

// --- votes -------------------------------------------------------------------

pub async fn vote(conn: &mut PgConnection, tenant_id: Uuid, issue_id: Uuid, user_id: Uuid) -> ApiResult<()> {
    sqlx::query("INSERT INTO votes (tenant_id, issue_id, user_id) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING")
        .bind(tenant_id)
        .bind(issue_id)
        .bind(user_id)
        .execute(conn)
        .await?;
    Ok(())
}

pub async fn unvote(conn: &mut PgConnection, tenant_id: Uuid, issue_id: Uuid, user_id: Uuid) -> ApiResult<()> {
    sqlx::query("DELETE FROM votes WHERE tenant_id = $1 AND issue_id = $2 AND user_id = $3")
        .bind(tenant_id)
        .bind(issue_id)
        .bind(user_id)
        .execute(conn)
        .await?;
    Ok(())
}

/// (has the user voted, total votes)
pub async fn vote_state(conn: &mut PgConnection, tenant_id: Uuid, issue_id: Uuid, user_id: Uuid) -> ApiResult<(bool, i64)> {
    Ok(sqlx::query_as(
        "SELECT COALESCE(bool_or(user_id = $3), false), count(*) FROM votes WHERE tenant_id = $1 AND issue_id = $2",
    )
    .bind(tenant_id)
    .bind(issue_id)
    .bind(user_id)
    .fetch_one(conn)
    .await?)
}

/// Who voted, earliest first (at most 500).
pub async fn voters(conn: &mut PgConnection, tenant_id: Uuid, issue_id: Uuid) -> ApiResult<Vec<(Uuid, OffsetDateTime)>> {
    Ok(sqlx::query_as(
        "SELECT user_id, created_at FROM votes WHERE tenant_id = $1 AND issue_id = $2 ORDER BY created_at, user_id LIMIT 500",
    )
    .bind(tenant_id)
    .bind(issue_id)
    .fetch_all(conn)
    .await?)
}

// --- custom fields ----------------------------------------------------------

use crate::custom_fields::Field;

const FIELD: &str = "id, name, field_type, options, required, project_keys, issue_types, created_at";

pub async fn fields(conn: &mut PgConnection, tenant_id: Uuid) -> ApiResult<Vec<Field>> {
    Ok(sqlx::query_as(&format!(
        "SELECT {FIELD} FROM custom_fields WHERE tenant_id = $1 AND archived_at IS NULL ORDER BY lower(name) LIMIT 500"
    ))
    .bind(tenant_id)
    .fetch_all(conn)
    .await?)
}

#[allow(clippy::too_many_arguments)]
pub async fn insert_field(
    conn: &mut PgConnection,
    tenant_id: Uuid,
    name: &str,
    field_type: &str,
    options: &[String],
    required: bool,
    project_keys: &[String],
    issue_types: &[String],
    created_by: Uuid,
) -> Result<Field, sqlx::Error> {
    sqlx::query_as(&format!(
        "INSERT INTO custom_fields (id, tenant_id, name, field_type, options, required, project_keys, issue_types, created_by) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) RETURNING {FIELD}"
    ))
    .bind(platform_common::new_id())
    .bind(tenant_id)
    .bind(name)
    .bind(field_type)
    .bind(options)
    .bind(required)
    .bind(project_keys)
    .bind(issue_types)
    .bind(created_by)
    .fetch_one(conn)
    .await
}

/// The type of a field never changes: stored values would stop making sense.
#[allow(clippy::too_many_arguments)]
pub async fn update_field(
    conn: &mut PgConnection,
    tenant_id: Uuid,
    id: Uuid,
    name: &str,
    options: &[String],
    required: bool,
    project_keys: &[String],
    issue_types: &[String],
) -> Result<Option<Field>, sqlx::Error> {
    sqlx::query_as(&format!(
        "UPDATE custom_fields SET name = $3, options = $4, required = $5, project_keys = $6, issue_types = $7, updated_at = now() \
         WHERE tenant_id = $1 AND id = $2 AND archived_at IS NULL RETURNING {FIELD}"
    ))
    .bind(tenant_id)
    .bind(id)
    .bind(name)
    .bind(options)
    .bind(required)
    .bind(project_keys)
    .bind(issue_types)
    .fetch_optional(conn)
    .await
}

pub async fn archive_field(conn: &mut PgConnection, tenant_id: Uuid, id: Uuid) -> ApiResult<bool> {
    Ok(sqlx::query("UPDATE custom_fields SET archived_at = now() WHERE tenant_id = $1 AND id = $2 AND archived_at IS NULL")
        .bind(tenant_id)
        .bind(id)
        .execute(conn)
        .await?
        .rows_affected()
        == 1)
}

// --- attachments ------------------------------------------------------------

#[derive(Debug, Clone, FromRow)]
pub struct Attachment {
    pub id: Uuid,
    pub uploader_id: Uuid,
    pub filename: String,
    pub content_type: String,
    pub size_bytes: i64,
    pub object_key: String,
    pub created_at: OffsetDateTime,
}

const ATTACHMENT: &str = "id, uploader_id, filename, content_type, size_bytes, object_key, created_at";

pub async fn attachments(conn: &mut PgConnection, tenant_id: Uuid, issue_id: Uuid) -> ApiResult<Vec<Attachment>> {
    Ok(sqlx::query_as(&format!(
        "SELECT {ATTACHMENT} FROM attachments WHERE tenant_id = $1 AND issue_id = $2 AND deleted_at IS NULL ORDER BY created_at, id"
    ))
    .bind(tenant_id)
    .bind(issue_id)
    .fetch_all(conn)
    .await?)
}

pub async fn attachment(conn: &mut PgConnection, tenant_id: Uuid, issue_id: Uuid, id: Uuid) -> ApiResult<Option<Attachment>> {
    Ok(sqlx::query_as(&format!(
        "SELECT {ATTACHMENT} FROM attachments WHERE tenant_id = $1 AND issue_id = $2 AND id = $3 AND deleted_at IS NULL"
    ))
    .bind(tenant_id)
    .bind(issue_id)
    .bind(id)
    .fetch_optional(conn)
    .await?)
}

#[allow(clippy::too_many_arguments)]
pub async fn insert_attachment(
    conn: &mut PgConnection,
    tenant_id: Uuid,
    id: Uuid,
    issue_id: Uuid,
    uploader: Uuid,
    filename: &str,
    content_type: &str,
    size: i64,
    sha256: &str,
    object_key: &str,
) -> ApiResult<Attachment> {
    Ok(sqlx::query_as(&format!(
        "INSERT INTO attachments (id, tenant_id, issue_id, uploader_id, filename, content_type, size_bytes, sha256, object_key) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) RETURNING {ATTACHMENT}"
    ))
    .bind(id)
    .bind(tenant_id)
    .bind(issue_id)
    .bind(uploader)
    .bind(filename)
    .bind(content_type)
    .bind(size)
    .bind(sha256)
    .bind(object_key)
    .fetch_one(conn)
    .await?)
}

pub async fn delete_attachment(conn: &mut PgConnection, tenant_id: Uuid, id: Uuid) -> ApiResult<()> {
    sqlx::query("UPDATE attachments SET deleted_at = now() WHERE tenant_id = $1 AND id = $2")
        .bind(tenant_id)
        .bind(id)
        .execute(conn)
        .await?;
    Ok(())
}

// --- internal readers (gRPC) ---------------------------------------------------

pub async fn issues_by_id(conn: &mut PgConnection, tenant_id: Uuid, ids: &[Uuid]) -> ApiResult<Vec<Issue>> {
    Ok(sqlx::query_as::<_, Issue>(&format!("SELECT {ISSUE} WHERE i.tenant_id = $1 AND i.id = ANY($2) AND i.deleted_at IS NULL"))
        .bind(tenant_id)
        .bind(ids)
        .fetch_all(conn)
        .await?)
}

pub async fn watcher_ids(conn: &mut PgConnection, tenant_id: Uuid, issue_id: Uuid) -> ApiResult<Vec<Uuid>> {
    Ok(sqlx::query_scalar("SELECT user_id FROM watchers WHERE tenant_id = $1 AND issue_id = $2 ORDER BY created_at LIMIT 1000")
        .bind(tenant_id)
        .bind(issue_id)
        .fetch_all(conn)
        .await?)
}

/// Comment bodies per issue, oldest first, for the given issues.
pub async fn comment_bodies(conn: &mut PgConnection, tenant_id: Uuid, ids: &[Uuid]) -> ApiResult<Vec<(Uuid, Value)>> {
    Ok(sqlx::query_as(
        "SELECT issue_id, body FROM comments WHERE tenant_id = $1 AND issue_id = ANY($2) AND deleted_at IS NULL ORDER BY created_at, id",
    )
    .bind(tenant_id)
    .bind(ids)
    .fetch_all(conn)
    .await?)
}

pub async fn issue_ids_after(conn: &mut PgConnection, tenant_id: Uuid, after: Option<Uuid>, limit: i64) -> ApiResult<Vec<Uuid>> {
    Ok(sqlx::query_scalar(
        "SELECT id FROM issues WHERE tenant_id = $1 AND deleted_at IS NULL AND ($2::uuid IS NULL OR id > $2) ORDER BY id LIMIT $3",
    )
    .bind(tenant_id)
    .bind(after)
    .bind(limit)
    .fetch_all(conn)
    .await?)
}
