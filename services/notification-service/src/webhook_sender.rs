//! Drains the webhook delivery queue: POSTs each event, signed, and retries failures with
//! backoff. Rows are leased like email deliveries, so several instances can run.
//!
//! A webhook is turned off after `DISABLE_AFTER` deliveries in a row fail for good, or at
//! once when the endpoint answers `410 Gone`. Endpoint URLs and secrets are never logged.

use std::{sync::Arc, time::Duration};

use reqwest::{redirect, StatusCode, Url};
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::webhooks::{self, Reach, SecretBox};

const BATCH: i64 = 20;
const LEASE_SECONDS: i64 = 120;
pub const MAX_ATTEMPTS: i32 = 6;
pub const DISABLE_AFTER: i32 = 15;
const TIMEOUT: Duration = Duration::from_secs(10);
const IDLE: Duration = Duration::from_secs(2);
const RETENTION_DAYS: i32 = 7;
/// Kept in the delivery log: enough to diagnose, never the whole response.
const ERROR_LIMIT: usize = 300;

#[derive(Debug, sqlx::FromRow)]
struct Claimed {
    id: Uuid,
    tenant_id: Uuid,
    webhook_id: Uuid,
    event_type: String,
    body: serde_json::Value,
    attempts: i32,
}

/// Wait before retry `attempt` (1-based): 1, 2, 4, 8, 16 minutes.
pub fn backoff(attempt: i32) -> time::Duration {
    time::Duration::minutes(1i64 << (attempt.clamp(1, 6) - 1))
}

enum Outcome {
    Delivered(u16),
    /// Worth trying again later.
    Retry(Option<u16>, String),
    /// The endpoint says it is gone for good.
    Gone,
}

fn truncate(mut s: String) -> String {
    if s.len() > ERROR_LIMIT {
        let mut end = ERROR_LIMIT;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        s.truncate(end);
    }
    s
}

/// Where and how a delivery goes.
struct Target {
    slack: bool,
    url: String,
    /// The signing secret; empty for Slack, whose URL is the credential.
    secret: String,
}

async fn post(target: &Target, delivery: &Claimed, reach: Reach) -> Outcome {
    let (url, secret) = (target.url.as_str(), target.secret.as_str());
    let Ok(url) = Url::parse(url) else { return Outcome::Retry(None, "The URL is not valid.".into()) };
    let addr = match webhooks::resolve(&url, reach).await {
        Ok(addr) => addr,
        Err(why) => return Outcome::Retry(None, why),
    };
    let host = url.host_str().unwrap_or_default().trim_start_matches('[').trim_end_matches(']').to_owned();
    let client = match reqwest::Client::builder()
        .redirect(redirect::Policy::none())
        .timeout(TIMEOUT)
        .connect_timeout(Duration::from_secs(5))
        .user_agent("Meridian-Webhooks/1")
        // Connect to the address that was checked, whatever DNS says next.
        .resolve(&host, addr)
        .build()
    {
        Ok(c) => c,
        Err(e) => return Outcome::Retry(None, truncate(e.to_string())),
    };
    let body = serde_json::to_vec(&delivery.body).unwrap_or_default();
    let now = OffsetDateTime::now_utc().unix_timestamp();
    let mut request = client.post(url).header("content-type", "application/json");
    if !target.slack {
        request = request
            .header("x-meridian-event", &delivery.event_type)
            .header("x-meridian-delivery", delivery.id.to_string())
            .header("x-meridian-signature", webhooks::signature(secret, now, &body));
    }
    let result = request.body(body).send().await;
    match result {
        Ok(resp) if resp.status().is_success() => Outcome::Delivered(resp.status().as_u16()),
        Ok(resp) if resp.status() == StatusCode::GONE => Outcome::Gone,
        // Slack answers 404 (`no_service`) for an incoming webhook that was removed.
        Ok(resp) if target.slack && resp.status() == StatusCode::NOT_FOUND => Outcome::Gone,
        Ok(resp) if resp.status().is_redirection() => {
            Outcome::Retry(Some(resp.status().as_u16()), "The endpoint redirected; redirects are not followed.".into())
        }
        Ok(resp) => Outcome::Retry(Some(resp.status().as_u16()), format!("The endpoint answered {}.", resp.status())),
        Err(e) if e.is_timeout() => Outcome::Retry(None, "The endpoint did not answer within 10 seconds.".into()),
        Err(e) if e.is_connect() => Outcome::Retry(None, "Could not connect to the endpoint.".into()),
        Err(_) => Outcome::Retry(None, "The request failed.".into()),
    }
}

async fn finish(pool: &PgPool, id: Uuid, status: &str, response: Option<u16>, error: Option<&str>) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE webhook_deliveries SET status = $2, response_status = $3, last_error = $4, finished_at = now() WHERE id = $1",
    )
    .bind(id)
    .bind(status)
    .bind(response.map(i32::from))
    .bind(error)
    .execute(pool)
    .await?;
    Ok(())
}

