//! Client side of tenant provisioning (the first step of creating an organization).

use std::sync::Arc;

use api_contracts::tenant::v1::{AbandonTenantRequest, ProvisionTenantRequest};
use async_trait::async_trait;
use auth_middleware::service_auth::is_transient;
use error_handling::{from_status, ApiError, ApiResult};
use platform_common::resilience::{retry, CircuitBreaker, RetryPolicy};
use tenant_context::RegistryPlacement;
use uuid::Uuid;

#[async_trait]
pub trait TenantProvisioner: Send + Sync {
    /// Idempotent on `organization_id`. Returns the tenant id once the tenant is usable.
    async fn provision(&self, organization_id: Uuid) -> ApiResult<Uuid>;
    /// Best-effort compensation when the organization could not be created.
    async fn abandon(&self, tenant_id: Uuid);
}

pub struct GrpcProvisioner {
    client: tenant_context::RegistryClient,
    breaker: Arc<CircuitBreaker>,
}

impl GrpcProvisioner {
    pub fn new(url: &str, secret: &str) -> anyhow::Result<Self> {
        Ok(Self {
            client: RegistryPlacement::client(url, secret, crate::SERVICE_NAME)?,
            breaker: Arc::new(CircuitBreaker::default()),
        })
    }
}

#[async_trait]
impl TenantProvisioner for GrpcProvisioner {
    async fn provision(&self, organization_id: Uuid) -> ApiResult<Uuid> {
        if self.breaker.try_acquire().is_err() {
            return Err(ApiError::DependencyUnavailable("tenant-service"));
        }
        // Safe to retry: the registry deduplicates on organization_id.
        let result = retry(RetryPolicy::default(), is_transient, || {
            let mut client = self.client.clone();
            async move {
                client
                    .provision_tenant(ProvisionTenantRequest {
                        organization_id: organization_id.to_string(),
                        deployment_tier: "standard".to_owned(),
                        region: String::new(),
                    })
                    .await
            }
        })
        .await;
        self.breaker.record(result.as_ref().map_or_else(|s| !is_transient(s), |_| true));
        let tenant = result.map_err(|s| from_status(&s, "tenant-service"))?.into_inner();
        if tenant.tenant_status != "active" {
            return Err(ApiError::TenantUnavailable("The workspace is still being provisioned."));
        }
        Uuid::parse_str(&tenant.tenant_id)
            .map_err(|_| ApiError::internal(anyhow::anyhow!("tenant-service returned an invalid tenant id")))
    }

    async fn abandon(&self, tenant_id: Uuid) {
        let mut client = self.client.clone();
        if let Err(status) = client.abandon_tenant(AbandonTenantRequest { tenant_id: tenant_id.to_string() }).await {
            // Leaves an orphan registry row with no organization behind it: harmless to
            // users, visible to operators. Logged so it can be reconciled.
            tracing::error!(%tenant_id, code = ?status.code(), "could not abandon tenant after failed organization creation");
        }
    }
}
