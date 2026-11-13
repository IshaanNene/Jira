//! Consumer: turns issue events into notifications for the people who should hear about
//! them and can still see the issue.

use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use auth_middleware::{OrgContext, PolicyClient};
use event_bus::{inbox, Event, Handler, HandlerError};
use event_contracts::{outbox, payloads::NotificationCreatedV1, Actor, EventEnvelope};
use tenant_context::DbRouter;
use uuid::Uuid;

use crate::{directory::Directory, domain, store};

pub const CONSUMER: &str = "notification-service";
/// Older events are not worth telling anyone about (e.g. history replayed on first start).
pub const STALE_AFTER: Duration = Duration::from_secs(24 * 3600);

pub struct Notifier {
    pub db: Arc<DbRouter>,
    pub directory: Arc<dyn Directory>,
    pub policy: Arc<PolicyClient>,
}

impl Notifier {
    /// Whether `user` (with this workspace role) may browse the project now.
    async fn can_see(&self, tenant_id: Uuid, person: &crate::directory::Person, project_id: Uuid) -> Result<bool, HandlerError> {
        let ctx = OrgContext {
            user_id: person.user_id,
            session_id: Uuid::nil(),
            tenant_id,
            organization_id: Uuid::nil(),
            role: person.role,
            email: String::new(),
            name: String::new(),
        };
        let snapshot = self.policy.snapshot(&ctx).await?;
        Ok(snapshot.project_role(&ctx, project_id).is_some())
    }
}

#[async_trait]
impl Handler for Notifier {
    async fn handle(&self, event: &Event) -> Result<(), HandlerError> {
        let Some(trigger) = domain::trigger(event)? else { return Ok(()) };
        let age = time::OffsetDateTime::now_utc() - event.envelope.occurred_at;
        if age > STALE_AFTER {
            return Ok(());
        }
        let tenant_id = event.tenant_id()?;
        let Some(audience) = self.directory.audience(tenant_id, trigger.issue_id).await? else { return Ok(()) };
        let candidates = domain::recipients(&trigger, &audience);
        if candidates.is_empty() {
            return Ok(());
        }

        let mut ids: Vec<Uuid> = candidates.iter().map(|(u, _)| *u).collect();
        ids.extend(trigger.actor);
        let people = self.directory.people(tenant_id, &ids).await?;
        let actor_name = trigger
            .actor
            .and_then(|a| people.iter().find(|p| p.user_id == a))
            .map(|p| p.display_name.clone())
            .unwrap_or_default();

        // Only active members who can still see the project; mentions of anyone else are dropped.
        let mut send = Vec::new();
        for (user, kind) in candidates {
            let Some(person) = people.iter().find(|p| p.user_id == user && p.active) else { continue };
            if self.can_see(tenant_id, person, audience.project_id).await? {
                send.push((user, kind));
            }
        }

        let mut tx = self.db.begin(tenant_id).await?;
        if !inbox::claim(tx.conn(), CONSUMER, event.event_id()).await? {
            tx.rollback().await?;
            return Ok(());
        }
        let detail = trigger.detail();
        for (user, kind) in send {
            let id = platform_common::new_id();
            let inserted = store::insert(
                tx.conn(),
                tenant_id,
                &store::NewNotification {
                    id,
                    recipient_id: user,
                    kind: kind.as_str(),
                    project_id: audience.project_id,
                    issue_id: trigger.issue_id,
                    issue_key: &audience.key,
                    issue_summary: &audience.summary,
                    actor_id: trigger.actor,
                    actor_name: &actor_name,
                    detail: &detail,
                    source_event_id: event.event_id(),
                },
            )
            .await?;
            if inserted {
                let created = EventEnvelope::new(
                    Some(tenant_id),
                    id,
                    1,
                    Actor::System(CONSUMER.into()),
                    NotificationCreatedV1 {
                        notification_id: id,
                        recipient_id: user,
                        kind: kind.as_str().into(),
                        issue_key: audience.key.clone(),
                    },
                )
                .caused_by(event.event_id());
                outbox::enqueue(tx.conn(), &created).await?;
            }
        }
        tx.commit().await?;
        Ok(())
    }
}
