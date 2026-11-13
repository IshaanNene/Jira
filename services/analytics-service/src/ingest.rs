//! The ingester: turns issue, sprint and project events into ClickHouse rows. Every write is
//! idempotent (events are keyed by event id, issue state by version), so redelivery and
//! replaying a topic from the start are both safe.

use std::sync::Arc;

use api_contracts::issue::v1::Document;
use async_trait::async_trait;
use event_bus::{Event, Handler, HandlerError};
use event_contracts::payloads::{IssueTransitionedV1, SprintCompletedV1, SprintStartedV1};
use serde::{Deserialize, Serialize};
use time::format_description::well_known::Rfc3339;
use uuid::Uuid;

use crate::{
    clickhouse::{ClickHouse, Params},
    source::Source,
};

pub const CONSUMER: &str = "analytics-service";

pub struct Ingester {
    pub store: ClickHouse,
    pub source: Arc<dyn Source>,
}

#[derive(Debug, Default, Serialize)]
struct EventRow {
    tenant_id: Uuid,
    event_id: Uuid,
    event_type: String,
    occurred_at: String,
    project_id: Uuid,
    issue_id: Uuid,
    sprint_id: Uuid,
    actor_id: String,
    from_status: String,
    to_status: String,
    from_category: String,
    to_category: String,
    name: String,
    completed: u32,
    incomplete: u32,
}

#[derive(Debug, Serialize)]
struct StateRow {
    tenant_id: Uuid,
    issue_id: Uuid,
    project_id: Uuid,
    key: String,
    r#type: String,
    status: String,
    status_category: String,
    priority: String,
    assignee_id: String,
    created_at: String,
    resolved_at: Option<String>,
    updated_at: String,
    version: i64,
    is_deleted: u8,
}

/// A deletion outranks every real version, so nothing can bring the issue back.
const DELETED: i64 = i64::MAX;

#[derive(Deserialize)]
struct AboutIssue {
    issue_id: Uuid,
    project_id: Uuid,
}

#[derive(Deserialize)]
struct AboutProject {
    project_id: Uuid,
}

fn transient(error: anyhow::Error) -> HandlerError {
    HandlerError::Transient(error)
}

impl StateRow {
    fn from_document(tenant_id: Uuid, d: Document) -> Option<Self> {
        let opt = |s: String| (!s.is_empty()).then_some(s);
        Some(Self {
            tenant_id,
            issue_id: Uuid::parse_str(&d.id).ok()?,
            project_id: Uuid::parse_str(&d.project_id).ok()?,
            key: d.key,
            r#type: d.r#type,
            status: d.status,
            status_category: d.status_category,
            priority: d.priority,
            assignee_id: d.assignee_id,
            created_at: d.created_at,
            resolved_at: opt(d.resolved_at),
            updated_at: d.updated_at,
            version: d.version,
            is_deleted: 0,
        })
    }

    fn deleted(tenant_id: Uuid, issue_id: Uuid, project_id: Uuid) -> Self {
        let epoch = "1970-01-01T00:00:00Z".to_owned();
        Self {
            tenant_id,
            issue_id,
            project_id,
            key: String::new(),
            r#type: String::new(),
            status: String::new(),
            status_category: String::new(),
            priority: String::new(),
            assignee_id: String::new(),
            created_at: epoch.clone(),
            resolved_at: None,
            updated_at: epoch,
            version: DELETED,
            is_deleted: 1,
        }
    }
}

impl Ingester {
    /// Write the issue's current state. Returns it, for events that need its category.
    async fn refresh(&self, tenant_id: Uuid, issue_id: Uuid, project_id: Uuid) -> Result<Option<StateRow>, HandlerError> {
        let row = match self.source.document(tenant_id, issue_id).await? {
            Some(doc) => StateRow::from_document(tenant_id, doc)
                .ok_or_else(|| HandlerError::Permanent(anyhow::anyhow!("issue document with malformed ids")))?,
            None => StateRow::deleted(tenant_id, issue_id, project_id),
        };
        self.store.insert("issue_state", std::slice::from_ref(&row)).await.map_err(transient)?;
        Ok((row.is_deleted == 0).then_some(row))
    }

