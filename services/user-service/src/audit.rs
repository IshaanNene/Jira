//! The workspace audit log: who did what to which resource, and when.
//!
//! Entries are recorded by a consumer from events the services already publish (only the
//! administrative ones; issue edits live in each issue's history), plus `audit.recorded`
//! events for actions no domain event describes. The tenant always comes from the
//! validated event envelope.
//!
//! Each tenant's entries form a hash chain: `hash = SHA-256(prev_hash || canonical row)`,
//! with 32 zero bytes before the first entry. Appends are serialised per tenant with an
//! advisory lock; the table is append-only (see migration 0002). [`verify`] walks a chain
//! and reports the first entry that does not match.

use std::{collections::BTreeMap, sync::Arc};

use async_trait::async_trait;
use error_handling::ApiResult;
use event_bus::{inbox, Event, Handler, HandlerError};
use event_contracts::{payloads::*, Actor};
use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::PgConnection;
use tenant_context::DbRouter;
use time::OffsetDateTime;
use uuid::Uuid;

pub const CONSUMER: &str = "user-service.audit";
const GENESIS: [u8; 32] = [0; 32];

/// What an event means for the audit log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub action: String,
    pub resource_type: String,
    pub resource_id: String,
    pub resource_label: String,
    pub details: BTreeMap<String, String>,
}

fn entry(action: impl Into<String>, resource_type: &str, resource_id: impl ToString, label: impl Into<String>) -> Entry {
    Entry {
        action: action.into(),
        resource_type: resource_type.to_owned(),
        resource_id: resource_id.to_string(),
        resource_label: label.into(),
        details: BTreeMap::new(),
    }
}

impl Entry {
    fn with(mut self, key: &str, value: impl ToString) -> Self {
        self.details.insert(key.to_owned(), value.to_string());
        self
    }
}

/// `change` values come from producers; only known ones become actions.
fn known(change: &str, allowed: &[&str]) -> Option<String> {
    allowed.contains(&change).then(|| change.to_owned())
}

fn bounded(value: &str, max: usize) -> bool {
    !value.is_empty() && value.chars().count() <= max && !value.chars().any(char::is_control)
}

/// An `audit.recorded` payload is checked before it is believed: a dotted lower-case
/// action, bounded lengths, at most 20 string details.
fn checked(p: AuditRecordedV1) -> Option<Entry> {
    let action_ok = p
        .action
        .split_once('.')
        .is_some_and(|(a, b)| [a, b].iter().all(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_lowercase() || c == '_')))
        && p.action.len() <= 64;
    let fields_ok = bounded(&p.resource_type, 40)
        && bounded(&p.resource_id, 100)
        && (p.resource_label.is_empty() || bounded(&p.resource_label, 200))
        && p.details.len() <= 20
        && p.details.iter().all(|(k, v)| bounded(k, 40) && v.chars().count() <= 200 && !v.chars().any(char::is_control));
    (action_ok && fields_ok).then_some(Entry {
        action: p.action,
        resource_type: p.resource_type,
        resource_id: p.resource_id,
        resource_label: p.resource_label,
        details: p.details,
    })
}