/// Record how a webhook is doing, under its tenant's row-level security; turn it off when
/// it keeps failing (or is gone).
async fn health(pool: &PgPool, tenant_id: Uuid, webhook_id: Uuid, success: bool, gone: bool) -> sqlx::Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT set_config('app.tenant_id', $1, true)").bind(tenant_id.to_string()).execute(&mut *tx).await?;
    if success {
        sqlx::query("UPDATE webhooks SET consecutive_failures = 0 WHERE tenant_id = $1 AND id = $2")
            .bind(tenant_id)
            .bind(webhook_id)
            .execute(&mut *tx)
            .await?;
    } else {
        let reason = if gone {
            "The endpoint answered 410 Gone.".to_owned()
        } else {
            format!("Turned off after {DISABLE_AFTER} deliveries in a row failed.")
        };
        sqlx::query(
            "UPDATE webhooks SET consecutive_failures = consecutive_failures + 1, \
                    enabled = CASE WHEN $3 OR consecutive_failures + 1 >= $4 THEN false ELSE enabled END, \
                    disabled_reason = CASE WHEN enabled AND ($3 OR consecutive_failures + 1 >= $4) THEN $5 ELSE disabled_reason END \
             WHERE tenant_id = $1 AND id = $2",
        )
        .bind(tenant_id)
        .bind(webhook_id)
        .bind(gone)
        .bind(DISABLE_AFTER)
        .bind(reason)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await
}

/// Where the webhook delivers, if it is still on.
async fn target(pool: &PgPool, secrets: &SecretBox, tenant_id: Uuid, webhook_id: Uuid) -> sqlx::Result<Option<Target>> {
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT set_config('app.tenant_id', $1, true)").bind(tenant_id.to_string()).execute(&mut *tx).await?;
    let hook = webhooks::get(&mut tx, tenant_id, webhook_id).await?;
    tx.commit().await?;
    Ok(hook.filter(|h| h.enabled).and_then(|h| {
        let sealed = secrets.open(&h.secret_sealed)?;
        Some(if h.kind == webhooks::Kind::Slack.as_str() {
            Target { slack: true, url: sealed, secret: String::new() }
        } else {
            Target { slack: false, url: h.url, secret: sealed }
        })
    }))
}

/// Deliver what is due on this database; returns how many rows were handled.
pub async fn run_once(pool: &PgPool, secrets: &SecretBox, reach: Reach) -> sqlx::Result<usize> {
    let claimed: Vec<Claimed> = sqlx::query_as(
        "UPDATE webhook_deliveries SET status = 'sending', attempts = attempts + 1, \
                send_after = now() + make_interval(secs => $2) \
         WHERE id IN (SELECT id FROM webhook_deliveries WHERE status IN ('pending', 'sending') AND send_after <= now() \
                      ORDER BY send_after LIMIT $1 FOR UPDATE SKIP LOCKED) \
         RETURNING id, tenant_id, webhook_id, event_type, body, attempts",
    )
    .bind(BATCH)
    .bind(LEASE_SECONDS as f64)
    .fetch_all(pool)
    .await?;

    for d in &claimed {
        let Some(target) = target(pool, secrets, d.tenant_id, d.webhook_id).await? else {
            finish(pool, d.id, "skipped", None, Some("The webhook was turned off.")).await?;
            continue;
        };
        match post(&target, d, reach).await {
            Outcome::Delivered(status) => {
                finish(pool, d.id, "delivered", Some(status), None).await?;
                health(pool, d.tenant_id, d.webhook_id, true, false).await?;
                metrics::counter!("webhook_deliveries_total", "outcome" => "delivered").increment(1);
            }
            Outcome::Gone => {
                finish(pool, d.id, "failed", Some(410), Some("The endpoint answered 410 Gone.")).await?;
                health(pool, d.tenant_id, d.webhook_id, false, true).await?;
                metrics::counter!("webhook_deliveries_total", "outcome" => "failed").increment(1);
            }
            Outcome::Retry(status, error) if d.attempts < MAX_ATTEMPTS => {
                tracing::info!(delivery = %d.id, attempts = d.attempts, "webhook not delivered; will retry");
                sqlx::query(
                    "UPDATE webhook_deliveries SET status = 'pending', send_after = $2, response_status = $3, last_error = $4 WHERE id = $1",
                )
                .bind(d.id)
                .bind(OffsetDateTime::now_utc() + backoff(d.attempts))
                .bind(status.map(i32::from))
                .bind(truncate(error))
                .execute(pool)
                .await?;
            }
            Outcome::Retry(status, error) => {
                tracing::warn!(delivery = %d.id, attempts = d.attempts, "webhook delivery failed for good");
                finish(pool, d.id, "failed", status, Some(&truncate(error))).await?;
                health(pool, d.tenant_id, d.webhook_id, false, false).await?;
                metrics::counter!("webhook_deliveries_total", "outcome" => "failed").increment(1);
            }
        }
    }
    Ok(claimed.len())
}

/// Delete finished deliveries after a week.
pub async fn sweep(pool: &PgPool) -> sqlx::Result<u64> {
    Ok(sqlx::query("DELETE FROM webhook_deliveries WHERE finished_at < now() - make_interval(days => $1)")
        .bind(RETENTION_DAYS)
        .execute(pool)
        .await?
        .rows_affected())
}

/// One delivery loop per database.
pub fn spawn(pools: Vec<PgPool>, settings: Arc<webhooks::Settings>) {
    for pool in pools {
        let settings = Arc::clone(&settings);
        tokio::spawn(async move {
            let mut last_sweep = std::time::Instant::now() - Duration::from_secs(3600);
            loop {
                if last_sweep.elapsed() >= Duration::from_secs(3600) {
                    if let Err(error) = sweep(&pool).await {
                        tracing::warn!(%error, "webhook retention sweep failed");
                    }
                    last_sweep = std::time::Instant::now();
                }
                match run_once(&pool, &settings.secrets, settings.reach).await {
                    Ok(n) if n as i64 == BATCH => continue,
                    Ok(_) => tokio::time::sleep(IDLE).await,
                    Err(error) => {
                        tracing::warn!(%error, "webhook sender pass failed");
                        tokio::time::sleep(Duration::from_secs(10)).await;
                    }
                }
            }
        });
    }
}
