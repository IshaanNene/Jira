//! The ingester: turns issue, sprint and project events into ClickHouse rows. Every write is
//! idempotent (events are keyed by event id, issue state by version), so redelivery and
//! replaying a topic from the start are both safe.

use std::{
    collections::{BTreeSet, HashMap},
    sync::Arc,
};

use api_contracts::issue::v1::Document;
use async_trait::async_trait;
use event_bus::{BatchHandler, Event, Handler, HandlerError};
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

#[derive(Debug, Clone, Serialize)]
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

/// What one event asks of the store, decided before anything is written.
enum Step {
    /// Write the issue's current state; `record` also logs the event (issue.created).
    Refresh {
        tenant_id: Uuid,
        issue_id: Uuid,
        project_id: Uuid,
        record: Option<EventRow>,
    },
    /// Write the current state and log the move. Without a category in the event, the current
    /// state supplies the category of the status the issue is still in.
    Transition {
        tenant_id: Uuid,
        row: EventRow,
        category_known: bool,
    },
    Delete(StateRow),
    Record(EventRow),
    DeleteProject {
        tenant_id: Uuid,
        project_id: Uuid,
    },
    Ignore,
}

impl Ingester {
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

    fn plan(event: &Event) -> Result<Step, HandlerError> {
        let tenant_id = event.tenant_id()?;
        Ok(match event.event_type() {
            "issue.created" | "issue.updated" | "issue.assigned" => {
                let AboutIssue { issue_id, project_id } = event.payload()?;
                let record = (event.event_type() == "issue.created")
                    .then(|| Self::base(event, tenant_id).map(|b| EventRow { project_id, issue_id, ..b }))
                    .transpose()?;
                Step::Refresh { tenant_id, issue_id, project_id, record }
            }
            "issue.transitioned" => {
                let t: IssueTransitionedV1 = event.payload()?;
                let category_known = t.to_category.is_some();
                Step::Transition {
                    tenant_id,
                    category_known,
                    row: EventRow {
                        project_id: t.project_id,
                        issue_id: t.issue_id,
                        from_status: t.from_status,
                        to_status: t.to_status,
                        from_category: t.from_category.unwrap_or_default(),
                        to_category: t.to_category.unwrap_or_default(),
                        ..Self::base(event, tenant_id)?
                    },
                }
            }
            "issue.deleted" => {
                let AboutIssue { issue_id, project_id } = event.payload()?;
                Step::Delete(StateRow::deleted(tenant_id, issue_id, project_id))
            }
            "sprint.started" => {
                let s: SprintStartedV1 = event.payload()?;
                Step::Record(EventRow {
                    project_id: s.project_id,
                    sprint_id: s.sprint_id,
                    name: s.name,
                    completed: s.issue_count,
                    ..Self::base(event, tenant_id)?
                })
            }
            "sprint.completed" => {
                let s: SprintCompletedV1 = event.payload()?;
                Step::Record(EventRow {
                    project_id: s.project_id,
                    sprint_id: s.sprint_id,
                    name: s.name,
                    completed: s.completed_issues,
                    incomplete: s.incomplete_issues,
                    ..Self::base(event, tenant_id)?
                })
            }
            "project.deleted" => {
                let AboutProject { project_id } = event.payload()?;
                Step::DeleteProject { tenant_id, project_id }
            }
            _ => Step::Ignore,
        })
    }

    /// The current state of every issue the steps refresh, fetched in one call per tenant.
    /// `None` for an issue that no longer exists; `Err` for a document with malformed ids.
    async fn current(
        &self,
        steps: &[(usize, Step)],
    ) -> Result<HashMap<(Uuid, Uuid), Result<Option<StateRow>, ()>>, anyhow::Error> {
        let mut wanted: HashMap<Uuid, BTreeSet<Uuid>> = HashMap::new();
        for (_, step) in steps {
            match step {
                Step::Refresh { tenant_id, issue_id, .. } => {
                    wanted.entry(*tenant_id).or_default().insert(*issue_id);
                }
                Step::Transition { tenant_id, row, .. } => {
                    wanted.entry(*tenant_id).or_default().insert(row.issue_id);
                }
                _ => {}
            }
        }
        let mut current = HashMap::new();
        for (tenant_id, ids) in wanted {
            let ids: Vec<Uuid> = ids.into_iter().collect();
            let documents = self.source.documents(tenant_id, &ids).await.map_err(|e| anyhow::anyhow!("{e}"))?;
            for id in &ids {
                current.insert((tenant_id, *id), Ok(None));
            }
            for document in documents {
                let id = Uuid::parse_str(&document.id).ok();
                match (id, StateRow::from_document(tenant_id, document)) {
                    (Some(id), Some(row)) => current.insert((tenant_id, id), Ok(Some(row))),
                    (Some(id), None) => current.insert((tenant_id, id), Err(())),
                    (None, _) => None,
                };
            }
        }
        Ok(current)
    }

