//! Consumer: keeps boards consistent when issues or projects are deleted elsewhere.
//!
//! - `issue.deleted`: the issue's placement goes; if it was in a running sprint, the sprint
//!   records it as removed from scope so the burndown drops it. Closed sprints keep their
//!   frozen reports.
//! - `project.deleted` (a soft delete): the project's boards are marked deleted and its
//!   placements and copied cards removed. Sprints and their reports stay, unreachable without a board.

use std::sync::Arc;

use async_trait::async_trait;
use event_bus::{inbox, Event, Handler, HandlerError};
use event_contracts::payloads::{IssueDeletedV1, ProjectDeletedV1};
use tenant_context::DbRouter;
use uuid::Uuid;

use crate::store;

pub const CONSUMER: &str = "agile-service.cleanup";

pub struct Cleanup {
    pub db: Arc<DbRouter>,
}

#[async_trait]
impl Handler for Cleanup {
    async fn handle(&self, event: &Event) -> Result<(), HandlerError> {
        let kind = event.event_type();
        if kind != "issue.deleted" && kind != "project.deleted" {
            return Ok(());
        }
        let tenant_id = event.tenant_id()?;
        let actor = event.actor_user().unwrap_or(Uuid::nil());
        let mut tx = self.db.begin(tenant_id).await?;
        if !inbox::claim(tx.conn(), CONSUMER, event.event_id()).await? {
            tx.rollback().await?;
            return Ok(());
        }
        if kind == "issue.deleted" {
            let p: IssueDeletedV1 = event.payload()?;
            if let Some(sprint) = store::remove_placement(tx.conn(), tenant_id, p.issue_id).await? {
                store::log_scope(tx.conn(), tenant_id, sprint, &[p.issue_id], false, actor).await?;
            }
        } else {
            let p: ProjectDeletedV1 = event.payload()?;
            store::retire_project(tx.conn(), tenant_id, p.project_id).await?;
            crate::cards::forget_project(tx.conn(), tenant_id, p.project_id).await?;
        }
        tx.commit().await?;
        Ok(())
    }
}
