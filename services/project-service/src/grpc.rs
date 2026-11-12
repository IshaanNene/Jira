use std::sync::Arc;

use api_contracts::project::v1::{projects_server::Projects, GetProjectRequest, Project, Ref};
use error_handling::ApiError;
use tenant_context::DbRouter;
use tonic::{Request, Response, Status};
use uuid::Uuid;

use crate::store;

pub struct ProjectsService {
    pub db: Arc<DbRouter>,
}

#[tonic::async_trait]
impl Projects for ProjectsService {
    async fn get_project(&self, request: Request<GetProjectRequest>) -> Result<Response<Project>, Status> {
        let req = request.into_inner();
        let tenant_id = Uuid::parse_str(&req.tenant_id)
            .map_err(|_| Status::from(ApiError::invalid("tenant_id", "invalid_uuid", "Must be a UUID.")))?;
        let mut tx = self.db.begin(tenant_id).await?;
        let project = store::project_by_key(tx.conn(), tenant_id, &req.key).await?.ok_or(ApiError::NotFound("project"))?;
        let components = store::components(tx.conn(), tenant_id, project.id).await?;
        let versions = store::versions(tx.conn(), tenant_id, project.id).await?;
        tx.commit().await?;
        Ok(Response::new(Project {
            id: project.id.to_string(),
            key: project.key,
            name: project.name,
            archived: project.status == "archived",
            components: components.into_iter().map(|c| Ref { id: c.id.to_string(), name: c.name }).collect(),
            versions: versions.into_iter().map(|v| Ref { id: v.id.to_string(), name: v.name }).collect(),
        }))
    }
}
