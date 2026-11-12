use std::sync::Arc;

use api_contracts::permission::v1::{
    authorizer_server::Authorizer, DeleteProjectRequest, GetPolicySnapshotRequest, ListProjectMembersRequest, PolicySnapshot,
    PolicyVersion, ProjectMember, ProjectMembers, ProjectRole as WireRole, RemoveProjectRoleRequest,
    SetProjectDefaultRoleRequest, SetProjectRoleRequest,
};
use auth_middleware::{policy_version_key, ProjectRole, RedisHandle};
use error_handling::ApiError;
use tenant_context::{DbRouter, TenantTx};
use tonic::{Request, Response, Status};
use uuid::Uuid;

use crate::store;

pub struct AuthorizerService {
    pub db: Arc<DbRouter>,
    pub redis: RedisHandle,
}

fn id(field: &'static str, value: &str) -> Result<Uuid, Status> {
    Uuid::parse_str(value).map_err(|_| Status::from(ApiError::invalid(field, "invalid_uuid", "Must be a UUID.")))
}

fn role(value: &str) -> Result<ProjectRole, Status> {
    ProjectRole::parse(value)
        .ok_or_else(|| Status::from(ApiError::invalid("role", "unknown_role", "Role must be administrator, member or viewer.")))
}

fn wire(rows: Vec<(Uuid, String)>) -> Vec<WireRole> {
    rows.into_iter().map(|(project_id, role)| WireRole { project_id: project_id.to_string(), role }).collect()
}

impl AuthorizerService {
    /// Tell every service instance that cached snapshots for this tenant are stale.
    /// Best effort: without Redis, consumers fall back to a short TTL.
    async fn publish_version(&self, tenant_id: Uuid, version: i64) {
        let Some(mut conn) = self.redis.get() else {
            tracing::warn!(%tenant_id, "redis unavailable; policy change propagates by TTL only");
            return;
        };
        let mut cmd = redis::cmd("SET");
        cmd.arg(policy_version_key(tenant_id)).arg(version);
        if let Err(err) = cmd.query_async::<()>(&mut conn).await {
            tracing::warn!(%tenant_id, error = %err, "could not publish policy version");
        }
    }

    /// Commit a mutation: bump the version only if something changed, then publish it.
    async fn finish(&self, mut tx: TenantTx, changed: bool, reason: &str) -> Result<Response<PolicyVersion>, Status> {
        let tenant_id = tx.tenant_id();
        let version = if changed {
            store::bump_version(tx.conn(), tenant_id, reason).await?
        } else {
            store::version(tx.conn(), tenant_id).await?
        };
        tx.commit().await?;
        if changed {
            self.publish_version(tenant_id, version).await;
        }
        Ok(Response::new(PolicyVersion { version }))
    }
}

#[tonic::async_trait]
impl Authorizer for AuthorizerService {
    async fn get_policy_snapshot(&self, request: Request<GetPolicySnapshotRequest>) -> Result<Response<PolicySnapshot>, Status> {
        let req = request.into_inner();
        let (tenant_id, user_id) = (id("tenant_id", &req.tenant_id)?, id("user_id", &req.user_id)?);
        let mut tx = self.db.begin(tenant_id).await?;
        let version = store::version(tx.conn(), tenant_id).await?;
        let roles = store::roles_for_user(tx.conn(), tenant_id, user_id).await?;
        let default_roles = store::default_roles(tx.conn(), tenant_id).await?;
        tx.commit().await?;
        // Re-publishing heals an evicted Redis key.
        self.publish_version(tenant_id, version).await;
        Ok(Response::new(PolicySnapshot { version, roles: wire(roles), default_roles: wire(default_roles) }))
    }

    async fn set_project_role(&self, request: Request<SetProjectRoleRequest>) -> Result<Response<PolicyVersion>, Status> {
        let req = request.into_inner();
        let (tenant_id, project_id, user_id) =
            (id("tenant_id", &req.tenant_id)?, id("project_id", &req.project_id)?, id("user_id", &req.user_id)?);
        let role = role(&req.role)?;
        let mut tx = self.db.begin(tenant_id).await?;
        let changed = store::set_role(tx.conn(), tenant_id, project_id, user_id, role.as_str()).await?;
        self.finish(tx, changed, "project_role_set").await
    }

    async fn remove_project_role(&self, request: Request<RemoveProjectRoleRequest>) -> Result<Response<PolicyVersion>, Status> {
        let req = request.into_inner();
        let (tenant_id, project_id, user_id) =
            (id("tenant_id", &req.tenant_id)?, id("project_id", &req.project_id)?, id("user_id", &req.user_id)?);
        let mut tx = self.db.begin(tenant_id).await?;
        let changed = store::remove_role(tx.conn(), tenant_id, project_id, user_id).await?;
        self.finish(tx, changed, "project_role_removed").await
    }

    async fn set_project_default_role(
        &self,
        request: Request<SetProjectDefaultRoleRequest>,
    ) -> Result<Response<PolicyVersion>, Status> {
        let req = request.into_inner();
        let (tenant_id, project_id) = (id("tenant_id", &req.tenant_id)?, id("project_id", &req.project_id)?);
        let default = if req.role.is_empty() { None } else { Some(role(&req.role)?) };
        let mut tx = self.db.begin(tenant_id).await?;
        let changed = store::set_default_role(tx.conn(), tenant_id, project_id, default.map(ProjectRole::as_str)).await?;
        self.finish(tx, changed, "project_default_role_set").await
    }

    async fn list_project_members(
        &self,
        request: Request<ListProjectMembersRequest>,
    ) -> Result<Response<ProjectMembers>, Status> {
        let req = request.into_inner();
        let (tenant_id, project_id) = (id("tenant_id", &req.tenant_id)?, id("project_id", &req.project_id)?);
        let mut tx = self.db.begin(tenant_id).await?;
        let (members, default_role) = store::members(tx.conn(), tenant_id, project_id).await?;
        tx.commit().await?;
        Ok(Response::new(ProjectMembers {
            members: members.into_iter().map(|(user_id, role)| ProjectMember { user_id: user_id.to_string(), role }).collect(),
            default_role: default_role.unwrap_or_default(),
        }))
    }

    async fn delete_project(&self, request: Request<DeleteProjectRequest>) -> Result<Response<PolicyVersion>, Status> {
        let req = request.into_inner();
        let (tenant_id, project_id) = (id("tenant_id", &req.tenant_id)?, id("project_id", &req.project_id)?);
        let mut tx = self.db.begin(tenant_id).await?;
        let changed = store::delete_project(tx.conn(), tenant_id, project_id).await?;
        self.finish(tx, changed, "project_deleted").await
    }
}
