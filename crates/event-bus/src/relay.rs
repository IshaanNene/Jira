//! Outbox relay: publishes committed events from a service database to Kafka.
//!
//! * One relay is active per database at a time (transaction-scoped advisory lock), so
//!   replicas cannot publish two events of one key out of order.
//! * Rows are read in commit order, produced concurrently (the idempotent producer keeps
//!   per-partition order), and marked published only once Kafka has acknowledged them.
//! * A row that fails holds back every later row of the same key in that batch, so a key's
//!   events are never published out of order; the held rows are retried with it.
//! * At least once: a crash between the acknowledgement and the commit republishes the
//!   batch. Consumers deduplicate by `event_id`.

use std::{collections::HashSet, time::Duration};

use futures::future::join_all;
use rdkafka::{
    message::{Header, OwnedHeaders},
    producer::{FutureProducer, FutureRecord},
    util::Timeout,
};
use sqlx::PgPool;
use uuid::Uuid;

use crate::settings::KafkaSettings;

/// Arbitrary, fixed: "one relay per database".
const RELAY_LOCK: i64 = 0x6f75_7462_6f78; // "outbox"
const BATCH: i64 = 200;
const IDLE: Duration = Duration::from_millis(250);
const MAX_BACKOFF: Duration = Duration::from_secs(10);
const RETENTION_SWEEP: Duration = Duration::from_secs(3600);

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct OutboxRow {
    pub id: Uuid,
    pub topic: String,
    pub partition_key: String,
    pub event_type: String,
    pub payload: serde_json::Value,
}

/// Which rows of a batch may be marked published, given which ones Kafka acknowledged.
/// A failure holds back every later row with the same key.
pub fn publishable(rows: &[OutboxRow], delivered: &[bool]) -> (Vec<Uuid>, Vec<Uuid>) {
    let mut blocked: HashSet<(&str, &str)> = HashSet::new();
    let (mut done, mut retry) = (Vec::new(), Vec::new());
    for (row, ok) in rows.iter().zip(delivered) {
        let key = (row.topic.as_str(), row.partition_key.as_str());
        if *ok && !blocked.contains(&key) {
            done.push(row.id);
        } else {
            blocked.insert(key);
            retry.push(row.id);
        }
    }
    (done, retry)
}

pub struct OutboxRelay {
    service: &'static str,
    producer: FutureProducer,
}

impl OutboxRelay {
    pub fn new(service: &'static str, settings: &KafkaSettings) -> anyhow::Result<Self> {
        Ok(Self { service, producer: settings.producer().create()? })
    }

    /// Publish one batch from `pool`. Returns how many rows were published; 0 also when
    /// another relay holds this database.
    pub async fn publish_batch(&self, pool: &PgPool) -> anyhow::Result<usize> {
        let mut tx = pool.begin().await?;
        let leader: bool =
            sqlx::query_scalar("SELECT pg_try_advisory_xact_lock($1)").bind(RELAY_LOCK).fetch_one(&mut *tx).await?;
        if !leader {
            return Ok(0);
        }
        let rows: Vec<OutboxRow> = sqlx::query_as(
            "SELECT id, topic, partition_key, event_type, payload FROM outbox \
             WHERE published_at IS NULL ORDER BY created_at, id LIMIT $1",
        )
        .bind(BATCH)
        .fetch_all(&mut *tx)
        .await?;
        if rows.is_empty() {
            return Ok(0);
        }

        let bodies: Vec<Vec<u8>> = rows.iter().map(|r| serde_json::to_vec(&r.payload)).collect::<Result<_, _>>()?;
        let ids: Vec<String> = rows.iter().map(|r| r.id.to_string()).collect();
        let sends = rows.iter().zip(&bodies).zip(&ids).map(|((row, body), id)| {
            let headers = OwnedHeaders::new()
                .insert(Header { key: "event_type", value: Some(row.event_type.as_str()) })
                .insert(Header { key: "event_id", value: Some(id.as_str()) });
            let record = FutureRecord::to(&row.topic).key(&row.partition_key).payload(body).headers(headers);
            self.producer.send(record, Timeout::After(Duration::from_secs(30)))
        });
        let results = join_all(sends).await;

        let mut first_error = None;
        let delivered: Vec<bool> = results
            .into_iter()
            .map(|r| match r {
                Ok(_) => true,
                Err((error, _)) => {
                    first_error.get_or_insert_with(|| error.to_string());
                    false
                }
            })
            .collect();
        let (done, retry) = publishable(&rows, &delivered);

        if !done.is_empty() {
            sqlx::query("UPDATE outbox SET published_at = now() WHERE id = ANY($1)").bind(&done).execute(&mut *tx).await?;
        }
        if !retry.is_empty() {
            let error = first_error.unwrap_or_else(|| "held back behind an earlier failure of the same key".to_owned());
            sqlx::query("UPDATE outbox SET attempts = attempts + 1, last_error = $2 WHERE id = ANY($1)")
                .bind(&retry)
                .bind(&error)
                .execute(&mut *tx)
                .await?;
            tracing::warn!(service = self.service, failed = retry.len(), %error, "outbox rows not published; will retry");
        }
        tx.commit().await?;
        metrics::counter!("outbox_published_total", "service" => self.service).increment(done.len() as u64);
        metrics::counter!("outbox_failed_total", "service" => self.service).increment(retry.len() as u64);
        if retry.is_empty() {
            Ok(done.len())
        } else {
            anyhow::bail!("{} of {} events not delivered", retry.len(), rows.len())
        }
    }