/// The audit entry for an event, or `None` when the event is not audited.
pub fn entry_for(event: &Event) -> Result<Option<Entry>, HandlerError> {
    Ok(match event.event_type() {
        "organization.created" => {
            let p: OrganizationCreatedV1 = event.payload()?;
            Some(entry("organization.created", "organization", p.organization_id, p.name).with("slug", p.slug))
        }
        "organization.updated" => {
            let p: OrganizationUpdatedV1 = event.payload()?;
            Some(entry("organization.renamed", "organization", p.organization_id, p.name))
        }
        "organization.two_step_changed" => {
            let p: OrganizationTwoStepChangedV1 = event.payload()?;
            let action = if p.required { "organization.two_step_required" } else { "organization.two_step_optional" };
            Some(entry(action, "organization", p.organization_id, p.name))
        }
        "user.invited" => {
            let p: UserInvitedV1 = event.payload()?;
            Some(entry("member.invited", "invitation", p.invitation_id, p.email).with("role", p.org_role))
        }
        "membership.changed" => {
            let p: MembershipChangedV1 = event.payload()?;
            known(&p.change, &["joined", "role_changed", "suspended", "reactivated", "removed", "left"]).map(|change| {
                let e = entry(format!("member.{change}"), "user", p.user_id, "");
                match p.org_role {
                    Some(role) => e.with("role", role),
                    None => e,
                }
            })
        }
        "project.created" => {
            let p: ProjectCreatedV1 = event.payload()?;
            Some(entry("project.created", "project", p.project_id, p.key).with("name", p.name).with("type", p.project_type))
        }
        "project.updated" => {
            let p: ProjectUpdatedV1 = event.payload()?;
            Some(entry("project.updated", "project", p.project_id, p.key).with("name", p.name).with("status", p.status))
        }
        "project.deleted" => {
            let p: ProjectDeletedV1 = event.payload()?;
            Some(entry("project.deleted", "project", p.project_id, p.key))
        }
        "version.released" => {
            let p: VersionReleasedV1 = event.payload()?;
            Some(entry("version.released", "version", p.version_id, p.name).with("project_id", p.project_id))
        }
        "workflow.published" => {
            let p: WorkflowPublishedV1 = event.payload()?;
            Some(entry("workflow.published", "workflow", p.workflow_id, p.name).with("version", p.version))
        }
        "workflow.updated" => {
            let p: WorkflowUpdatedV1 = event.payload()?;
            known(&p.change, &["created", "renamed", "assigned", "unassigned", "deleted"])
                .map(|change| entry(format!("workflow.{change}"), "workflow", p.workflow_id, ""))
        }
        "board.updated" => {
            let p: BoardUpdatedV1 = event.payload()?;
            known(&p.change, &["created", "renamed", "columns_changed", "deleted"])
                .map(|change| entry(format!("board.{change}"), "board", p.board_id, "").with("project_id", p.project_id))
        }
        "sprint.started" => {
            let p: SprintStartedV1 = event.payload()?;
            Some(entry("sprint.started", "sprint", p.sprint_id, p.name).with("issues", p.issue_count))
        }
        "sprint.completed" => {
            let p: SprintCompletedV1 = event.payload()?;
            Some(
                entry("sprint.completed", "sprint", p.sprint_id, p.name)
                    .with("completed", p.completed_issues)
                    .with("incomplete", p.incomplete_issues),
            )
        }
        "issue.deleted" => {
            let p: IssueDeletedV1 = event.payload()?;
            Some(entry("issue.deleted", "issue", p.issue_id, p.key).with("project_id", p.project_id))
        }
        "comment.deleted" => {
            let p: CommentDeletedV1 = event.payload()?;
            Some(entry("comment.deleted", "issue", p.issue_id, p.key).with("comment_id", p.comment_id))
        }
        "audit.recorded" => {
            let p: AuditRecordedV1 = event.payload()?;
            match checked(p) {
                Some(e) => Some(e),
                None => return Err(HandlerError::Permanent(anyhow::anyhow!("audit.recorded payload failed validation"))),
            }
        }
        _ => None,
    })
}

/// One stored entry.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Row {
    pub tenant_id: Uuid,
    pub seq: i64,
    pub id: Uuid,
    pub event_id: Uuid,
    pub occurred_at: OffsetDateTime,
    pub actor_type: String,
    pub actor_id: String,
    pub action: String,
    pub resource_type: String,
    pub resource_id: String,
    pub resource_label: String,
    pub details: sqlx::types::Json<BTreeMap<String, String>>,
    pub request_id: Option<String>,
    pub prev_hash: Vec<u8>,
    pub hash: Vec<u8>,
}

/// The fields a hash covers, in a fixed order with sorted details.
#[derive(Serialize)]
struct Canonical<'a> {
    tenant_id: Uuid,
    seq: i64,
    id: Uuid,
    event_id: Uuid,
    occurred_at: String,
    actor_type: &'a str,
    actor_id: &'a str,
    action: &'a str,
    resource_type: &'a str,
    resource_id: &'a str,
    resource_label: &'a str,
    details: &'a BTreeMap<String, String>,
    request_id: Option<&'a str>,
}

