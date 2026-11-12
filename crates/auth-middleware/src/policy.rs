//! Project-level authorization from versioned policy snapshots (ADR-0007).
//!
//! A snapshot lists one user's project roles in one tenant. Services cache it and
//! evaluate permissions locally; the permission service bumps a per-tenant version in
//! Redis on every change, and a cached snapshot with a different version is discarded.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use api_contracts::permission::v1::{authorizer_client::AuthorizerClient, GetPolicySnapshotRequest};
use async_trait::async_trait;
use error_handling::{from_status, ApiError, ApiResult};
use platform_common::resilience::{retry, CircuitBreaker, RetryPolicy};
use tonic::{service::interceptor::InterceptedService, transport::Channel};
use uuid::Uuid;

use crate::{
    authz::OrgRole,
    extract::OrgContext,
    redis_handle::RedisHandle,
    service_auth::{is_transient, lazy_channel, ServiceClientAuth},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ProjectRole {
    Viewer,
    Member,
    Administrator,
}

impl ProjectRole {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "viewer" => Some(Self::Viewer),
            "member" => Some(Self::Member),
            "administrator" => Some(Self::Administrator),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Viewer => "viewer",
            Self::Member => "member",
            Self::Administrator => "administrator",
        }
    }

    pub fn allows(self, permission: ProjectPermission) -> bool {
        self >= permission.minimum_role()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProjectPermission {
    Browse,
    CreateIssue,
    EditIssue,
    TransitionIssue,
    Comment,
    /// Create, start, complete and delete sprints.
    ManageSprints,
    DeleteIssue,
    /// Edit or delete someone else's comment.
    ModerateComments,
    ManageProject,
}

impl ProjectPermission {
    pub const fn minimum_role(self) -> ProjectRole {
        match self {
            Self::Browse => ProjectRole::Viewer,
            Self::CreateIssue | Self::EditIssue | Self::TransitionIssue | Self::Comment | Self::ManageSprints => {
                ProjectRole::Member
            }
            Self::DeleteIssue | Self::ModerateComments | Self::ManageProject => ProjectRole::Administrator,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PolicySnapshot {
    pub version: i64,
    /// This user's explicit bindings.
    pub roles: HashMap<Uuid, ProjectRole>,
    /// Role granted to every organization member (not guests), per project.
    pub default_roles: HashMap<Uuid, ProjectRole>,
}

/// Which projects a caller may see.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VisibleProjects {
    All,
    Only(Vec<Uuid>),
}

impl PolicySnapshot {
    /// Effective role: organization admins and owners administer every project; everyone
    /// else gets the stronger of their explicit binding and the project default.
    pub fn project_role(&self, ctx: &OrgContext, project_id: Uuid) -> Option<ProjectRole> {
        if ctx.role >= OrgRole::Admin {
            return Some(ProjectRole::Administrator);
        }
        let explicit = self.roles.get(&project_id).copied();
        let default = if ctx.role >= OrgRole::Member { self.default_roles.get(&project_id).copied() } else { None };
        explicit.max(default)
    }

    /// Deny by default. No access at all reads as "not found" so that the existence of
    /// private projects is not revealed; insufficient access reads as "forbidden".
    pub fn require(&self, ctx: &OrgContext, project_id: Uuid, permission: ProjectPermission) -> ApiResult<ProjectRole> {
        match self.project_role(ctx, project_id) {
            None => Err(ApiError::NotFound("project")),
            Some(role) if role.allows(permission) => Ok(role),
            Some(_) => Err(ApiError::Forbidden),
        }
    }

    pub fn visible_projects(&self, ctx: &OrgContext) -> VisibleProjects {
        if ctx.role >= OrgRole::Admin {
            return VisibleProjects::All;
        }
        let mut ids: Vec<Uuid> = self.roles.keys().copied().collect();
        if ctx.role >= OrgRole::Member {
            ids.extend(self.default_roles.keys().copied());
        }
        ids.sort_unstable();
        ids.dedup();
        VisibleProjects::Only(ids)
    }
}

#[async_trait]
pub trait PolicySource: Send + Sync {
    async fn fetch(&self, tenant_id: Uuid, user_id: Uuid) -> ApiResult<PolicySnapshot>;
}

/// Fixed snapshot for every user. Tests only.
#[derive(Default)]
pub struct StaticPolicy(pub Mutex<PolicySnapshot>);

#[async_trait]
impl PolicySource for StaticPolicy {
    async fn fetch(&self, _tenant_id: Uuid, _user_id: Uuid) -> ApiResult<PolicySnapshot> {
        Ok(self.0.lock().map(|s| s.clone()).unwrap_or_default())
    }
}

pub type AuthorizerGrpc = AuthorizerClient<InterceptedService<Channel, ServiceClientAuth>>;

pub struct GrpcPolicySource {
    client: AuthorizerGrpc,
    breaker: CircuitBreaker,
}

impl GrpcPolicySource {
    pub fn new(url: &str, secret: &str, caller: &'static str) -> anyhow::Result<Self> {
        Ok(Self { client: Self::client(url, secret, caller)?, breaker: CircuitBreaker::default() })
    }

    pub fn client(url: &str, secret: &str, caller: &'static str) -> anyhow::Result<AuthorizerGrpc> {
        let auth = ServiceClientAuth::new(secret, caller, "permission-service")?;
        Ok(AuthorizerClient::with_interceptor(lazy_channel(url)?, auth))
    }
}

fn parse_roles(roles: Vec<api_contracts::permission::v1::ProjectRole>) -> HashMap<Uuid, ProjectRole> {
    // Unknown ids or role names grant nothing.
    roles.into_iter().filter_map(|r| Some((Uuid::parse_str(&r.project_id).ok()?, ProjectRole::parse(&r.role)?))).collect()
}

#[async_trait]
impl PolicySource for GrpcPolicySource {
    async fn fetch(&self, tenant_id: Uuid, user_id: Uuid) -> ApiResult<PolicySnapshot> {
        if self.breaker.try_acquire().is_err() {
            return Err(ApiError::DependencyUnavailable("permission-service"));
        }
        let result = retry(RetryPolicy::default(), is_transient, || {
            let mut client = self.client.clone();
            async move {
                client
                    .get_policy_snapshot(GetPolicySnapshotRequest {
                        tenant_id: tenant_id.to_string(),
                        user_id: user_id.to_string(),
                    })
                    .await
            }
        })
        .await;
        self.breaker.record(result.as_ref().map_or_else(|s| !is_transient(s), |_| true));
        let reply = result.map_err(|s| from_status(&s, "permission-service"))?.into_inner();
        Ok(PolicySnapshot {
            version: reply.version,
            roles: parse_roles(reply.roles),
            default_roles: parse_roles(reply.default_roles),
        })
    }
}

pub fn policy_version_key(tenant_id: Uuid) -> String {
    format!("t:{tenant_id}:pv")
}

/// With Redis reachable a cached snapshot is trusted while its version matches (re-checked
/// every request, one GET). Without it, snapshots are only trusted this long.
const TTL_WITH_VERSION: Duration = Duration::from_secs(300);
const TTL_BLIND: Duration = Duration::from_secs(5);
const MAX_ENTRIES: usize = 20_000;

/// (tenant, user) → (fetched at, snapshot)
type SnapshotCache = HashMap<(Uuid, Uuid), (Instant, Arc<PolicySnapshot>)>;

pub struct PolicyClient {
    source: Arc<dyn PolicySource>,
    redis: RedisHandle,
    cache: Mutex<SnapshotCache>,
}

impl PolicyClient {
    pub fn new(source: Arc<dyn PolicySource>, redis: RedisHandle) -> Self {
        Self { source, redis, cache: Mutex::new(HashMap::new()) }
    }

    async fn current_version(&self, tenant_id: Uuid) -> Option<i64> {
        let mut conn = self.redis.get()?;
        let mut cmd = redis::cmd("GET");
        cmd.arg(policy_version_key(tenant_id));
        match tokio::time::timeout(Duration::from_millis(200), cmd.query_async::<Option<i64>>(&mut conn)).await {
            Ok(Ok(version)) => Some(version.unwrap_or(0)),
            _ => None,
        }
    }

    pub async fn snapshot(&self, ctx: &OrgContext) -> ApiResult<Arc<PolicySnapshot>> {
        let key = (ctx.tenant_id, ctx.user_id);
        let cached = self.cache.lock().ok().and_then(|c| c.get(&key).cloned());
        if let Some((at, snapshot)) = cached {
            let fresh = match self.current_version(ctx.tenant_id).await {
                Some(version) => version == snapshot.version && at.elapsed() < TTL_WITH_VERSION,
                None => at.elapsed() < TTL_BLIND,
            };
            if fresh {
                return Ok(snapshot);
            }
        }
        let snapshot = Arc::new(self.source.fetch(ctx.tenant_id, ctx.user_id).await?);
        if let Ok(mut cache) = self.cache.lock() {
            if cache.len() >= MAX_ENTRIES {
                cache.clear(); // bounded memory; a cold cache only costs refetches
            }
            cache.insert(key, (Instant::now(), Arc::clone(&snapshot)));
        }
        Ok(snapshot)
    }

    /// Drop this user's cached snapshot (after this process changed their bindings).
    pub fn forget(&self, tenant_id: Uuid, user_id: Uuid) {
        if let Ok(mut cache) = self.cache.lock() {
            cache.remove(&(tenant_id, user_id));
        }
    }

    pub fn forget_tenant(&self, tenant_id: Uuid) {
        if let Ok(mut cache) = self.cache.lock() {
            cache.retain(|(t, _), _| *t != tenant_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(role: OrgRole) -> OrgContext {
        OrgContext {
            user_id: Uuid::now_v7(),
            session_id: Uuid::now_v7(),
            tenant_id: Uuid::now_v7(),
            organization_id: Uuid::now_v7(),
            role,
            email: "a@example.com".into(),
            name: "A".into(),
        }
    }

    #[test]
    fn role_permission_table() {
        assert!(ProjectRole::Viewer.allows(ProjectPermission::Browse));
        assert!(!ProjectRole::Viewer.allows(ProjectPermission::CreateIssue));
        assert!(ProjectRole::Member.allows(ProjectPermission::EditIssue));
        assert!(!ProjectRole::Member.allows(ProjectPermission::DeleteIssue));
        assert!(!ProjectRole::Member.allows(ProjectPermission::ManageProject));
        assert!(ProjectRole::Administrator.allows(ProjectPermission::ManageProject));
    }

    #[test]
    fn org_admins_administer_everything() {
        let snapshot = PolicySnapshot::default();
        let project = Uuid::now_v7();
        assert_eq!(snapshot.project_role(&ctx(OrgRole::Admin), project), Some(ProjectRole::Administrator));
        assert_eq!(snapshot.visible_projects(&ctx(OrgRole::Owner)), VisibleProjects::All);
    }

    #[test]
    fn members_need_a_binding_or_a_default_and_guests_never_get_defaults() {
        let (private, open, bound) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
        let mut snapshot = PolicySnapshot::default();
        snapshot.default_roles.insert(open, ProjectRole::Member);
        snapshot.default_roles.insert(bound, ProjectRole::Viewer);
        snapshot.roles.insert(bound, ProjectRole::Administrator);

        let member = ctx(OrgRole::Member);
        assert!(matches!(snapshot.require(&member, private, ProjectPermission::Browse), Err(ApiError::NotFound(_))));
        assert_eq!(snapshot.project_role(&member, open), Some(ProjectRole::Member));
        assert_eq!(snapshot.project_role(&member, bound), Some(ProjectRole::Administrator), "strongest wins");
        assert!(matches!(snapshot.require(&member, open, ProjectPermission::ManageProject), Err(ApiError::Forbidden)));

        let guest = ctx(OrgRole::Guest);
        assert_eq!(snapshot.project_role(&guest, open), None);
        assert_eq!(snapshot.project_role(&guest, bound), Some(ProjectRole::Administrator), "explicit bindings still apply");
        let VisibleProjects::Only(ids) = snapshot.visible_projects(&guest) else { panic!("guest sees a list") };
        assert_eq!(ids, vec![bound]);
    }

    #[tokio::test]
    async fn client_caches_briefly_without_redis() {
        let source = Arc::new(StaticPolicy::default());
        let client = PolicyClient::new(source.clone(), RedisHandle::default());
        let c = ctx(OrgRole::Member);
        let first = client.snapshot(&c).await.unwrap();
        source.0.lock().unwrap().version = 9;
        assert_eq!(client.snapshot(&c).await.unwrap().version, first.version, "served from cache");
        client.forget(c.tenant_id, c.user_id);
        assert_eq!(client.snapshot(&c).await.unwrap().version, 9);
    }
}
