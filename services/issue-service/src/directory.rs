//! What the issue service needs to know from other services: where a project key
//! points, and whether someone belongs to the organization.

use std::{
    collections::{HashMap, HashSet},
    sync::Mutex,
    time::{Duration, Instant},
};

use api_contracts::{
    project::v1::{projects_client::ProjectsClient, GetProjectRequest},
    user::v1::{membership_client::MembershipClient, CheckMembershipRequest},
};
use async_trait::async_trait;
use auth_middleware::service_auth::{lazy_channel, ServiceClientAuth};
use error_handling::{from_status, ApiError, ApiResult};
use tonic::{service::interceptor::InterceptedService, transport::Channel};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectInfo {
    pub id: Uuid,
    pub key: String,
    pub archived: bool,
    pub component_ids: HashSet<Uuid>,
    pub version_ids: HashSet<Uuid>,
}

#[async_trait]
pub trait Directory: Send + Sync {
    /// `NotFound` when the tenant has no such project. `fresh` bypasses any cache — used
    /// when a cached answer says a component or version does not exist (it may be new).
    async fn project(&self, tenant_id: Uuid, key: &str, fresh: bool) -> ApiResult<ProjectInfo>;
    async fn is_org_member(&self, user_id: Uuid, organization_id: Uuid) -> ApiResult<bool>;
}

type Authed = InterceptedService<Channel, ServiceClientAuth>;

pub struct GrpcDirectory {
    projects: ProjectsClient<Authed>,
    membership: MembershipClient<Authed>,
    // Keys are immutable and archiving is rare: a short cache removes a remote call
    // from most requests while bounding how long a just-archived project stays writable.
    cache: Mutex<HashMap<(Uuid, String), (Instant, ProjectInfo)>>,
}

const CACHE_TTL: Duration = Duration::from_secs(15);

impl GrpcDirectory {
    pub fn new(project_url: &str, user_url: &str, secret: &str) -> anyhow::Result<Self> {
        Ok(Self {
            projects: ProjectsClient::with_interceptor(
                lazy_channel(project_url)?,
                ServiceClientAuth::new(secret, crate::SERVICE_NAME, "project-service")?,
            ),
            membership: MembershipClient::with_interceptor(
                lazy_channel(user_url)?,
                ServiceClientAuth::new(secret, crate::SERVICE_NAME, "user-service")?,
            ),
            cache: Mutex::new(HashMap::new()),
        })
    }
}

#[async_trait]
impl Directory for GrpcDirectory {
    async fn project(&self, tenant_id: Uuid, key: &str, fresh: bool) -> ApiResult<ProjectInfo> {
        let cache_key = (tenant_id, key.to_owned());
        let cached = if fresh { None } else { self.cache.lock().ok().and_then(|c| c.get(&cache_key).cloned()) };
        if let Some((at, info)) = cached {
            if at.elapsed() < CACHE_TTL {
                return Ok(info);
            }
        }
        let reply = self
            .projects
            .clone()
            .get_project(GetProjectRequest { tenant_id: tenant_id.to_string(), key: key.to_owned() })
            .await
            .map_err(|s| match from_status(&s, "project-service") {
                ApiError::NotFound(_) => ApiError::NotFound("project"),
                other => other,
            })?
            .into_inner();
        let info = ProjectInfo {
            id: Uuid::parse_str(&reply.id)
                .map_err(|_| ApiError::internal(anyhow::anyhow!("project-service returned an invalid id")))?,
            key: reply.key,
            archived: reply.archived,
            component_ids: reply.components.iter().filter_map(|r| Uuid::parse_str(&r.id).ok()).collect(),
            version_ids: reply.versions.iter().filter_map(|r| Uuid::parse_str(&r.id).ok()).collect(),
        };
        if let Ok(mut cache) = self.cache.lock() {
            if cache.len() > 10_000 {
                cache.clear();
            }
            cache.insert(cache_key, (Instant::now(), info.clone()));
        }
        Ok(info)
    }

    async fn is_org_member(&self, user_id: Uuid, organization_id: Uuid) -> ApiResult<bool> {
        let req = CheckMembershipRequest { user_id: user_id.to_string(), organization_id: organization_id.to_string() };
        let reply =
            self.membership.clone().check_membership(req).await.map_err(|s| from_status(&s, "user-service"))?.into_inner();
        Ok(reply.is_member && reply.status == "active")
    }
}
