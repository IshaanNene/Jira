//! What this service reads from other services: project identity and issue cards.
//! Boards never store issue fields; they are fetched when a board is rendered.
//! (Phase 5 replaces the per-request read with a local projection fed by issue events.)

use api_contracts::{
    issue::v1::{issues_client::IssuesClient, Card as WireCard, ListCardsRequest},
    project::v1::{projects_client::ProjectsClient, GetProjectRequest},
};
use async_trait::async_trait;
use auth_middleware::service_auth::{is_transient, lazy_channel, ServiceClientAuth};
use error_handling::{from_status, ApiError, ApiResult};
use platform_common::resilience::{retry, CircuitBreaker, RetryPolicy};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};
use tonic::{service::interceptor::InterceptedService, transport::Channel};
use uuid::Uuid;

/// Issue types that are not planned on boards in this phase.
pub const OFF_BOARD_TYPES: [&str; 2] = ["epic", "subtask"];

#[derive(Debug, Clone, PartialEq)]
pub struct Card {
    pub id: Uuid,
    pub key: String,
    pub number: i64,
    pub issue_type: String,
    pub status: String,
    pub status_category: String,
    pub priority: String,
    pub summary: String,
    pub assignee_id: Option<Uuid>,
    pub story_points: Option<f64>,
    pub resolved_at: Option<OffsetDateTime>,
    pub parent_key: Option<String>,
    pub parent_summary: Option<String>,
    pub version: i64,
    pub labels: Vec<String>,
}

