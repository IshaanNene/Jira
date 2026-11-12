use std::sync::Arc;

use api_contracts::user::v1::{membership_server::Membership, CheckMembershipRequest, CheckMembershipResponse};
use error_handling::ApiError;
use tenant_context::DbRouter;
use tonic::{Request, Response, Status};
use uuid::Uuid;

use crate::store;

pub struct MembershipService {
    pub db: Arc<DbRouter>,
}

fn parse_id(field: &'static str, value: &str) -> Result<Uuid, Status> {
    Uuid::parse_str(value).map_err(|_| Status::from(ApiError::invalid(field, "invalid_uuid", "Must be a UUID.")))
}

#[tonic::async_trait]
impl Membership for MembershipService {
    async fn check_membership(
        &self,
        request: Request<CheckMembershipRequest>,
    ) -> Result<Response<CheckMembershipResponse>, Status> {
        let req = request.into_inner();
        let user_id = parse_id("user_id", &req.user_id)?;
        let organization_id = parse_id("organization_id", &req.organization_id)?;
        let not_member = || Response::new(CheckMembershipResponse::default());

        let Some(tenant_id) = store::directory_tenant(self.db.global()?, user_id, organization_id).await? else {
            return Ok(not_member());
        };
        // The directory is only a pointer; the authoritative membership row is read
        // inside the tenant's own scope.
        let mut tx = self.db.begin(tenant_id).await?;
        let membership = store::membership(tx.conn(), tenant_id, user_id).await?;
        tx.commit().await?;
        Ok(match membership {
            Some(m) => Response::new(CheckMembershipResponse {
                is_member: true,
                tenant_id: tenant_id.to_string(),
                org_role: m.org_role,
                status: m.status,
            }),
            None => not_member(),
        })
    }
}
