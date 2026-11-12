//! Outbound calls: role bindings live in the permission service, organization membership
//! in the user service.

use api_contracts::{
    permission::v1::{
        DeleteProjectRequest, ListProjectMembersRequest, RemoveProjectRoleRequest, SetProjectDefaultRoleRequest,
        SetProjectRoleRequest,
    },
    user::v1::{membership_client::MembershipClient, CheckMembershipRequest},
};
use async_trait::async_trait;
use auth_middleware::{
    policy::AuthorizerGrpc,
    service_auth::{lazy_channel, ServiceClientAuth},
    GrpcPolicySource, ProjectRole,
};
use error_handling::{from_status, ApiResult};
use tonic::{service::interceptor::InterceptedService, transport::Channel};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberList {
    pub members: Vec<(Uuid, ProjectRole)>,
    pub default_role: Option<ProjectRole>,
}

#[async_trait]
pub trait ProjectAccess: Send + Sync {
    async fn set_role(&self, tenant: Uuid, project: Uuid, user: Uuid, role: ProjectRole) -> ApiResult<()>;
    async fn remove_role(&self, tenant: Uuid, project: Uuid, user: Uuid) -> ApiResult<()>;
    async fn set_default_role(&self, tenant: Uuid, project: Uuid, role: Option<ProjectRole>) -> ApiResult<()>;
    async fn members(&self, tenant: Uuid, project: Uuid) -> ApiResult<MemberList>;
    async fn delete_project(&self, tenant: Uuid, project: Uuid) -> ApiResult<()>;
    /// Whether `user` is an active member of the organization.
    async fn is_org_member(&self, user: Uuid, organization: Uuid) -> ApiResult<bool>;
}

pub struct GrpcAccess {
    authorizer: AuthorizerGrpc,
    membership: MembershipClient<InterceptedService<Channel, ServiceClientAuth>>,
}

impl GrpcAccess {
    pub fn new(permission_url: &str, user_url: &str, secret: &str) -> anyhow::Result<Self> {
        let user_auth = ServiceClientAuth::new(secret, crate::SERVICE_NAME, "user-service")?;
        Ok(Self {
            authorizer: GrpcPolicySource::client(permission_url, secret, crate::SERVICE_NAME)?,
            membership: MembershipClient::with_interceptor(lazy_channel(user_url)?, user_auth),
        })
    }
}

const PERMISSION: &str = "permission-service";

#[async_trait]
impl ProjectAccess for GrpcAccess {
    async fn set_role(&self, tenant: Uuid, project: Uuid, user: Uuid, role: ProjectRole) -> ApiResult<()> {
        let req = SetProjectRoleRequest {
            tenant_id: tenant.to_string(),
            project_id: project.to_string(),
            user_id: user.to_string(),
            role: role.as_str().to_owned(),
        };
        self.authorizer.clone().set_project_role(req).await.map_err(|s| from_status(&s, PERMISSION))?;
        Ok(())
    }

    async fn remove_role(&self, tenant: Uuid, project: Uuid, user: Uuid) -> ApiResult<()> {
        let req = RemoveProjectRoleRequest {
            tenant_id: tenant.to_string(),
            project_id: project.to_string(),
            user_id: user.to_string(),
        };
        self.authorizer.clone().remove_project_role(req).await.map_err(|s| from_status(&s, PERMISSION))?;
        Ok(())
    }

    async fn set_default_role(&self, tenant: Uuid, project: Uuid, role: Option<ProjectRole>) -> ApiResult<()> {
        let req = SetProjectDefaultRoleRequest {
            tenant_id: tenant.to_string(),
            project_id: project.to_string(),
            role: role.map(|r| r.as_str().to_owned()).unwrap_or_default(),
        };
        self.authorizer.clone().set_project_default_role(req).await.map_err(|s| from_status(&s, PERMISSION))?;
        Ok(())
    }

    async fn members(&self, tenant: Uuid, project: Uuid) -> ApiResult<MemberList> {
        let req = ListProjectMembersRequest { tenant_id: tenant.to_string(), project_id: project.to_string() };
        let reply =
            self.authorizer.clone().list_project_members(req).await.map_err(|s| from_status(&s, PERMISSION))?.into_inner();
        Ok(MemberList {
            members: reply
                .members
                .into_iter()
                .filter_map(|m| Some((Uuid::parse_str(&m.user_id).ok()?, ProjectRole::parse(&m.role)?)))
                .collect(),
            default_role: ProjectRole::parse(&reply.default_role),
        })
    }

    async fn delete_project(&self, tenant: Uuid, project: Uuid) -> ApiResult<()> {
        let req = DeleteProjectRequest { tenant_id: tenant.to_string(), project_id: project.to_string() };
        self.authorizer.clone().delete_project(req).await.map_err(|s| from_status(&s, PERMISSION))?;
        Ok(())
    }

    async fn is_org_member(&self, user: Uuid, organization: Uuid) -> ApiResult<bool> {
        let req = CheckMembershipRequest { user_id: user.to_string(), organization_id: organization.to_string() };
        let reply =
            self.membership.clone().check_membership(req).await.map_err(|s| from_status(&s, "user-service"))?.into_inner();
        Ok(reply.is_member && reply.status == "active")
    }
}
