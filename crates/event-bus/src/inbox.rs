//! Consumer-side deduplication. Kafka delivers at least once; a handler claims the event in
//! the same transaction as its side effect, so a redelivered event changes nothing.

use sqlx::PgConnection;
use uuid::Uuid;

/// `true` if this consumer has not processed `event_id` before (and now has, once the
/// surrounding transaction commits).
pub async fn claim(conn: &mut PgConnection, consumer: &str, event_id: Uuid) -> Result<bool, sqlx::Error> {
    let done = sqlx::query("INSERT INTO inbox (consumer, event_id) VALUES ($1, $2) ON CONFLICT DO NOTHING")
        .bind(consumer)
        .bind(event_id)
        .execute(conn)
        .await?;
    Ok(done.rows_affected() == 1)
}
