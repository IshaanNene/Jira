//! Internal read API: issue cards for boards and sprint reports.

use std::sync::Arc;

use api_contracts::issue::v1::{issues_server::Issues, Card, Cards, ListCardsRequest};
use error_handling::ApiError;
use tenant_context::DbRouter;
use time::{format_description::well_known::Rfc3339, OffsetDateTime};
use tonic::{Request, Response, Status};
use uuid::Uuid;

use crate::store;

/// Hard ceiling per call, whatever the caller asks for.
const MAX_CARDS: u32 = 2000;

pub struct IssuesService {
    pub db: Arc<DbRouter>,
}

fn id(field: &'static str, value: &str) -> Result<Uuid, Status> {
    Uuid::parse_str(value).map_err(|_| Status::from(ApiError::invalid(field, "invalid_uuid", "Must be a UUID.")))
}

fn card(issue: store::Issue) -> Card {
    Card {
        key: issue.key(),
        id: issue.id.to_string(),
        number: issue.number,
        r#type: issue.type_key,
        status: issue.status,
        status_category: issue.status_category,
        priority: issue.priority,
        summary: issue.summary,
        assignee_id: issue.assignee_id.map(|a| a.to_string()).unwrap_or_default(),
        story_points: issue.story_points,
        resolved_at: issue.resolved_at.and_then(|t| t.format(&Rfc3339).ok()).unwrap_or_default(),
        parent_key: match (&issue.parent_project_key, issue.parent_number) {
            (Some(key), Some(number)) => format!("{key}-{number}"),
            _ => String::new(),
        },
        parent_summary: issue.parent_summary.unwrap_or_default(),
        version: issue.version,
        labels: issue.labels,
    }
}

#[tonic::async_trait]
impl Issues for IssuesService {
    async fn list_cards(&self, request: Request<ListCardsRequest>) -> Result<Response<Cards>, Status> {
        let req = request.into_inner();
        let tenant_id = id("tenant_id", &req.tenant_id)?;
        let project_id = id("project_id", &req.project_id)?;
        let ids = req.issue_ids.iter().map(|v| id("issue_ids", v)).collect::<Result<Vec<_>, _>>()?;
        let done_since = if req.done_since.is_empty() {
            None
        } else {
            Some(OffsetDateTime::parse(&req.done_since, &Rfc3339).map_err(|_| {
                Status::from(ApiError::invalid("done_since", "invalid_timestamp", "Must be an RFC 3339 timestamp."))
            })?)
        };
        let limit = if req.limit == 0 { MAX_CARDS } else { req.limit.min(MAX_CARDS) };

        let mut tx = self.db.begin(tenant_id).await?;
        let mut rows =
            store::cards(tx.conn(), tenant_id, project_id, &ids, done_since, &req.exclude_types, i64::from(limit)).await?;
        tx.commit().await?;
        let truncated = rows.len() > limit as usize;
        rows.truncate(limit as usize);
        Ok(Response::new(Cards { cards: rows.into_iter().map(card).collect(), truncated }))
    }
}