    /// Drop published events after 7 days and inbox entries after 14 (longer than topic
    /// retention, so a redelivered event is always recognised).
    pub async fn sweep(pool: &PgPool) -> anyhow::Result<()> {
        sqlx::query("DELETE FROM outbox WHERE published_at < now() - interval '7 days'").execute(pool).await?;
        sqlx::query("DELETE FROM inbox WHERE processed_at < now() - interval '14 days'").execute(pool).await?;
        let backlog: i64 = sqlx::query_scalar("SELECT count(*) FROM outbox WHERE published_at IS NULL").fetch_one(pool).await?;
        if backlog > 10_000 {
            tracing::error!(backlog, "outbox backlog is large; is Kafka reachable?");
        }
        Ok(())
    }

    async fn run(self: std::sync::Arc<Self>, pool: PgPool) {
        let mut backoff = IDLE;
        let mut last_sweep = std::time::Instant::now() - RETENTION_SWEEP;
        loop {
            if last_sweep.elapsed() >= RETENTION_SWEEP {
                if let Err(error) = Self::sweep(&pool).await {
                    tracing::warn!(%error, "outbox retention sweep failed");
                }
                last_sweep = std::time::Instant::now();
            }
            match self.publish_batch(&pool).await {
                Ok(n) if n as i64 == BATCH => {
                    backoff = IDLE;
                    continue;
                }
                Ok(_) => {
                    backoff = IDLE;
                    tokio::time::sleep(IDLE).await;
                }
                Err(error) => {
                    tracing::warn!(service = self.service, %error, retry_in_ms = backoff.as_millis() as u64, "outbox relay failed");
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(MAX_BACKOFF);
                }
            }
        }
    }
}

/// Start the relay for each of this service's databases, according to `OUTBOX_RELAY`:
/// `kafka` (the default when `KAFKA_BROKERS` is set) or `hold` (events stay in the outbox).
pub fn spawn_relay(service: &'static str, pools: Vec<PgPool>) -> anyhow::Result<()> {
    let settings = KafkaSettings::from_env(service);
    let mode = std::env::var("OUTBOX_RELAY").unwrap_or_else(|_| if settings.is_some() { "kafka".into() } else { "hold".into() });
    match (mode.as_str(), settings) {
        ("hold", _) => {
            tracing::info!("outbox relay disabled (OUTBOX_RELAY=hold); events stay in the outbox");
            Ok(())
        }
        ("kafka", Some(settings)) => {
            let relay = std::sync::Arc::new(OutboxRelay::new(service, &settings)?);
            tracing::info!(brokers = %settings.brokers, databases = pools.len(), "outbox relay started");
            for pool in pools {
                tokio::spawn(std::sync::Arc::clone(&relay).run(pool));
            }
            Ok(())
        }
        ("kafka", None) => anyhow::bail!("OUTBOX_RELAY=kafka needs KAFKA_BROKERS"),
        (other, _) => anyhow::bail!("unknown OUTBOX_RELAY mode {other:?} (expected kafka or hold)"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(key: &str) -> OutboxRow {
        OutboxRow {
            id: Uuid::now_v7(),
            topic: "t".into(),
            partition_key: key.into(),
            event_type: "x".into(),
            payload: serde_json::Value::Null,
        }
    }

    #[test]
    fn a_failure_holds_back_later_events_of_the_same_key_only() {
        let rows = [row("a"), row("b"), row("a"), row("b"), row("c")];
        let (done, retry) = publishable(&rows, &[false, true, true, true, true]);
        assert_eq!(done, vec![rows[1].id, rows[3].id, rows[4].id]);
        assert_eq!(retry, vec![rows[0].id, rows[2].id]);
    }

    #[test]
    fn everything_delivered_is_published() {
        let rows = [row("a"), row("a")];
        let (done, retry) = publishable(&rows, &[true, true]);
        assert_eq!(done.len(), 2);
        assert!(retry.is_empty());
    }
}
