//! What this service asks other services: the people around an issue (issue-service) and
//! who those people are in the workspace (user-service).

use api_contracts::{
    issue::v1::{issues_client::IssuesClient, GetAudienceRequest},
    user::v1::{membership_client::MembershipClient, GetMembersRequest},
};
use async_trait::async_trait;
use auth_middleware::{
    service_auth::{is_transient, lazy_channel, ServiceClientAuth},
    OrgRole,
};
use error_handling::{from_status, ApiError, ApiResult};
use platform_common::resilience::{retry, RetryPolicy};
use tonic::{service::interceptor::InterceptedService, transport::Channel};
use uuid::Uuid;

use crate::domain::Audience;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Person {
    pub user_id: Uuid,
    pub role: OrgRole,
    pub active: bool,
    pub display_name: String,
}

#[async_trait]
pub trait Directory: Send + Sync {
    /// `None` when the issue no longer exists.
    async fn audience(&self, tenant_id: Uuid, issue_id: Uuid) -> ApiResult<Option<Audience>>;
    /// Those of `ids` who are members of the workspace.
    async fn people(&self, tenant_id: Uuid, ids: &[Uuid]) -> ApiResult<Vec<Person>>;
}

type Authed = InterceptedService<Channel, ServiceClientAuth>;

pub struct GrpcDirectory {
    issues: IssuesClient<Authed>,
    members: MembershipClient<Authed>,
}

impl GrpcDirectory {
    pub fn new(issue_url: &str, user_url: &str, secret: &str) -> anyhow::Result<Self> {
        Ok(Self {
            issues: IssuesClient::with_interceptor(
                lazy_channel(issue_url)?,
                ServiceClientAuth::new(secret, crate::SERVICE_NAME, "issue-service")?,
            ),
            members: MembershipClient::with_interceptor(
                lazy_channel(user_url)?,
                ServiceClientAuth::new(secret, crate::SERVICE_NAME, "user-service")?,
            ),
        })
    }
}

fn uuid(value: &str, what: &str) -> ApiResult<Uuid> {
    Uuid::parse_str(value).map_err(|_| ApiError::internal(anyhow::anyhow!("invalid {what} from a dependency")))
}

#[async_trait]
impl Directory for GrpcDirectory {
    async fn audience(&self, tenant_id: Uuid, issue_id: Uuid) -> ApiResult<Option<Audience>> {
        let reply = retry(RetryPolicy::default(), is_transient, || {
            let mut client = self.issues.clone();
            let req = GetAudienceRequest { tenant_id: tenant_id.to_string(), issue_id: issue_id.to_string() };
            async move { client.get_audience(req).await }
        })
        .await
        .map_err(|s| from_status(&s, "issue-service"))?
        .into_inner();
        if !reply.found {
            return Ok(None);
        }
        Ok(Some(Audience {
            key: reply.key,
            summary: reply.summary,
            project_id: uuid(&reply.project_id, "project id")?,
            reporter_id: uuid(&reply.reporter_id, "reporter id")?,
            assignee_id: if reply.assignee_id.is_empty() { None } else { Some(uuid(&reply.assignee_id, "assignee id")?) },
            watcher_ids: reply.watcher_ids.iter().map(|w| uuid(w, "watcher id")).collect::<ApiResult<_>>()?,
        }))
    }

    async fn people(&self, tenant_id: Uuid, ids: &[Uuid]) -> ApiResult<Vec<Person>> {
        let mut out = Vec::new();
        for chunk in ids.chunks(500) {
            let reply = retry(RetryPolicy::default(), is_transient, || {
                let mut client = self.members.clone();
                let req =
                    GetMembersRequest { tenant_id: tenant_id.to_string(), user_ids: chunk.iter().map(Uuid::to_string).collect() };
                async move { client.get_members(req).await }
            })
            .await
            .map_err(|s| from_status(&s, "user-service"))?
            .into_inner();
            for m in reply.members {
                // An unknown role string grants nothing.
                let Some(role) = OrgRole::parse(&m.org_role) else { continue };
                out.push(Person {
                    user_id: uuid(&m.user_id, "user id")?,
                    role,
                    active: m.status == "active",
                    display_name: m.display_name,
                });
            }
        }
        Ok(out)
    }
}
