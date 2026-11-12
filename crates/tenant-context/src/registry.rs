//! Placement lookups against the tenant registry service.

use std::sync::Arc;

use api_contracts::tenant::v1::{tenant_registry_client::TenantRegistryClient, ResolveTenantRequest};
use async_trait::async_trait;
use auth_middleware::service_auth::{is_transient, lazy_channel, ServiceClientAuth};
use error_handling::{from_status, ApiError};
use platform_common::resilience::{retry, CircuitBreaker, RetryPolicy};
use tonic::{service::interceptor::InterceptedService, transport::Channel};
use uuid::Uuid;

use crate::router::{Placement, PlacementResolver};

pub type RegistryClient = TenantRegistryClient<InterceptedService<Channel, ServiceClientAuth>>;

/// Resolves tenant placement over authenticated gRPC, with bounded retry and a circuit
/// breaker. `DbRouter` caches the result, so this is not on the per-request path.
pub struct RegistryPlacement {
    client: RegistryClient,
    breaker: Arc<CircuitBreaker>,
}

impl RegistryPlacement {
    pub fn new(url: &str, secret: &str, caller: &'static str) -> anyhow::Result<Self> {
        Ok(Self { client: Self::client(url, secret, caller)?, breaker: Arc::new(CircuitBreaker::default()) })
    }

    /// An authenticated registry client for callers that need RPCs beyond `ResolveTenant`.
    pub fn client(url: &str, secret: &str, caller: &'static str) -> anyhow::Result<RegistryClient> {
        let auth = ServiceClientAuth::new(secret, caller, "tenant-service")?;
        Ok(TenantRegistryClient::with_interceptor(lazy_channel(url)?, auth))
    }
}

#[async_trait]
impl PlacementResolver for RegistryPlacement {
    async fn resolve(&self, tenant_id: Uuid) -> Result<Placement, ApiError> {
        if self.breaker.try_acquire().is_err() {
            return Err(ApiError::DependencyUnavailable("tenant-service"));
        }
        let result = retry(RetryPolicy::default(), is_transient, || {
            let mut client = self.client.clone();
            async move { client.resolve_tenant(ResolveTenantRequest { tenant_id: tenant_id.to_string() }).await }
        })
        .await;
        self.breaker.record(result.as_ref().map_or_else(|s| !is_transient(s), |_| true));
        let tenant = result.map_err(|s| from_status(&s, "tenant-service"))?.into_inner();
        Ok(Placement {
            destination: tenant.database_destination,
            tenant_status: tenant.tenant_status,
            configuration_version: tenant.configuration_version,
        })
    }
}
