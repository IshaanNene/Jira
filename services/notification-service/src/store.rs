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

// --- Email preferences -------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct Preferences {
    pub email_enabled: bool,
    pub email_muted: Vec<String>,
    /// off | hourly | daily: activity emails one by one, or gathered into one per hour or day.
    pub email_digest: String,
}

impl Default for Preferences {
    fn default() -> Self {
        Self { email_enabled: true, email_muted: Vec::new(), email_digest: DIGEST_OFF.into() }
    }
}

pub const DIGEST_OFF: &str = "off";

/// How long a digest window lasts, for an `email_digest` value; `None` when off.
pub fn digest_window(value: &str) -> Option<time::Duration> {
    match value {
        "hourly" => Some(time::Duration::hours(1)),
        "daily" => Some(time::Duration::days(1)),
        _ => None,
    }
}

impl Preferences {
    pub fn wants_email(&self, kind: &str) -> bool {
        self.email_enabled && !self.email_muted.iter().any(|k| k == kind)
    }
}

pub async fn preferences(conn: &mut PgConnection, tenant_id: Uuid, user_id: Uuid) -> ApiResult<Preferences> {
    Ok(sqlx::query_as(
        "SELECT email_enabled, email_muted, email_digest FROM notification_preferences WHERE tenant_id = $1 AND user_id = $2",
    )
    .bind(tenant_id)
    .bind(user_id)
    .fetch_optional(conn)
    .await?
    .unwrap_or_default())
}

pub async fn preferences_of(conn: &mut PgConnection, tenant_id: Uuid, user_ids: &[Uuid]) -> ApiResult<Vec<(Uuid, Preferences)>> {
    let rows: Vec<(Uuid, bool, Vec<String>, String)> = sqlx::query_as(
        "SELECT user_id, email_enabled, email_muted, email_digest FROM notification_preferences WHERE tenant_id = $1 AND user_id = ANY($2)",
    )
    .bind(tenant_id)
    .bind(user_ids)
    .fetch_all(conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(u, email_enabled, email_muted, email_digest)| (u, Preferences { email_enabled, email_muted, email_digest }))
        .collect())
}

pub async fn save_preferences(conn: &mut PgConnection, tenant_id: Uuid, user_id: Uuid, p: &Preferences) -> ApiResult<()> {
    sqlx::query(
        "INSERT INTO notification_preferences (tenant_id, user_id, email_enabled, email_muted, email_digest) VALUES ($1, $2, $3, $4, $5) \
         ON CONFLICT (tenant_id, user_id) DO UPDATE SET email_enabled = $3, email_muted = $4, email_digest = $5, updated_at = now()",
    )
    .bind(tenant_id)
    .bind(user_id)
    .bind(p.email_enabled)
    .bind(&p.email_muted)
    .bind(&p.email_digest)
    .execute(conn)
    .await?;
    Ok(())
}

// --- Email queue -------------------------------------------------------------------

pub struct NewDelivery<'a> {
    pub notification_id: Uuid,
    pub recipient_id: Uuid,
    pub kind: &'a str,
    pub to_address: &'a str,
    pub subject: &'a str,
    pub text_body: &'a str,
    pub html_body: &'a str,
    pub unsubscribe_url: Option<&'a str>,
    pub send_after: OffsetDateTime,
    /// Gather into the recipient's digest; `send_after` is then only used when no digest
    /// window is open yet (see [`digest_send_after`]).
    pub digest: bool,
}

pub async fn enqueue_email(conn: &mut PgConnection, tenant_id: Uuid, d: &NewDelivery<'_>) -> ApiResult<()> {
    sqlx::query(
        "INSERT INTO email_deliveries (id, tenant_id, notification_id, recipient_id, kind, to_address, subject, text_body, html_body, send_after, unsubscribe_url, digest) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12) ON CONFLICT (notification_id) DO NOTHING",
    )
    .bind(platform_common::new_id())
    .bind(tenant_id)
    .bind(d.notification_id)
    .bind(d.recipient_id)
    .bind(d.kind)
    .bind(d.to_address)
    .bind(d.subject)
    .bind(d.text_body)
    .bind(d.html_body)
    .bind(d.send_after)
    .bind(d.unsubscribe_url)
    .bind(d.digest)
    .execute(conn)
    .await?;
    Ok(())
}

/// When a new digest row of the person goes out: with the digest window already open (their
/// other pending digest rows), or a new window of `window` from now.
pub async fn digest_send_after(
    conn: &mut PgConnection,
    tenant_id: Uuid,
    recipient_id: Uuid,
    window: time::Duration,
) -> ApiResult<OffsetDateTime> {
    let open: Option<OffsetDateTime> = sqlx::query_scalar(
        "SELECT min(send_after) FROM email_deliveries WHERE tenant_id = $1 AND recipient_id = $2 AND digest AND status = 'pending'",
    )
    .bind(tenant_id)
    .bind(recipient_id)
    .fetch_one(conn)
    .await?;
    Ok(open.unwrap_or_else(|| OffsetDateTime::now_utc() + window))
}

/// Turn email off for one kind, or entirely (`kind` = None). Never turns anything on.
pub async fn mute_email(conn: &mut PgConnection, tenant_id: Uuid, user_id: Uuid, kind: Option<&str>) -> ApiResult<()> {
    let mut prefs = preferences(&mut *conn, tenant_id, user_id).await?;
    match kind {
        Some(kind) if !prefs.email_muted.iter().any(|k| k == kind) => prefs.email_muted.push(kind.to_owned()),
        Some(_) => {}
        None => prefs.email_enabled = false,
    }
    save_preferences(conn, tenant_id, user_id, &prefs).await
}