impl Row {
    fn canonical(&self) -> Vec<u8> {
        let occurred_at = self
            .occurred_at
            .to_offset(time::UtcOffset::UTC)
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_default();
        serde_json::to_vec(&Canonical {
            tenant_id: self.tenant_id,
            seq: self.seq,
            id: self.id,
            event_id: self.event_id,
            occurred_at,
            actor_type: &self.actor_type,
            actor_id: &self.actor_id,
            action: &self.action,
            resource_type: &self.resource_type,
            resource_id: &self.resource_id,
            resource_label: &self.resource_label,
            details: &self.details.0,
            request_id: self.request_id.as_deref(),
        })
        .expect("serialising plain fields cannot fail")
    }

    pub fn expected_hash(&self) -> Vec<u8> {
        let mut h = Sha256::new();
        h.update(&self.prev_hash);
        h.update(self.canonical());
        h.finalize().to_vec()
    }
}

fn actor_parts(actor: &Actor) -> (&'static str, String) {
    match actor {
        Actor::User(id) => ("user", id.to_string()),
        Actor::System(name) => ("system", name.clone()),
        Actor::Automation(id) => ("automation", id.to_string()),
    }
}

/// PostgreSQL keeps microseconds; hash what will be read back.
fn to_micros(at: OffsetDateTime) -> OffsetDateTime {
    let at = at.to_offset(time::UtcOffset::UTC);
    at.replace_nanosecond(at.nanosecond() / 1000 * 1000).unwrap_or(at)
}

/// Append one entry to the tenant's chain. Returns `false` when the event was already
/// recorded. Call inside a tenant transaction.
pub async fn append(
    conn: &mut PgConnection,
    tenant_id: Uuid,
    event_id: Uuid,
    occurred_at: OffsetDateTime,
    actor: &Actor,
    request_id: Option<&str>,
    e: Entry,
) -> ApiResult<bool> {
    // One writer per tenant chain at a time, across instances.
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('audit:' || $1::text, 0))")
        .bind(tenant_id)
        .execute(&mut *conn)
        .await?;
    let exists: Option<(i64,)> = sqlx::query_as("SELECT seq FROM audit_log WHERE tenant_id = $1 AND event_id = $2")
        .bind(tenant_id)
        .bind(event_id)
        .fetch_optional(&mut *conn)
        .await?;
    if exists.is_some() {
        return Ok(false);
    }
    let head: Option<(i64, Vec<u8>)> =
        sqlx::query_as("SELECT seq, hash FROM audit_log WHERE tenant_id = $1 ORDER BY seq DESC LIMIT 1")
            .bind(tenant_id)
            .fetch_optional(&mut *conn)
            .await?;
    let (seq, prev_hash) = head.map_or((1, GENESIS.to_vec()), |(s, h)| (s + 1, h));
    let (actor_type, actor_id) = actor_parts(actor);
    let mut row = Row {
        tenant_id,
        seq,
        id: platform_common::new_id(),
        event_id,
        occurred_at: to_micros(occurred_at),
        actor_type: actor_type.to_owned(),
        actor_id,
        action: e.action,
        resource_type: e.resource_type,
        resource_id: e.resource_id,
        resource_label: e.resource_label,
        details: sqlx::types::Json(e.details),
        request_id: request_id.map(|r| r.chars().take(100).collect()),
        prev_hash,
        hash: vec![],
    };
    row.hash = row.expected_hash();
    sqlx::query(
        "INSERT INTO audit_log (tenant_id, seq, id, event_id, occurred_at, actor_type, actor_id, action, resource_type, \
                                resource_id, resource_label, details, request_id, prev_hash, hash) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)",
    )
    .bind(row.tenant_id)
    .bind(row.seq)
    .bind(row.id)
    .bind(row.event_id)
    .bind(row.occurred_at)
    .bind(&row.actor_type)
    .bind(&row.actor_id)
    .bind(&row.action)
    .bind(&row.resource_type)
    .bind(&row.resource_id)
    .bind(&row.resource_label)
    .bind(&row.details)
    .bind(&row.request_id)
    .bind(&row.prev_hash)
    .bind(&row.hash)
    .execute(&mut *conn)
    .await?;
    Ok(true)
}