    fn base(event: &Event, tenant_id: Uuid) -> Result<EventRow, HandlerError> {
        Ok(EventRow {
            tenant_id,
            event_id: event.event_id(),
            event_type: event.event_type().to_owned(),
            occurred_at: event.envelope.occurred_at.format(&Rfc3339).map_err(|e| HandlerError::Permanent(e.into()))?,
            actor_id: event.actor_user().map(|u| u.to_string()).unwrap_or_default(),
            ..EventRow::default()
        })
    }

    async fn record(&self, row: EventRow) -> Result<(), HandlerError> {
        self.store.insert("issue_events", &[row]).await.map_err(transient)
    }
}

#[async_trait]
impl Handler for Ingester {
    async fn handle(&self, event: &Event) -> Result<(), HandlerError> {
        let tenant_id = event.tenant_id()?;
        match event.event_type() {
            "issue.created" | "issue.updated" | "issue.assigned" => {
                let AboutIssue { issue_id, project_id } = event.payload()?;
                self.refresh(tenant_id, issue_id, project_id).await?;
                if event.event_type() == "issue.created" {
                    self.record(EventRow { project_id, issue_id, ..Self::base(event, tenant_id)? }).await?;
                }
            }
            "issue.transitioned" => {
                let t: IssueTransitionedV1 = event.payload()?;
                let current = self.refresh(tenant_id, t.issue_id, t.project_id).await?;
                // Events from before categories were sent: the current state knows the
                // category of the status the issue is still in.
                let known = |status: &str| {
                    current.as_ref().filter(|c| c.status == status).map(|c| c.status_category.clone()).unwrap_or_default()
                };
                let to_category = t.to_category.unwrap_or_else(|| known(&t.to_status));
                let from_category = t.from_category.unwrap_or_default();
                self.record(EventRow {
                    project_id: t.project_id,
                    issue_id: t.issue_id,
                    from_status: t.from_status,
                    to_status: t.to_status,
                    from_category,
                    to_category,
                    ..Self::base(event, tenant_id)?
                })
                .await?;
            }
            "issue.deleted" => {
                let AboutIssue { issue_id, project_id } = event.payload()?;
                let row = StateRow::deleted(tenant_id, issue_id, project_id);
                self.store.insert("issue_state", &[row]).await.map_err(transient)?;
            }
            "sprint.started" => {
                let s: SprintStartedV1 = event.payload()?;
                self.record(EventRow {
                    project_id: s.project_id,
                    sprint_id: s.sprint_id,
                    name: s.name,
                    completed: s.issue_count,
                    ..Self::base(event, tenant_id)?
                })
                .await?;
            }
            "sprint.completed" => {
                let s: SprintCompletedV1 = event.payload()?;
                self.record(EventRow {
                    project_id: s.project_id,
                    sprint_id: s.sprint_id,
                    name: s.name,
                    completed: s.completed_issues,
                    incomplete: s.incomplete_issues,
                    ..Self::base(event, tenant_id)?
                })
                .await?;
            }
            "project.deleted" => {
                let AboutProject { project_id } = event.payload()?;
                // Every issue of the project, marked deleted.
                let sql = "INSERT INTO issue_state \
                    SELECT tenant_id, issue_id, project_id, '', '', '', '', '', '', created_at, NULL, updated_at, \
                           9223372036854775807, 1 \
                    FROM issue_state FINAL WHERE tenant_id = {tenant:UUID} AND project_id = {project:UUID}";
                let params = Params::new().add("tenant", tenant_id).add("project", project_id);
                self.store.command(sql, &params).await.map_err(transient)?;
            }
            _ => {}
        }
        Ok(())
    }
}
