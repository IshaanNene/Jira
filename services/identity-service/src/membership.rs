//! Client for the user service's membership check, used before signing an
//! organization-scoped token.

use std::sync::Arc;

use api_contracts::user::v1::{membership_client::MembershipClient, CheckMembershipRequest};
use async_trait::async_trait;
use auth_middleware::{
    service_auth::{is_transient, lazy_channel, ServiceClientAuth},
    OrgRole,
};
use error_handling::{from_status, ApiError, ApiResult};
use platform_common::resilience::{retry, CircuitBreaker, RetryPolicy};
use tonic::{service::interceptor::InterceptedService, transport::Channel};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MembershipInfo {
    pub tenant_id: Uuid,
    pub role: OrgRole,
}

#[async_trait]
pub trait MembershipChecker: Send + Sync {
    /// `Ok(None)` when the user is not an active member of the organization.
    async fn check(&self, user_id: Uuid, organization_id: Uuid) -> ApiResult<Option<MembershipInfo>>;
}

pub struct GrpcMembership {
    client: MembershipClient<InterceptedService<Channel, ServiceClientAuth>>,
    breaker: Arc<CircuitBreaker>,
}

impl GrpcMembership {
    pub fn new(url: &str, secret: &str) -> anyhow::Result<Self> {
        let auth = ServiceClientAuth::new(secret, crate::SERVICE_NAME, "user-service")?;
        Ok(Self {
            client: MembershipClient::with_interceptor(lazy_channel(url)?, auth),
            breaker: Arc::new(CircuitBreaker::default()),
        })
    }
}

#[async_trait]
impl MembershipChecker for GrpcMembership {
    async fn check(&self, user_id: Uuid, organization_id: Uuid) -> ApiResult<Option<MembershipInfo>> {
        if self.breaker.try_acquire().is_err() {
            return Err(ApiError::DependencyUnavailable("user-service"));
        }
        // Read-only RPC: safe to retry.
        let result = retry(RetryPolicy::default(), is_transient, || {
            let mut client = self.client.clone();
            async move {
                client
                    .check_membership(CheckMembershipRequest {
                        user_id: user_id.to_string(),
                        organization_id: organization_id.to_string(),
                    })
                    .await
            }
        })
        .await;
        self.breaker.record(result.as_ref().map_or_else(|s| !is_transient(s), |_| true));
        let reply = result.map_err(|s| from_status(&s, "user-service"))?.into_inner();

        if !reply.is_member || reply.status != "active" {
            return Ok(None);
        }
        let tenant_id = Uuid::parse_str(&reply.tenant_id)
            .map_err(|_| ApiError::internal(anyhow::anyhow!("user-service returned an invalid tenant id")))?;
        let role = OrgRole::parse(&reply.org_role)
            .ok_or_else(|| ApiError::internal(anyhow::anyhow!("user-service returned an unknown role")))?;
        Ok(Some(MembershipInfo { tenant_id, role }))
    }
}
