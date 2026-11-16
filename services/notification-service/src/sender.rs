//! Drains the email queue. Emails wait a short while before sending so that a
//! notification read in the app in the meantime is not also emailed, and preferences
//! changed in that window are honoured.
//!
//! Rows are leased (`sending` with a lease expiry in `send_after`), so several instances
//! can run and a crashed one's rows are picked up again. Recipient addresses are never
//! logged.
//!
//! Digest rows (people who chose hourly or daily email) wait for their recipient's window
//! to close and go out together as one email listing what happened; each still drops out
//! if it was read in the app or its kind was muted in the meantime.
//!
//! An address on the suppression list (see [`crate::suppression`]) is not emailed; a
//! recipient the receiving server rejects is put on it.

use std::{sync::Arc, time::Duration};

use sqlx::{PgConnection, PgPool};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    domain::Kind,
    email::{self, ActivityLine},
    mailer::{Mailer, Outgoing, SendError},
    store, suppression,
};

const BATCH: i64 = 20;
const LEASE_SECONDS: i64 = 300;
pub const MAX_ATTEMPTS: i32 = 6;
const IDLE: Duration = Duration::from_secs(2);
const RETENTION_DAYS: i64 = 7;
/// Most notifications gathered into one digest email; later ones wait for the next.
const DIGEST_ROWS: i64 = 200;

/// What the sender needs besides the queue.
#[derive(Clone)]
pub struct Settings {
    /// Base URL of the web app for links in digests, without a trailing slash.
    pub app_url: String,
    /// The global database, where the suppression list is kept.
    pub suppressions: PgPool,
}

#[derive(Debug, sqlx::FromRow)]
struct Claimed {
    id: Uuid,
    tenant_id: Uuid,
    notification_id: Uuid,
    recipient_id: Uuid,
    kind: String,
    to_address: String,
    subject: String,
    text_body: String,
    html_body: String,
    unsubscribe_url: Option<String>,
    attempts: i32,
}

/// Wait before retry `attempt` (1-based): 1, 2, 4, 8, 16 minutes.
pub fn backoff(attempt: i32) -> time::Duration {
    time::Duration::minutes(1i64 << (attempt.clamp(1, 6) - 1))
}

async fn scope(conn: &mut PgConnection, tenant_id: Uuid) -> sqlx::Result<()> {
    sqlx::query("SELECT set_config('app.tenant_id', $1, true)").bind(tenant_id.to_string()).execute(conn).await?;
    Ok(())
}

/// Whether the email is still wanted: unread, and the person still wants this kind.
async fn still_wanted(pool: &PgPool, c: &Claimed) -> sqlx::Result<bool> {
    let mut tx = pool.begin().await?;
    scope(&mut tx, c.tenant_id).await?;
    let unread: Option<bool> = sqlx::query_scalar("SELECT read_at IS NULL FROM notifications WHERE id = $1 AND tenant_id = $2")
        .bind(c.notification_id)
        .bind(c.tenant_id)
        .fetch_optional(&mut *tx)
        .await?;
    let prefs =
        store::preferences(&mut tx, c.tenant_id, c.recipient_id).await.map_err(|e| sqlx::Error::Protocol(e.to_string()))?;
    tx.commit().await?;
    // A digest has no in-app notification to read first.
    let pending = if c.kind == Kind::FilterDigest.as_str() { true } else { unread == Some(true) };
    Ok(pending && prefs.wants_email(&c.kind))
}

async fn finish(pool: &PgPool, id: Uuid, status: &str, error: Option<&str>) -> sqlx::Result<()> {
    sqlx::query("UPDATE email_deliveries SET status = $2, last_error = $3, finished_at = now() WHERE id = $1")
        .bind(id)
        .bind(status)
        .bind(error)
        .execute(pool)
        .await?;
    Ok(())
}

async fn finish_all(pool: &PgPool, ids: &[Uuid], status: &str, error: Option<&str>) -> sqlx::Result<()> {
    sqlx::query("UPDATE email_deliveries SET status = $2, last_error = $3, finished_at = now() WHERE id = ANY($1)")
        .bind(ids)
        .bind(status)
        .bind(error)
        .execute(pool)
        .await?;
    Ok(())
}

