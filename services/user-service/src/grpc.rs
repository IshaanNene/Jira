use std::sync::Arc;

use api_contracts::user::v1::{
    membership_server::Membership, CheckMembershipRequest, CheckMembershipResponse, FindMembersRequest, GetMembersRequest,
    Member, Members,
};
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

    async fn get_members(&self, request: Request<GetMembersRequest>) -> Result<Response<Members>, Status> {
        let req = request.into_inner();
        let tenant_id = parse_id("tenant_id", &req.tenant_id)?;
        if req.user_ids.len() > 500 {
            return Err(Status::from(ApiError::invalid("user_ids", "too_many", "At most 500 people per call.")));
        }
        let ids = req.user_ids.iter().map(|v| parse_id("user_ids", v)).collect::<Result<Vec<_>, _>>()?;
        let mut tx = self.db.begin(tenant_id).await?;
        let rows = store::members_by_id(tx.conn(), tenant_id, &ids).await?;
        tx.commit().await?;
        Ok(Response::new(members(rows)))
    }

    async fn find_members(&self, request: Request<FindMembersRequest>) -> Result<Response<Members>, Status> {
        let req = request.into_inner();
        let tenant_id = parse_id("tenant_id", &req.tenant_id)?;
        if req.terms.len() > 100 || req.terms.iter().any(|t| t.chars().count() > 200) {
            return Err(Status::from(ApiError::invalid("terms", "too_many", "At most 100 terms of 200 characters.")));
        }
        if req.terms.is_empty() {
            return Ok(Response::new(Members::default()));
        }
        let mut tx = self.db.begin(tenant_id).await?;
        let rows = store::members_matching(tx.conn(), tenant_id, &req.terms).await?;
        tx.commit().await?;
        Ok(Response::new(members(rows)))
    }
}

fn members(rows: Vec<(Uuid, String, String, String, String)>) -> Members {
    Members {
        members: rows
            .into_iter()
            .map(|(user_id, org_role, status, display_name, email)| Member {
                user_id: user_id.to_string(),
                org_role,
                status,
                display_name,
                email,
            })
            .collect(),
    }
}
