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

use crate::{
    directory::{Directory, ProjectInfo},
    richtext, store,
    workflows::{Scope, WorkflowInfo, Workflows},
};

const MAX_DOCUMENTS: usize = 500;
const MAX_IDS: u32 = 1000;
/// Characters of a comment returned with an audience (for quoting in emails).
const MAX_COMMENT_TEXT: usize = 2000;

fn bounded(text: &str, max: usize) -> String {
    let text = text.trim();
    match text.char_indices().nth(max) {
        Some((end, _)) => format!("{}…", text[..end].trim_end()),
        None => text.to_owned(),
    }
}

/// Hard ceiling per call, whatever the caller asks for.
const MAX_CARDS: u32 = 2000;

pub struct IssuesService {
    pub db: Arc<DbRouter>,
    /// Component and version names for search documents.
    pub directory: Arc<dyn Directory>,
    /// Status display names for search documents.
    pub workflows: Arc<dyn Workflows>,
}

/// Names a search document shows: per project key, its components and versions; per
/// (project, issue type), its workflow.
#[derive(Default)]
struct Names {
    projects: HashMap<String, ProjectInfo>,
    workflows: HashMap<(Uuid, String), WorkflowInfo>,
}

impl Names {
    async fn load(&mut self, service: &IssuesService, tenant_id: Uuid, issues: &[store::Issue]) -> Result<(), ApiError> {
        for issue in issues {
            if !self.projects.contains_key(&issue.project_key) {
                let mut info = service.directory.project(tenant_id, &issue.project_key, false).await?;
                // A component or version created moments ago may not be in a cached answer.
                let named = |info: &ProjectInfo| {
                    issue.component_ids.iter().chain(&issue.fix_version_ids).all(|id| info.names.contains_key(id))
                };
                if !named(&info) {
                    info = service.directory.project(tenant_id, &issue.project_key, true).await?;
                }
                self.projects.insert(issue.project_key.clone(), info);
            }
            if let std::collections::hash_map::Entry::Vacant(slot) =
                self.workflows.entry((issue.project_id, issue.type_key.clone()))
            {
                let scope = Scope { tenant_id, project_id: issue.project_id, issue_type: &issue.type_key };
                slot.insert(service.workflows.info(scope).await?);
            }
        }
        Ok(())
    }

    fn of(&self, project_key: &str, ids: &[Uuid]) -> Vec<String> {
        let names = self.projects.get(project_key).map(|p| &p.names);
        ids.iter().filter_map(|id| names.and_then(|n| n.get(id)).cloned()).collect()
    }

    fn status(&self, issue: &store::Issue) -> String {
        self.workflows
            .get(&(issue.project_id, issue.type_key.clone()))
            .and_then(|w| w.status(&issue.status))
            .map(|s| s.name.clone())
            .unwrap_or_default()
    }
}

fn id(field: &'static str, value: &str) -> Result<Uuid, Status> {
    Uuid::parse_str(value).map_err(|_| Status::from(ApiError::invalid(field, "invalid_uuid", "Must be a UUID.")))
}

fn timestamp(t: OffsetDateTime) -> String {
    t.format(&Rfc3339).unwrap_or_default()
}

fn document(issue: store::Issue, comments: &[String], names: &Names) -> Document {
    Document {
        status_name: names.status(&issue),
        components: names.of(&issue.project_key, &issue.component_ids),
        fix_versions: names.of(&issue.project_key, &issue.fix_version_ids),
        parent_key: match (&issue.parent_project_key, issue.parent_number) {
            (Some(key), Some(number)) => format!("{key}-{number}"),
            _ => String::new(),
        },
        due_date: issue.due_date.map(|d| d.to_string()).unwrap_or_default(),
        story_points: issue.story_points,
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
        let comment = if req.comment_id.is_empty() {
            None
        } else {
            store::comment_body(tx.conn(), tenant_id, issue_id, id("comment_id", &req.comment_id)?).await?
        };
        tx.commit().await?;
        Ok(Response::new(Audience {
            found: true,
            key: issue.key(),
            summary: issue.summary,
            project_id: issue.project_id.to_string(),
            reporter_id: issue.reporter_id.to_string(),
            assignee_id: issue.assignee_id.map(|a| a.to_string()).unwrap_or_default(),
            watcher_ids: watchers.iter().map(Uuid::to_string).collect(),
            comment_text: comment.map(|body| bounded(&richtext::plain_text(&body), MAX_COMMENT_TEXT)).unwrap_or_default(),
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
        let mut names = Names::default();
        names.load(self, tenant_id, &issues).await?;
        let missing_ids = ids.iter().filter(|i| !found.contains(i)).map(Uuid::to_string).collect();
        let documents = issues
            .into_iter()
            .map(|issue| {
                let texts = comments.remove(&issue.id).unwrap_or_default();
                document(issue, &texts, &names)
            })
            .collect();
        Ok(Response::new(Documents { documents, missing_ids }))
    }

    async fn list_issue_ids(&self, request: Request<ListIssueIdsRequest>) -> Result<Response<IssueIds>, Status> {
        let req = request.into_inner();
        let tenant_id = id("tenant_id", &req.tenant_id)?;
        let after = if req.after_id.is_empty() { None } else { Some(id("after_id", &req.after_id)?) };
        let project = if req.project_id.is_empty() { None } else { Some(id("project_id", &req.project_id)?) };
        let limit = if req.limit == 0 { MAX_IDS } else { req.limit.min(MAX_IDS) };
        let mut tx = self.db.begin(tenant_id).await?;
        let ids = store::issue_ids_after(tx.conn(), tenant_id, project, after, i64::from(limit)).await?;
        tx.commit().await?;
        Ok(Response::new(IssueIds { ids: ids.iter().map(Uuid::to_string).collect() }))
    }
}