impl Card {
    pub fn done(&self) -> bool {
        self.status_category == "done"
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectRef {
    pub id: Uuid,
    pub key: String,
    pub archived: bool,
}

#[async_trait]
pub trait Projects: Send + Sync {
    /// `NotFound` when the tenant has no such project.
    async fn project(&self, tenant_id: Uuid, key: &str) -> ApiResult<ProjectRef>;
}

#[async_trait]
pub trait Issues: Send + Sync {
    /// Board-eligible issues that are unfinished or were resolved at or after
    /// `done_since`. The flag reports that more matched than could be returned.
    async fn open_cards(&self, tenant_id: Uuid, project_id: Uuid, done_since: OffsetDateTime) -> ApiResult<(Vec<Card>, bool)>;
    /// The given issues, as far as they exist in the project (any type, any status).
    async fn cards_by_id(&self, tenant_id: Uuid, project_id: Uuid, ids: &[Uuid]) -> ApiResult<Vec<Card>>;
}

type Authed = InterceptedService<Channel, ServiceClientAuth>;

pub struct GrpcProjects {
    client: ProjectsClient<Authed>,
}

impl GrpcProjects {
    pub fn new(url: &str, secret: &str) -> anyhow::Result<Self> {
        let auth = ServiceClientAuth::new(secret, crate::SERVICE_NAME, "project-service")?;
        Ok(Self { client: ProjectsClient::with_interceptor(lazy_channel(url)?, auth) })
    }
}

#[async_trait]
impl Projects for GrpcProjects {
    async fn project(&self, tenant_id: Uuid, key: &str) -> ApiResult<ProjectRef> {
        let result = retry(RetryPolicy::default(), is_transient, || {
            let mut client = self.client.clone();
            let req = GetProjectRequest { tenant_id: tenant_id.to_string(), key: key.to_owned() };
            async move { client.get_project(req).await }
        })
        .await;
        let reply = result
            .map_err(|s| match from_status(&s, "project-service") {
                ApiError::NotFound(_) => ApiError::NotFound("project"),
                other => other,
            })?
            .into_inner();
        Ok(ProjectRef {
            id: Uuid::parse_str(&reply.id)
                .map_err(|_| ApiError::internal(anyhow::anyhow!("project-service returned an invalid id")))?,
            key: reply.key,
            archived: reply.archived,
        })
    }
}

const ISSUE_SERVICE: &str = "issue-service";
/// issue-service caps one call at 2000 cards.
const CHUNK: usize = 1000;

pub struct GrpcIssues {
    client: IssuesClient<Authed>,
    breaker: CircuitBreaker,
}

impl GrpcIssues {
    pub fn new(url: &str, secret: &str) -> anyhow::Result<Self> {
        let auth = ServiceClientAuth::new(secret, crate::SERVICE_NAME, ISSUE_SERVICE)?;
        Ok(Self { client: IssuesClient::with_interceptor(lazy_channel(url)?, auth), breaker: CircuitBreaker::default() })
    }

    /// Read-only, so safe to retry.
    async fn list(&self, req: ListCardsRequest) -> ApiResult<(Vec<Card>, bool)> {
        if self.breaker.try_acquire().is_err() {
            return Err(ApiError::DependencyUnavailable(ISSUE_SERVICE));
        }
        let result = retry(RetryPolicy::default(), is_transient, || {
            let mut client = self.client.clone();
            let req = req.clone();
            async move { client.list_cards(req).await }
        })
        .await;
        self.breaker.record(result.as_ref().map_or_else(|s| !is_transient(s), |_| true));
        let reply = result.map_err(|s| from_status(&s, ISSUE_SERVICE))?.into_inner();
        let cards = reply.cards.into_iter().map(card).collect::<ApiResult<Vec<_>>>()?;
        Ok((cards, reply.truncated))
    }
}

fn invalid(what: &str) -> ApiError {
    ApiError::internal(anyhow::anyhow!("issue-service returned an invalid {what}"))
}

fn non_empty(value: String) -> Option<String> {
    (!value.is_empty()).then_some(value)
}

fn card(w: WireCard) -> ApiResult<Card> {
    Ok(Card {
        id: Uuid::parse_str(&w.id).map_err(|_| invalid("issue id"))?,
        key: w.key,
        number: w.number,
        issue_type: w.r#type,
        status: w.status,
        status_category: w.status_category,
        priority: w.priority,
        summary: w.summary,
        assignee_id: match w.assignee_id.as_str() {
            "" => None,
            id => Some(Uuid::parse_str(id).map_err(|_| invalid("assignee id"))?),
        },
        story_points: w.story_points,
        resolved_at: match w.resolved_at.as_str() {
            "" => None,
            at => Some(OffsetDateTime::parse(at, &Rfc3339).map_err(|_| invalid("timestamp"))?),
        },
        parent_key: non_empty(w.parent_key),
        parent_summary: non_empty(w.parent_summary),
        version: w.version,
        labels: w.labels,
    })
}

#[async_trait]
impl Issues for GrpcIssues {
    async fn open_cards(&self, tenant_id: Uuid, project_id: Uuid, done_since: OffsetDateTime) -> ApiResult<(Vec<Card>, bool)> {
        self.list(ListCardsRequest {
            tenant_id: tenant_id.to_string(),
            project_id: project_id.to_string(),
            issue_ids: vec![],
            done_since: done_since.format(&Rfc3339).map_err(ApiError::internal)?,
            exclude_types: OFF_BOARD_TYPES.iter().map(|t| (*t).to_owned()).collect(),
            limit: 0,
        })
        .await
    }

    async fn cards_by_id(&self, tenant_id: Uuid, project_id: Uuid, ids: &[Uuid]) -> ApiResult<Vec<Card>> {
        let mut out = Vec::with_capacity(ids.len());
        for chunk in ids.chunks(CHUNK) {
            let (cards, _) = self
                .list(ListCardsRequest {
                    tenant_id: tenant_id.to_string(),
                    project_id: project_id.to_string(),
                    issue_ids: chunk.iter().map(Uuid::to_string).collect(),
                    done_since: String::new(),
                    exclude_types: vec![],
                    limit: 0,
                })
                .await?;
            out.extend(cards);
        }
        Ok(out)
    }
}
