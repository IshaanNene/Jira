//! Keeps the email address in people's profiles in step with their account. identity-service owns
//! the address; this copy is what member lists, invitations and other services' emails read.

use std::sync::Arc;

use async_trait::async_trait;
use event_bus::{Event, Handler, HandlerError};
use event_contracts::payloads::EmailChangedV1;
use sqlx::PgPool;
use tenant_context::DbRouter;
use uuid::Uuid;

pub const CONSUMER: &str = "user-service.profiles";

/// Apply a change of address. Only over the address it replaced, so a replayed or out-of-date
/// event changes nothing. Returns whether the profile changed.
pub async fn apply_email_change(pool: &PgPool, change: &EmailChangedV1) -> Result<bool, sqlx::Error> {
    let done = sqlx::query(
        "UPDATE user_profiles SET email = $2, version = version + 1, updated_at = now() WHERE user_id = $1 AND email = $3",
    )
    .bind(change.user_id)
    .bind(&change.email)
    .bind(&change.previous_email)
    .execute(pool)
    .await?;
    Ok(done.rows_affected() > 0)
}

/// Kafka consumer of identity events.
pub struct ProfileSync {
    pub db: Arc<DbRouter>,
}

#[async_trait]
impl Handler for ProfileSync {
    async fn handle(&self, event: &Event) -> Result<(), HandlerError> {
        if event.event_type() != "identity.email_changed" {
            return Ok(());
        }
        let change: EmailChangedV1 = event.payload()?;
        let env = &event.envelope;
        // Account events carry no tenant and are about the account they name.
        if env.tenant_id.is_some() || env.aggregate_type != "account" || env.aggregate_id != change.user_id {
            return Err(HandlerError::Permanent(anyhow::anyhow!("email change event about a different aggregate")));
        }
        let changed = apply_email_change(self.db.global()?, &change).await?;
        let user_id: Uuid = change.user_id;
        tracing::info!(%user_id, changed, "profile email synchronised");
        Ok(())
    }
}
