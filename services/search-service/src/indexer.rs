//! Keeps the index in step with issue-service: on each issue or comment event the issue's
//! current document is fetched and written; deleted issues and projects are removed.

use std::sync::Arc;

use async_trait::async_trait;
use event_bus::{Event, Handler, HandlerError};
use serde::Deserialize;
use uuid::Uuid;

use crate::{
    index::{Index, IssueDoc},
    source::{Source, DOCUMENT_BATCH, ID_PAGE},
};

pub const CONSUMER: &str = "search-service";

pub struct Indexer {
    pub index: Index,
    pub source: Arc<dyn Source>,
}

#[derive(Deserialize)]
struct AboutIssue {
    issue_id: Uuid,
}

#[derive(Deserialize)]
struct AboutProject {
    project_id: Uuid,
}

impl Indexer {
    /// Bring these issues of the tenant up to date in the index.
    pub async fn sync(&self, tenant_id: Uuid, ids: &[Uuid]) -> Result<(), HandlerError> {
        for chunk in ids.chunks(DOCUMENT_BATCH) {
            let fetched = self.source.documents(tenant_id, chunk).await?;
            let docs: Vec<IssueDoc> =
                fetched.documents.into_iter().filter_map(|d| IssueDoc::from_document(tenant_id, d)).collect();
            if docs.len() == 1 {
                self.index.put(&docs[0]).await?;
            } else {
                self.index.put_many(&docs).await?;
            }
            for gone in fetched.missing {
                self.index.remove(tenant_id, gone).await?;
            }
        }
        Ok(())
    }

    /// Rebuild the tenant's part of the index from issue-service. Returns the number of
    /// issues indexed. Safe to run while the consumer is live.
    pub async fn reindex(&self, tenant_id: Uuid) -> Result<usize, HandlerError> {
        // Anything not rewritten during this pass no longer exists.
        let started = time::OffsetDateTime::now_utc() - time::Duration::seconds(1);
        let started =
            started.format(&time::format_description::well_known::Rfc3339).map_err(|e| HandlerError::Permanent(e.into()))?;
        let mut after = None;
        let mut total = 0;
        loop {
            let ids = self.source.issue_ids(tenant_id, after, ID_PAGE).await?;
            let Some(last) = ids.last().copied() else { break };
            self.sync(tenant_id, &ids).await?;
            total += ids.len();
            tracing::info!(%tenant_id, total, "reindexing");
            after = Some(last);
            if ids.len() < ID_PAGE as usize {
                break;
            }
        }
        let removed = self.index.remove_older_than(tenant_id, &started).await?;
        tracing::info!(%tenant_id, total, removed, "reindex finished");
        Ok(total)
    }
}

#[async_trait]
impl Handler for Indexer {
    async fn handle(&self, event: &Event) -> Result<(), HandlerError> {
        let event_type = event.event_type();
        let tenant_id = event.tenant_id()?;
        if event_type == "project.deleted" {
            let AboutProject { project_id } = event.payload()?;
            self.index.remove_project(tenant_id, project_id).await?;
            return Ok(());
        }
        if !(event_type.starts_with("issue.") || event_type.starts_with("comment.")) {
            return Ok(());
        }
        let AboutIssue { issue_id } = event.payload()?;
        if event_type == "issue.deleted" {
            self.index.remove(tenant_id, issue_id).await?;
        } else {
            // Always the current state, so replays and reordering converge.
            self.sync(tenant_id, &[issue_id]).await?;
        }
        Ok(())
    }
}
