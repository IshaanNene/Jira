//! Transactional outbox writer. An event is inserted with the same connection — and
//! therefore the same transaction — as the business change it describes.
//!
//! Every service database creates the table with [`OUTBOX_TABLE_SQL`]'s shape in its
//! first migration.

use serde::Serialize;
use sqlx::PgConnection;

use crate::envelope::{EventEnvelope, EventPayload};

/// Reference DDL (services copy this into their migrations; kept here so the shape has
/// one documented home).
pub const OUTBOX_TABLE_SQL: &str = r"
CREATE TABLE outbox (
    id            uuid PRIMARY KEY,
    tenant_id     uuid,
    topic         text NOT NULL,
    partition_key text NOT NULL,
    event_type    text NOT NULL,
    payload       jsonb NOT NULL,
    created_at    timestamptz NOT NULL DEFAULT now(),
    published_at  timestamptz,
    attempts      integer NOT NULL DEFAULT 0,
    last_error    text
);
CREATE INDEX outbox_unpublished_idx ON outbox (created_at) WHERE published_at IS NULL;
CREATE TABLE inbox (
    consumer     text NOT NULL,
    event_id     uuid NOT NULL,
    processed_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (consumer, event_id)
);
";

/// Enqueue `event` for publication. Must be called inside the business transaction.
pub async fn enqueue<P>(conn: &mut PgConnection, event: &EventEnvelope<P>) -> Result<(), sqlx::Error>
where
    P: EventPayload + Serialize,
{
    let payload = serde_json::to_value(event).map_err(|e| sqlx::Error::Encode(Box::new(e)))?;
    sqlx::query("INSERT INTO outbox (id, tenant_id, topic, partition_key, event_type, payload) VALUES ($1, $2, $3, $4, $5, $6)")
        .bind(event.event_id)
        .bind(event.tenant_id)
        .bind(event.topic())
        .bind(event.partition_key())
        .bind(&event.event_type)
        .bind(payload)
        .execute(conn)
        .await?;
    Ok(())
}
