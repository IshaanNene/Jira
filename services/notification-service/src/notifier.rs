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
    /// `None`: in-app only.
    pub email: Option<EmailSettings>,
}

#[derive(Debug, Clone)]
pub struct EmailSettings {
    /// Base URL of the web app for links, without a trailing slash.
    pub app_url: String,
    /// How long an email waits, so that reading the notification in the app first cancels it.
    pub delay: Duration,
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
                send.push((person, kind));
            }
        }

        let mut tx = self.db.begin(tenant_id).await?;
        if !inbox::claim(tx.conn(), CONSUMER, event.event_id()).await? {
            tx.rollback().await?;
            return Ok(());
        }
        let detail = trigger.detail();
        let recipients: Vec<Uuid> = send.iter().map(|(p, _)| p.user_id).collect();
        let preferences = store::preferences_of(tx.conn(), tenant_id, &recipients).await?;
        for (person, kind) in send {
            let user = person.user_id;
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
                let prefs = preferences.iter().find(|(u, _)| *u == user).map(|(_, p)| p.clone()).unwrap_or_default();
                if let Some(email) = &self.email {
                    if !person.email.is_empty() && prefs.wants_email(kind.as_str()) {
                        let rendered = crate::email::render(&crate::email::EmailInput {
                            kind,
                            actor_name: &actor_name,
                            issue_key: &audience.key,
                            issue_summary: &audience.summary,
                            to_status: detail.get("to_status").and_then(|v| v.as_str()),
                            app_url: &email.app_url,
                        });
                        store::enqueue_email(
                            tx.conn(),
                            tenant_id,
                            &store::NewDelivery {
                                notification_id: id,
                                recipient_id: user,
                                kind: kind.as_str(),
                                to_address: &person.email,
                                subject: &rendered.subject,
                                text_body: &rendered.text,
                                html_body: &rendered.html,
                                send_after: time::OffsetDateTime::now_utc() + email.delay,
                            },
                        )
                        .await?;
                    }
                }
            }
        }
        tx.commit().await?;
        Ok(())
    }
}