/// The outcome of one send attempt, applied to every row the email stood for.
async fn settle(
    pool: &PgPool,
    settings: &Settings,
    ids: &[Uuid],
    attempts: i32,
    to: &str,
    result: Result<(), SendError>,
) -> sqlx::Result<()> {
    match result {
        Ok(()) => {
            finish_all(pool, ids, "sent", None).await?;
            metrics::counter!("notification_emails_total", "outcome" => "sent").increment(1);
        }
        Err(SendError::Transient(error)) if attempts < MAX_ATTEMPTS => {
            tracing::warn!(deliveries = ids.len(), attempts, "email not sent; will retry");
            sqlx::query("UPDATE email_deliveries SET status = 'pending', send_after = $2, last_error = $3 WHERE id = ANY($1)")
                .bind(ids)
                .bind(OffsetDateTime::now_utc() + backoff(attempts))
                .bind(error)
                .execute(pool)
                .await?;
        }
        Err(SendError::Rejected(error)) => {
            tracing::warn!(deliveries = ids.len(), "recipient rejected; address suppressed");
            suppression::record(&settings.suppressions, to, suppression::Reason::Bounce).await?;
            finish_all(pool, ids, "failed", Some(&format!("recipient rejected: {error}"))).await?;
            metrics::counter!("notification_emails_total", "outcome" => "rejected").increment(1);
        }
        Err(error) => {
            tracing::error!(deliveries = ids.len(), attempts, "email delivery failed for good");
            finish_all(pool, ids, "failed", Some(&error.to_string())).await?;
            metrics::counter!("notification_emails_total", "outcome" => "failed").increment(1);
        }
    }
    Ok(())
}

async fn suppressed(settings: &Settings, address: &str) -> sqlx::Result<bool> {
    Ok(suppression::reason_of(&settings.suppressions, address).await?.is_some())
}

/// Send what is due on this database; returns how many rows were handled.
pub async fn run_once(pool: &PgPool, mailer: &dyn Mailer, settings: &Settings) -> sqlx::Result<usize> {
    let claimed: Vec<Claimed> = sqlx::query_as(
        "UPDATE email_deliveries SET status = 'sending', attempts = attempts + 1, \
                send_after = now() + make_interval(secs => $2) \
         WHERE id IN (SELECT id FROM email_deliveries WHERE NOT digest AND status IN ('pending', 'sending') AND send_after <= now() \
                      ORDER BY send_after LIMIT $1 FOR UPDATE SKIP LOCKED) \
         RETURNING id, tenant_id, notification_id, recipient_id, kind, to_address, subject, text_body, html_body, unsubscribe_url, attempts",
    )
    .bind(BATCH)
    .bind(LEASE_SECONDS as f64)
    .fetch_all(pool)
    .await?;

    for c in &claimed {
        if !still_wanted(pool, c).await? {
            finish(pool, c.id, "skipped", None).await?;
            continue;
        }
        if suppressed(settings, &c.to_address).await? {
            finish(pool, c.id, "suppressed", None).await?;
            metrics::counter!("notification_emails_total", "outcome" => "suppressed").increment(1);
            continue;
        }
        let email = Outgoing {
            to: &c.to_address,
            subject: &c.subject,
            text: &c.text_body,
            html: &c.html_body,
            unsubscribe_url: c.unsubscribe_url.as_deref(),
        };
        let result = mailer.send(&email).await;
        settle(pool, settings, &[c.id], c.attempts, &c.to_address, result).await?;
    }

    let mut handled = claimed.len();
    for _ in 0..BATCH {
        let n = digest_once(pool, mailer, settings).await?;
        if n == 0 {
            break;
        }
        handled += n;
    }
    Ok(handled)
}

/// What a digest says about one notification, and whether it still belongs in it.
#[derive(sqlx::FromRow)]
struct Activity {
    id: Uuid,
    kind: String,
    issue_key: String,
    issue_summary: String,
    actor_name: String,
    detail: serde_json::Value,
    read: bool,
}