    async fn apply(&self, events: &[Event]) -> Result<Vec<(usize, anyhow::Error)>, anyhow::Error> {
        let mut rejected = Vec::new();
        let mut steps = Vec::with_capacity(events.len());
        for (i, event) in events.iter().enumerate() {
            match Self::plan(event) {
                Ok(step) => steps.push((i, step)),
                Err(HandlerError::Permanent(error)) => rejected.push((i, error)),
                Err(HandlerError::Transient(error)) => return Err(error),
            }
        }
        let current = self.current(&steps).await?;
        let malformed = || anyhow::anyhow!("issue document with malformed ids");

        // Issue state: one row per refreshed issue (a missing issue is written as deleted).
        let mut states: HashMap<(Uuid, Uuid), StateRow> = HashMap::new();
        let mut rows = Vec::new();
        let mut projects = Vec::new();
        for (i, step) in steps {
            match step {
                Step::Refresh { tenant_id, issue_id, project_id, record } => match current.get(&(tenant_id, issue_id)) {
                    Some(Err(())) => rejected.push((i, malformed())),
                    found => {
                        let row = found
                            .and_then(|r| r.as_ref().ok().cloned().flatten())
                            .unwrap_or_else(|| StateRow::deleted(tenant_id, issue_id, project_id));
                        states.entry((tenant_id, issue_id)).or_insert(row);
                        rows.extend(record);
                    }
                },
                Step::Transition { tenant_id, mut row, category_known } => match current.get(&(tenant_id, row.issue_id)) {
                    Some(Err(())) => rejected.push((i, malformed())),
                    found => {
                        let now = found.and_then(|r| r.as_ref().ok().cloned().flatten());
                        if !category_known {
                            row.to_category = now
                                .as_ref()
                                .filter(|c| c.status == row.to_status)
                                .map(|c| c.status_category.clone())
                                .unwrap_or_default();
                        }
                        let state = now.unwrap_or_else(|| StateRow::deleted(tenant_id, row.issue_id, row.project_id));
                        states.entry((tenant_id, row.issue_id)).or_insert(state);
                        rows.push(row);
                    }
                },
                // A deletion outranks every version, so it wins whatever else the batch holds.
                Step::Delete(row) => {
                    states.insert((row.tenant_id, row.issue_id), row);
                }
                Step::Record(row) => rows.push(row),
                Step::DeleteProject { tenant_id, project_id } => projects.push((tenant_id, project_id)),
                Step::Ignore => {}
            }
        }
        let states: Vec<StateRow> = states.into_values().collect();
        self.store.insert("issue_state", &states).await?;
        self.store.insert("issue_events", &rows).await?;
        // After this batch's state rows, so none of them can outlive the project.
        for (tenant_id, project_id) in projects {
            let sql = "INSERT INTO issue_state \
                SELECT tenant_id, issue_id, project_id, '', '', '', '', '', '', created_at, NULL, updated_at, \
                       9223372036854775807, 1 \
                FROM issue_state FINAL WHERE tenant_id = {tenant:UUID} AND project_id = {project:UUID}";
            let params = Params::new().add("tenant", tenant_id).add("project", project_id);
            self.store.command(sql, &params).await?;
        }
        Ok(rejected)
    }
}

#[async_trait]
impl BatchHandler for Ingester {
    async fn handle_batch(&self, events: &[Event]) -> Result<Vec<(usize, anyhow::Error)>, anyhow::Error> {
        self.apply(events).await
    }
}

/// One event at a time: a batch of one.
#[async_trait]
impl Handler for Ingester {
    async fn handle(&self, event: &Event) -> Result<(), HandlerError> {
        match self.apply(std::slice::from_ref(event)).await {
            Ok(rejected) => rejected.into_iter().next().map_or(Ok(()), |(_, e)| Err(HandlerError::Permanent(e))),
            Err(error) => Err(HandlerError::Transient(error)),
        }
    }
}
