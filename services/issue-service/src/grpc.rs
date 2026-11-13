//! Internal read API for other services: board cards, notification audiences and search
//! documents. Callers authorize end users themselves; the tenant comes from the caller.

use std::sync::Arc;

use std::collections::HashMap;

use api_contracts::issue::v1::{
    issues_server::Issues, Audience, Card, Cards, Document, Documents, GetAudienceRequest, GetDocumentsRequest, IssueIds,
    ListCardsRequest, ListIssueIdsRequest,
};
use error_handling::ApiError;
use tenant_context::DbRouter;
use time::{format_description::well_known::Rfc3339, OffsetDateTime};
use tonic::{Request, Response, Status};
use uuid::Uuid;

use crate::{richtext, store};

const MAX_DOCUMENTS: usize = 500;
const MAX_IDS: u32 = 1000;

/// Hard ceiling per call, whatever the caller asks for.
const MAX_CARDS: u32 = 2000;

pub struct IssuesService {
    pub db: Arc<DbRouter>,
}

fn id(field: &'static str, value: &str) -> Result<Uuid, Status> {
    Uuid::parse_str(value).map_err(|_| Status::from(ApiError::invalid(field, "invalid_uuid", "Must be a UUID.")))
}

fn timestamp(t: OffsetDateTime) -> String {
    t.format(&Rfc3339).unwrap_or_default()
}

fn document(issue: store::Issue, comments: &[String]) -> Document {
    Document {
        key: issue.key(),
        id: issue.id.to_string(),
        project_id: issue.project_id.to_string(),
        project_key: issue.project_key,
        r#type: issue.type_key,
        status: issue.status,
        status_category: issue.status_category,
        priority: issue.priority,
        summary: issue.summary,
        description_text: issue.description.as_ref().map(richtext::plain_text).unwrap_or_default(),
        comments_text: comments.join("\n\n"),
        assignee_id: issue.assignee_id.map(|a| a.to_string()).unwrap_or_default(),
        reporter_id: issue.reporter_id.to_string(),
        labels: issue.labels,
        created_at: timestamp(issue.created_at),
        updated_at: timestamp(issue.updated_at),
        version: issue.version,
        resolved_at: issue.resolved_at.map(timestamp).unwrap_or_default(),
    }
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

    async fn get_audience(&self, request: Request<GetAudienceRequest>) -> Result<Response<Audience>, Status> {
        let req = request.into_inner();
        let tenant_id = id("tenant_id", &req.tenant_id)?;
        let issue_id = id("issue_id", &req.issue_id)?;
        let mut tx = self.db.begin(tenant_id).await?;
        let Some(issue) = store::issue_by_id(tx.conn(), tenant_id, issue_id).await? else {
            tx.commit().await?;
            return Ok(Response::new(Audience::default()));
        };
        let watchers = store::watcher_ids(tx.conn(), tenant_id, issue_id).await?;
        tx.commit().await?;
        Ok(Response::new(Audience {
            found: true,
            key: issue.key(),
            summary: issue.summary,
            project_id: issue.project_id.to_string(),
            reporter_id: issue.reporter_id.to_string(),
            assignee_id: issue.assignee_id.map(|a| a.to_string()).unwrap_or_default(),
            watcher_ids: watchers.iter().map(Uuid::to_string).collect(),
        }))
    }

    async fn get_documents(&self, request: Request<GetDocumentsRequest>) -> Result<Response<Documents>, Status> {
        let req = request.into_inner();
        let tenant_id = id("tenant_id", &req.tenant_id)?;
        if req.issue_ids.len() > MAX_DOCUMENTS {
            return Err(Status::from(ApiError::invalid("issue_ids", "too_many", "At most 500 issues per call.")));
        }
        let ids = req.issue_ids.iter().map(|v| id("issue_ids", v)).collect::<Result<Vec<_>, _>>()?;
        let mut tx = self.db.begin(tenant_id).await?;
        let issues = store::issues_by_id(tx.conn(), tenant_id, &ids).await?;
        let found: Vec<Uuid> = issues.iter().map(|i| i.id).collect();
        let mut comments: HashMap<Uuid, Vec<String>> = HashMap::new();
        for (issue_id, body) in store::comment_bodies(tx.conn(), tenant_id, &found).await? {
            comments.entry(issue_id).or_default().push(richtext::plain_text(&body));
        }
        tx.commit().await?;
        let missing_ids = ids.iter().filter(|i| !found.contains(i)).map(Uuid::to_string).collect();
        let documents = issues
            .into_iter()
            .map(|issue| {
                let texts = comments.remove(&issue.id).unwrap_or_default();
                document(issue, &texts)
            })
            .collect();
        Ok(Response::new(Documents { documents, missing_ids }))
    }

    async fn list_issue_ids(&self, request: Request<ListIssueIdsRequest>) -> Result<Response<IssueIds>, Status> {
        let req = request.into_inner();
        let tenant_id = id("tenant_id", &req.tenant_id)?;
        let after = if req.after_id.is_empty() { None } else { Some(id("after_id", &req.after_id)?) };
        let limit = if req.limit == 0 { MAX_IDS } else { req.limit.min(MAX_IDS) };
        let mut tx = self.db.begin(tenant_id).await?;
        let ids = store::issue_ids_after(tx.conn(), tenant_id, after, i64::from(limit)).await?;
        tx.commit().await?;
        Ok(Response::new(IssueIds { ids: ids.iter().map(Uuid::to_string).collect() }))
    }
}
