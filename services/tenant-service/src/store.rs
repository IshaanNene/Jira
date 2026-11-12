//! Persistence for the tenant registry.

use error_handling::{ApiError, ApiResult};
use event_contracts::{outbox, payloads::TenantProvisionedV1, Actor, EventEnvelope};
use sqlx::{FromRow, PgPool};
use uuid::Uuid;

use crate::domain::{decide_placement, SharedCluster, Tier};

#[derive(Debug, Clone, FromRow, PartialEq, Eq)]
pub struct TenantRow {
    pub tenant_id: Uuid,
    pub organization_id: Uuid,
    pub deployment_tier: String,
    pub database_cluster_id: Option<String>,
    pub database_destination: String,
    pub region: String,
    pub tenant_status: String,
    pub provisioning_status: String,
    pub configuration_version: i64,
}

const TENANT_COLUMNS: &str = "tenant_id, organization_id, deployment_tier, database_cluster_id, \
     database_destination, region, tenant_status, provisioning_status, configuration_version";

/// Make sure the region has a shared cluster record pointing at the default destination.
/// Idempotent; run at startup.
pub async fn ensure_default_cluster(pool: &PgPool, region: &str, destination: &str) -> ApiResult<()> {
    sqlx::query(
        "INSERT INTO database_clusters (id, region, kind, database_destination) VALUES ($1, $2, 'shared', $3) \
         ON CONFLICT DO NOTHING",
    )
    .bind(format!("shared-{region}"))
    .bind(region)
    .bind(destination)
    .execute(pool)
    .await?;
    Ok(())
}

/// Create the tenant for an organization, or return the one that already exists
/// (idempotent on `organization_id`, so callers may retry safely).
pub async fn provision(pool: &PgPool, organization_id: Uuid, tier: Tier, region: &str) -> ApiResult<TenantRow> {
    let mut tx = pool.begin().await?;

    // Serialise placement decisions per region so two concurrent provisions see
    // consistent tenant counts.
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1))").bind(format!("placement:{region}")).execute(&mut *tx).await?;

    if let Some(existing) =
        sqlx::query_as::<_, TenantRow>(&format!("SELECT {TENANT_COLUMNS} FROM tenants WHERE organization_id = $1"))
            .bind(organization_id)
            .fetch_optional(&mut *tx)
            .await?
    {
        if existing.provisioning_status == "abandoned" {
            return Err(ApiError::Conflict("This organization's workspace was abandoned and cannot be reused.".into()));
        }
        return Ok(existing);
    }

    let shared: Vec<SharedCluster> = sqlx::query_as::<_, (String, String, i64, i32)>(
        "SELECT c.id, c.database_destination, count(t.tenant_id), c.tenant_capacity \
         FROM database_clusters c \
         LEFT JOIN tenants t ON t.database_cluster_id = c.id AND t.tenant_status <> 'deleted' \
         WHERE c.region = $1 AND c.kind = 'shared' AND c.status = 'active' \
         GROUP BY c.id",
    )
    .bind(region)
    .fetch_all(&mut *tx)
    .await?
    .into_iter()
    .map(|(id, destination, tenant_count, capacity)| SharedCluster {
        id,
        destination,
        tenant_count,
        capacity: i64::from(capacity),
    })
    .collect();

    let tenant_id = platform_common::new_id();
    let decision = decide_placement(tier, region, tenant_id, &shared)?;

    let row = sqlx::query_as::<_, TenantRow>(&format!(
        "INSERT INTO tenants (tenant_id, organization_id, deployment_tier, database_cluster_id, \
                              database_destination, region, tenant_status, provisioning_status) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8) RETURNING {TENANT_COLUMNS}"
    ))
    .bind(tenant_id)
    .bind(organization_id)
    .bind(tier.as_str())
    .bind(&decision.cluster_id)
    .bind(&decision.destination)
    .bind(region)
    .bind(decision.tenant_status)
    .bind(decision.provisioning_status)
    .fetch_one(&mut *tx)
    .await?;

    let event = EventEnvelope::new(
        Some(tenant_id),
        tenant_id,
        row.configuration_version,
        Actor::System("tenant-service".into()),
        TenantProvisionedV1 {
            tenant_id,
            organization_id,
            deployment_tier: row.deployment_tier.clone(),
            region: row.region.clone(),
            database_destination: row.database_destination.clone(),
        },
    );
    outbox::enqueue(&mut tx, &event).await?;
    tx.commit().await?;
    Ok(row)
}

pub async fn by_tenant(pool: &PgPool, tenant_id: Uuid) -> ApiResult<TenantRow> {
    sqlx::query_as::<_, TenantRow>(&format!("SELECT {TENANT_COLUMNS} FROM tenants WHERE tenant_id = $1"))
        .bind(tenant_id)
        .fetch_optional(pool)
        .await?
        .ok_or(ApiError::NotFound("tenant"))
}

pub async fn by_organization(pool: &PgPool, organization_id: Uuid) -> ApiResult<TenantRow> {
    sqlx::query_as::<_, TenantRow>(&format!("SELECT {TENANT_COLUMNS} FROM tenants WHERE organization_id = $1"))
        .bind(organization_id)
        .fetch_optional(pool)
        .await?
        .ok_or(ApiError::NotFound("tenant"))
}

/// Compensate a failed organization creation. Restricted to tenants created in the last
/// few minutes so this can never be used to delete an established tenant.
pub async fn abandon(pool: &PgPool, tenant_id: Uuid) -> ApiResult<()> {
    let result = sqlx::query(
        "UPDATE tenants SET tenant_status = 'deleted', provisioning_status = 'abandoned', \
                            configuration_version = configuration_version + 1, updated_at = now() \
         WHERE tenant_id = $1 AND provisioning_status <> 'abandoned' AND created_at > now() - interval '5 minutes'",
    )
    .bind(tenant_id)
    .execute(pool)
    .await?;
    if result.rows_affected() == 0 {
        // Either unknown, already abandoned (idempotent success) or too old (refused).
        let row = by_tenant(pool, tenant_id).await?;
        if row.provisioning_status != "abandoned" {
            return Err(ApiError::Conflict("Only a newly provisioned tenant can be abandoned.".into()));
        }
    }
    Ok(())
}
