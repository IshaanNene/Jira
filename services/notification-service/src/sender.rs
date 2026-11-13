//! Drains the email queue. Emails wait a short while before sending so that a
//! notification read in the app in the meantime is not also emailed, and preferences
//! changed in that window are honoured.
//!
//! Rows are leased (`sending` with a lease expiry in `send_after`), so several instances
//! can run and a crashed one's rows are picked up again. Recipient addresses are never
//! logged.

use std::{sync::Arc, time::Duration};

use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    mailer::{Mailer, Outgoing, SendError},
    store,
};

const BATCH: i64 = 20;
const LEASE_SECONDS: i64 = 300;
pub const MAX_ATTEMPTS: i32 = 6;
const IDLE: Duration = Duration::from_secs(2);
const RETENTION_DAYS: i64 = 7;

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
    attempts: i32,
}

/// Wait before retry `attempt` (1-based): 1, 2, 4, 8, 16 minutes.
pub fn backoff(attempt: i32) -> time::Duration {
    time::Duration::minutes(1i64 << (attempt.clamp(1, 6) - 1))
}

/// Whether the email is still wanted: unread, and the person still wants this kind.
async fn still_wanted(pool: &PgPool, c: &Claimed) -> sqlx::Result<bool> {
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT set_config('app.tenant_id', $1, true)").bind(c.tenant_id.to_string()).execute(&mut *tx).await?;
    let unread: Option<bool> = sqlx::query_scalar("SELECT read_at IS NULL FROM notifications WHERE id = $1 AND tenant_id = $2")
        .bind(c.notification_id)
        .bind(c.tenant_id)
        .fetch_optional(&mut *tx)
        .await?;
    let prefs =
        store::preferences(&mut tx, c.tenant_id, c.recipient_id).await.map_err(|e| sqlx::Error::Protocol(e.to_string()))?;
    tx.commit().await?;
    Ok(unread == Some(true) && prefs.wants_email(&c.kind))
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

/// Send what is due on this database; returns how many rows were handled.
pub async fn run_once(pool: &PgPool, mailer: &dyn Mailer) -> sqlx::Result<usize> {
    let claimed: Vec<Claimed> = sqlx::query_as(
        "UPDATE email_deliveries SET status = 'sending', attempts = attempts + 1, \
                send_after = now() + make_interval(secs => $2) \
         WHERE id IN (SELECT id FROM email_deliveries WHERE status IN ('pending', 'sending') AND send_after <= now() \
                      ORDER BY send_after LIMIT $1 FOR UPDATE SKIP LOCKED) \
         RETURNING id, tenant_id, notification_id, recipient_id, kind, to_address, subject, text_body, html_body, attempts",
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
        let email = Outgoing { to: &c.to_address, subject: &c.subject, text: &c.text_body, html: &c.html_body };
        match mailer.send(&email).await {
            Ok(()) => {
                finish(pool, c.id, "sent", None).await?;
                metrics::counter!("notification_emails_total", "outcome" => "sent").increment(1);
            }
            Err(SendError::Transient(error)) if c.attempts < MAX_ATTEMPTS => {
                tracing::warn!(delivery = %c.id, attempts = c.attempts, "email not sent; will retry");
                sqlx::query("UPDATE email_deliveries SET status = 'pending', send_after = $2, last_error = $3 WHERE id = $1")
                    .bind(c.id)
                    .bind(OffsetDateTime::now_utc() + backoff(c.attempts))
                    .bind(error)
                    .execute(pool)
                    .await?;
            }
            Err(error) => {
                tracing::error!(delivery = %c.id, attempts = c.attempts, "email delivery failed for good");
                finish(pool, c.id, "failed", Some(&error.to_string())).await?;
                metrics::counter!("notification_emails_total", "outcome" => "failed").increment(1);
            }
        }
    }
    Ok(claimed.len())
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
pub fn spawn(pools: Vec<PgPool>, mailer: Arc<dyn Mailer>) {
    for pool in pools {
        let mailer = Arc::clone(&mailer);
        tokio::spawn(async move {
            let mut last_sweep = std::time::Instant::now() - Duration::from_secs(3600);
            loop {
                if last_sweep.elapsed() >= Duration::from_secs(3600) {
                    if let Err(error) = sweep(&pool).await {
                        tracing::warn!(%error, "email retention sweep failed");
                    }
                    last_sweep = std::time::Instant::now();
                }
                match run_once(&pool, mailer.as_ref()).await {
                    Ok(n) if n as i64 == BATCH => continue,
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