/// Send one recipient's due digest; returns how many rows it covered (0: none due).
async fn digest_once(pool: &PgPool, mailer: &dyn Mailer, settings: &Settings) -> sqlx::Result<usize> {
    // One recipient whose window has closed, and all of their due digest rows.
    let mut claimed: Vec<Claimed> = sqlx::query_as(
        "UPDATE email_deliveries SET status = 'sending', attempts = attempts + 1, \
                send_after = now() + make_interval(secs => $2) \
         WHERE id IN (SELECT id FROM email_deliveries d \
                      WHERE digest AND status IN ('pending', 'sending') AND send_after <= now() \
                        AND (tenant_id, recipient_id) = (SELECT tenant_id, recipient_id FROM email_deliveries \
                              WHERE digest AND status IN ('pending', 'sending') AND send_after <= now() \
                              ORDER BY send_after LIMIT 1 FOR UPDATE SKIP LOCKED) \
                      ORDER BY created_at LIMIT $1 FOR UPDATE SKIP LOCKED) \
         RETURNING id, tenant_id, notification_id, recipient_id, kind, to_address, subject, text_body, html_body, unsubscribe_url, attempts",
    )
    .bind(DIGEST_ROWS)
    .bind(LEASE_SECONDS as f64)
    .fetch_all(pool)
    .await?;
    let Some(first) = claimed.first() else { return Ok(0) };
    let (tenant_id, recipient_id) = (first.tenant_id, first.recipient_id);
    claimed.sort_by_key(|c| c.id);
    let ids: Vec<Uuid> = claimed.iter().map(|c| c.id).collect();
    let notifications: Vec<Uuid> = claimed.iter().map(|c| c.notification_id).collect();

    let mut tx = pool.begin().await?;
    scope(&mut tx, tenant_id).await?;
    let activity: Vec<Activity> = sqlx::query_as(
        "SELECT id, kind, issue_key, issue_summary, actor_name, detail, read_at IS NOT NULL AS read \
         FROM notifications WHERE tenant_id = $1 AND id = ANY($2) ORDER BY created_at, id",
    )
    .bind(tenant_id)
    .bind(&notifications)
    .fetch_all(&mut *tx)
    .await?;
    let prefs = store::preferences(&mut tx, tenant_id, recipient_id).await.map_err(|e| sqlx::Error::Protocol(e.to_string()))?;
    tx.commit().await?;

    let wanted: Vec<&Activity> = activity.iter().filter(|a| !a.read && prefs.wants_email(&a.kind)).collect();
    let wanted_rows: Vec<&Claimed> = claimed.iter().filter(|c| wanted.iter().any(|a| a.id == c.notification_id)).collect();
    let unwanted: Vec<Uuid> = claimed.iter().filter(|c| !wanted_rows.iter().any(|w| w.id == c.id)).map(|c| c.id).collect();
    if !unwanted.is_empty() {
        finish_all(pool, &unwanted, "skipped", None).await?;
    }
    let Some(latest) = wanted_rows.iter().max_by_key(|c| c.id) else { return Ok(ids.len()) };
    let send_ids: Vec<Uuid> = wanted_rows.iter().map(|c| c.id).collect();
    let to = latest.to_address.clone();
    if suppressed(settings, &to).await? {
        finish_all(pool, &send_ids, "suppressed", None).await?;
        metrics::counter!("notification_emails_total", "outcome" => "suppressed").increment(1);
        return Ok(ids.len());
    }

    let lines: Vec<ActivityLine<'_>> = wanted
        .iter()
        .filter_map(|a| {
            Some(ActivityLine {
                kind: Kind::parse(&a.kind)?,
                actor_name: &a.actor_name,
                issue_key: &a.issue_key,
                issue_summary: &a.issue_summary,
                to_status: a.detail.get("to_status").and_then(|v| v.as_str()),
            })
        })
        .collect();
    let token = latest.unsubscribe_url.as_deref().and_then(|u| u.split_once("?token=")).map(|(_, t)| t);
    let page = token.map(|t| format!("{}/unsubscribe?token={t}", settings.app_url));
    let rendered = email::render_activity_digest(&email::ActivityDigestInput {
        items: &lines,
        app_url: &settings.app_url,
        unsubscribe_url: page.as_deref(),
    });
    let attempts = wanted_rows.iter().map(|c| c.attempts).max().unwrap_or(1);
    let result = mailer
        .send(&Outgoing {
            to: &to,
            subject: &rendered.subject,
            text: &rendered.text,
            html: &rendered.html,
            unsubscribe_url: latest.unsubscribe_url.as_deref(),
        })
        .await;
    if result.is_ok() {
        metrics::counter!("notification_digest_items_total").increment(send_ids.len() as u64);
    }
    settle(pool, settings, &send_ids, attempts, &to, result).await?;
    Ok(ids.len())
}

/// Delete finished deliveries (they hold addresses and content) after a week.
pub async fn sweep(pool: &PgPool) -> sqlx::Result<u64> {
    Ok(sqlx::query("DELETE FROM email_deliveries WHERE finished_at < now() - make_interval(days => $1)")
        .bind(RETENTION_DAYS as i32)
        .execute(pool)
        .await?
        .rows_affected())
}

/// One sender loop per database.
pub fn spawn(pools: Vec<PgPool>, mailer: Arc<dyn Mailer>, settings: Settings) {
    for pool in pools {
        let mailer = Arc::clone(&mailer);
        let settings = settings.clone();
        tokio::spawn(async move {
            let mut last_sweep = std::time::Instant::now() - Duration::from_secs(3600);
            loop {
                if last_sweep.elapsed() >= Duration::from_secs(3600) {
                    if let Err(error) = sweep(&pool).await {
                        tracing::warn!(%error, "email retention sweep failed");
                    }
                    last_sweep = std::time::Instant::now();
                }
                match run_once(&pool, mailer.as_ref(), &settings).await {
                    Ok(n) if n as i64 >= BATCH => continue,
                    Ok(_) => tokio::time::sleep(IDLE).await,
                    Err(error) => {
                        tracing::warn!(%error, "email sender pass failed");
                        tokio::time::sleep(Duration::from_secs(10)).await;
                    }
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retries_back_off() {
        let mins: Vec<i64> = (1..=6).map(|a| backoff(a).whole_minutes()).collect();
        assert_eq!(mins, [1, 2, 4, 8, 16, 32]);
    }
}
