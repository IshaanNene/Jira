//! Project keys to ids (project-service), for choosing which projects a source-control
//! connection links. Keys never change, so the answer is stored with the connection.

use api_contracts::project::v1::{projects_client::ProjectsClient, GetProjectRequest};
use async_trait::async_trait;
use auth_middleware::service_auth::{lazy_channel, ServiceClientAuth};
use error_handling::{from_status, ApiError, ApiResult};
use tonic::{service::interceptor::InterceptedService, transport::Channel};
use uuid::Uuid;

#[async_trait]
pub trait Projects: Send + Sync {
    /// The project's id, or `None` when the workspace has no project with that key.
    async fn id_of(&self, tenant_id: Uuid, key: &str) -> ApiResult<Option<Uuid>>;
}

pub struct GrpcProjects {
    client: ProjectsClient<InterceptedService<Channel, ServiceClientAuth>>,
}

impl GrpcProjects {
    pub fn new(project_url: &str, secret: &str) -> anyhow::Result<Self> {
        Ok(Self {
            client: ProjectsClient::with_interceptor(
                lazy_channel(project_url)?,
                ServiceClientAuth::new(secret, crate::SERVICE_NAME, "project-service")?,
            ),
        })
    }
}

#[async_trait]
impl Projects for GrpcProjects {
    async fn id_of(&self, tenant_id: Uuid, key: &str) -> ApiResult<Option<Uuid>> {
        let request = GetProjectRequest { tenant_id: tenant_id.to_string(), key: key.to_owned() };
        match self.client.clone().get_project(request).await {
            Ok(reply) => Ok(Uuid::parse_str(&reply.into_inner().id).ok()),
            Err(status) => match from_status(&status, "project-service") {
                ApiError::NotFound(_) => Ok(None),
                other => Err(other),
            },
        }
    }
}