pub const ROW_COLUMNS: &str = "tenant_id, seq, id, event_id, occurred_at, actor_type, actor_id, action, resource_type, \
     resource_id, resource_label, details, request_id, prev_hash, hash";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verification {
    pub entries: i64,
    /// The first entry that does not fit the chain, and why.
    pub broken: Option<(i64, &'static str)>,
}

/// Walk the tenant's whole chain. Call inside a tenant transaction.
pub async fn verify(conn: &mut PgConnection, tenant_id: Uuid) -> ApiResult<Verification> {
    const BATCH: i64 = 1000;
    let mut prev = GENESIS.to_vec();
    let mut expected_seq = 1;
    loop {
        let rows: Vec<Row> = sqlx::query_as(&format!(
            "SELECT {ROW_COLUMNS} FROM audit_log WHERE tenant_id = $1 AND seq >= $2 ORDER BY seq LIMIT $3"
        ))
        .bind(tenant_id)
        .bind(expected_seq)
        .bind(BATCH)
        .fetch_all(&mut *conn)
        .await?;
        let done = (rows.len() as i64) < BATCH;
        for row in rows {
            let problem = if row.seq != expected_seq {
                Some("an entry is missing before this one")
            } else if row.prev_hash != prev {
                Some("it does not follow the entry before it")
            } else if row.hash != row.expected_hash() {
                Some("its contents were changed after it was recorded")
            } else {
                None
            };
            if let Some(reason) = problem {
                return Ok(Verification { entries: expected_seq - 1, broken: Some((expected_seq, reason)) });
            }
            prev = row.hash;
            expected_seq += 1;
        }
        if done {
            return Ok(Verification { entries: expected_seq - 1, broken: None });
        }
    }
}

/// Kafka consumer that records audited events.
pub struct Recorder {
    pub db: Arc<DbRouter>,
}

#[async_trait]
impl Handler for Recorder {
    async fn handle(&self, event: &Event) -> Result<(), HandlerError> {
        let Some(entry) = entry_for(event)? else { return Ok(()) };
        let tenant_id = event.tenant_id()?;
        let env = &event.envelope;
        let mut tx = self.db.begin(tenant_id).await?;
        if !inbox::claim(tx.conn(), CONSUMER, event.event_id()).await? {
            tx.rollback().await?;
            return Ok(());
        }
        append(tx.conn(), tenant_id, env.event_id, env.occurred_at, &env.actor, env.correlation_id.as_deref(), entry).await?;
        tx.commit().await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checked_action(action: &str) -> bool {
        checked(AuditRecordedV1 {
            action: action.into(),
            resource_type: "project".into(),
            resource_id: "x".into(),
            resource_label: String::new(),
            details: BTreeMap::new(),
        })
        .is_some()
    }

    #[test]
    fn recorded_actions_are_validated() {
        assert!(checked_action("project.access_granted"));
        for bad in ["project", "Project.x", "a.b.c", "a.", ".b", "a.b\n", &format!("a.{}", "b".repeat(70))] {
            assert!(!checked_action(bad), "{bad:?}");
        }
        let mut details = BTreeMap::new();
        details.insert("note".to_owned(), "line\nbreak".to_owned());
        let p = AuditRecordedV1 {
            action: "a.b".into(),
            resource_type: "project".into(),
            resource_id: "x".into(),
            resource_label: String::new(),
            details,
        };
        assert!(checked(p).is_none());
    }

    #[test]
    fn hashes_cover_every_field() {
        let mut row = Row {
            tenant_id: Uuid::from_u128(1),
            seq: 1,
            id: Uuid::from_u128(2),
            event_id: Uuid::from_u128(3),
            occurred_at: to_micros(OffsetDateTime::now_utc()),
            actor_type: "user".into(),
            actor_id: "u".into(),
            action: "project.deleted".into(),
            resource_type: "project".into(),
            resource_id: "p".into(),
            resource_label: "PAY".into(),
            details: sqlx::types::Json(BTreeMap::new()),
            request_id: None,
            prev_hash: GENESIS.to_vec(),
            hash: vec![],
        };
        let original = row.expected_hash();
        row.resource_label = "HR".into();
        assert_ne!(row.expected_hash(), original);
        row.resource_label = "PAY".into();
        row.prev_hash = vec![1; 32];
        assert_ne!(row.expected_hash(), original);
        row.prev_hash = GENESIS.to_vec();
        assert_eq!(row.expected_hash(), original);
    }
}
