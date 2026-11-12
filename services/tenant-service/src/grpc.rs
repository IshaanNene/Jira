use api_contracts::tenant::v1::{
    tenant_registry_server::TenantRegistry, AbandonTenantRequest, AbandonTenantResponse, GetTenantByOrganizationRequest,
    ProvisionTenantRequest, ResolveTenantRequest, Tenant,
};
use error_handling::ApiError;
use sqlx::PgPool;
use tonic::{Request, Response, Status};
use uuid::Uuid;

use crate::{
    domain::{validate_region, Tier},
    store::{self, TenantRow},
};

pub struct RegistryService {
    pub pool: PgPool,
    pub default_region: String,
}

fn parse_id(field: &'static str, value: &str) -> Result<Uuid, Status> {
    Uuid::parse_str(value).map_err(|_| Status::from(ApiError::invalid(field, "invalid_uuid", "Must be a UUID.")))
}

impl From<TenantRow> for Tenant {
    fn from(row: TenantRow) -> Self {
        Tenant {
            tenant_id: row.tenant_id.to_string(),
            organization_id: row.organization_id.to_string(),
            deployment_tier: row.deployment_tier,
            database_cluster_id: row.database_cluster_id.unwrap_or_default(),
            database_destination: row.database_destination,
            region: row.region,
            tenant_status: row.tenant_status,
            provisioning_status: row.provisioning_status,
            configuration_version: row.configuration_version,
        }
    }
}

#[tonic::async_trait]
impl TenantRegistry for RegistryService {
    async fn provision_tenant(&self, request: Request<ProvisionTenantRequest>) -> Result<Response<Tenant>, Status> {
        let req = request.into_inner();
        let organization_id = parse_id("organization_id", &req.organization_id)?;
        let tier = Tier::parse(&req.deployment_tier)?;
        let region = if req.region.is_empty() { self.default_region.clone() } else { req.region };
        validate_region(&region)?;
        let row = store::provision(&self.pool, organization_id, tier, &region).await?;
        tracing::info!(tenant_id = %row.tenant_id, %organization_id, tier = %tier, region, "tenant provisioned");
        Ok(Response::new(row.into()))
    }

    async fn resolve_tenant(&self, request: Request<ResolveTenantRequest>) -> Result<Response<Tenant>, Status> {
        let tenant_id = parse_id("tenant_id", &request.into_inner().tenant_id)?;
        Ok(Response::new(store::by_tenant(&self.pool, tenant_id).await?.into()))
    }

    async fn get_tenant_by_organization(
        &self,
        request: Request<GetTenantByOrganizationRequest>,
    ) -> Result<Response<Tenant>, Status> {
        let organization_id = parse_id("organization_id", &request.into_inner().organization_id)?;
        Ok(Response::new(store::by_organization(&self.pool, organization_id).await?.into()))
    }

    async fn abandon_tenant(&self, request: Request<AbandonTenantRequest>) -> Result<Response<AbandonTenantResponse>, Status> {
        let tenant_id = parse_id("tenant_id", &request.into_inner().tenant_id)?;
        store::abandon(&self.pool, tenant_id).await?;
        tracing::warn!(%tenant_id, "tenant abandoned");
        Ok(Response::new(AbandonTenantResponse {}))
    }
}
