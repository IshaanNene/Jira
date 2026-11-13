use error_handling::ApiResult;
use serde_json::Value;
use sqlx::PgConnection;
use time::OffsetDateTime;
use uuid::Uuid;

pub struct NewNotification<'a> {
    pub id: Uuid,
    pub recipient_id: Uuid,
    pub kind: &'a str,
    pub project_id: Uuid,
    pub issue_id: Uuid,
    pub issue_key: &'a str,
    pub issue_summary: &'a str,
    pub actor_id: Option<Uuid>,
    pub actor_name: &'a str,
    pub detail: &'a Value,
    pub source_event_id: Uuid,
}

/// `false` if this person already has a notification for this event.
pub async fn insert(conn: &mut PgConnection, tenant_id: Uuid, n: &NewNotification<'_>) -> ApiResult<bool> {
    let done = sqlx::query(
        "INSERT INTO notifications (id, tenant_id, recipient_id, kind, project_id, issue_id, issue_key, issue_summary, \
         actor_id, actor_name, detail, source_event_id) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12) \
         ON CONFLICT (tenant_id, recipient_id, source_event_id) DO NOTHING",
    )
    .bind(n.id)
    .bind(tenant_id)
    .bind(n.recipient_id)
    .bind(n.kind)
    .bind(n.project_id)
    .bind(n.issue_id)
    .bind(n.issue_key)
    .bind(n.issue_summary)
    .bind(n.actor_id)
    .bind(n.actor_name)
    .bind(n.detail)
    .bind(n.source_event_id)
    .execute(conn)
    .await?;
    Ok(done.rows_affected() == 1)
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Notification {
    pub id: Uuid,
    pub kind: String,
    pub project_id: Uuid,
    pub issue_id: Uuid,
    pub issue_key: String,
    pub issue_summary: String,
    pub actor_id: Option<Uuid>,
    pub actor_name: String,
    pub detail: Value,
    pub read_at: Option<OffsetDateTime>,
    pub created_at: OffsetDateTime,
}

/// Restricts reads to projects the person can still see (`None` = all projects).
pub type Visible<'a> = Option<&'a [Uuid]>;

pub async fn list(
    conn: &mut PgConnection,
    tenant_id: Uuid,
    user_id: Uuid,
    visible: Visible<'_>,
    unread_only: bool,
    after: Option<(OffsetDateTime, Uuid)>,
    limit: i64,
) -> ApiResult<Vec<Notification>> {
    let (at, id) = after.map_or((None, None), |(a, i)| (Some(a), Some(i)));
    Ok(sqlx::query_as(
        "SELECT id, kind, project_id, issue_id, issue_key, issue_summary, actor_id, actor_name, detail, read_at, created_at \
         FROM notifications WHERE tenant_id = $1 AND recipient_id = $2 \
         AND ($3::uuid[] IS NULL OR project_id = ANY($3)) AND (NOT $4 OR read_at IS NULL) \
         AND ($5::timestamptz IS NULL OR (created_at, id) < ($5, $6)) \
         ORDER BY created_at DESC, id DESC LIMIT $7",
    )
    .bind(tenant_id)
    .bind(user_id)
    .bind(visible)
    .bind(unread_only)
    .bind(at)
    .bind(id)
    .bind(limit)
    .fetch_all(conn)
    .await?)
}

pub async fn unread_count(conn: &mut PgConnection, tenant_id: Uuid, user_id: Uuid, visible: Visible<'_>) -> ApiResult<i64> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM notifications WHERE tenant_id = $1 AND recipient_id = $2 AND read_at IS NULL \
         AND ($3::uuid[] IS NULL OR project_id = ANY($3))",
    )
    .bind(tenant_id)
    .bind(user_id)
    .bind(visible)
    .fetch_one(conn)
    .await?)
}

pub async fn mark_read(conn: &mut PgConnection, tenant_id: Uuid, user_id: Uuid, id: Uuid) -> ApiResult<bool> {
    let done = sqlx::query(
        "UPDATE notifications SET read_at = COALESCE(read_at, now()) WHERE tenant_id = $1 AND recipient_id = $2 AND id = $3",
    )
    .bind(tenant_id)
    .bind(user_id)
    .bind(id)
    .execute(conn)
    .await?;
    Ok(done.rows_affected() == 1)
}

pub async fn mark_all_read(conn: &mut PgConnection, tenant_id: Uuid, user_id: Uuid) -> ApiResult<u64> {
    Ok(sqlx::query("UPDATE notifications SET read_at = now() WHERE tenant_id = $1 AND recipient_id = $2 AND read_at IS NULL")
        .bind(tenant_id)
        .bind(user_id)
        .execute(conn)
        .await?
        .rows_affected())
}
