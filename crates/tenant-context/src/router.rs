use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
    time::{Duration, Instant},
};

use async_trait::async_trait;
use error_handling::ApiError;
use sqlx::{PgConnection, PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::pool::{destination_key, DEFAULT_DESTINATION};

/// Where a tenant's data lives and whether it may be used right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placement {
    /// Logical destination name from the tenant registry (never a connection string).
    pub destination: String,
    pub tenant_status: String,
    pub configuration_version: i64,
}

#[async_trait]
pub trait PlacementResolver: Send + Sync {
    async fn resolve(&self, tenant_id: Uuid) -> Result<Placement, ApiError>;
}

/// Every tenant lives in the default shared destination. For services started without a
/// registry (tests) — production services resolve through the tenant registry.
pub struct StaticPlacement;

#[async_trait]
impl PlacementResolver for StaticPlacement {
    async fn resolve(&self, _tenant_id: Uuid) -> Result<Placement, ApiError> {
        Ok(Placement {
            destination: DEFAULT_DESTINATION.to_owned(),
            tenant_status: "active".to_owned(),
            configuration_version: 1,
        })
    }
}

pub struct DbRouter {
    pools: HashMap<String, PgPool>,
    resolver: Arc<dyn PlacementResolver>,
    cache: RwLock<HashMap<Uuid, (Instant, Placement)>>,
    ttl: Duration,
}

impl DbRouter {
    pub fn new(pools: HashMap<String, PgPool>, resolver: Arc<dyn PlacementResolver>) -> Self {
        Self { pools, resolver, cache: RwLock::new(HashMap::new()), ttl: Duration::from_secs(60) }
    }

    /// Single shared pool, static placement.
    pub fn single(pool: PgPool) -> Self {
        let mut pools = HashMap::new();
        pools.insert(destination_key(DEFAULT_DESTINATION), pool);
        Self::new(pools, Arc::new(StaticPlacement))
    }

    /// The default pool, for the few tables that are deliberately not tenant-owned.
    /// Row-level security still hides tenant-owned rows on this path because no tenant is set.
    pub fn global(&self) -> Result<&PgPool, ApiError> {
        self.pools
            .get(&destination_key(DEFAULT_DESTINATION))
            .ok_or_else(|| ApiError::internal(anyhow::anyhow!("default database destination is not configured")))
    }

    /// Every configured database, for work that spans tenants (the outbox relay).
    pub fn pools(&self) -> Vec<PgPool> {
        self.pools.values().cloned().collect()
    }

    /// Drop a cached placement (called when `tenant.placement_changed` is observed).
    pub fn invalidate(&self, tenant_id: Uuid) {
        if let Ok(mut cache) = self.cache.write() {
            cache.remove(&tenant_id);
        }
    }

    async fn placement(&self, tenant_id: Uuid) -> Result<Placement, ApiError> {
        if let Ok(cache) = self.cache.read() {
            if let Some((at, placement)) = cache.get(&tenant_id) {
                if at.elapsed() < self.ttl {
                    return Ok(placement.clone());
                }
            }
        }
        let placement = self.resolver.resolve(tenant_id).await?;
        if let Ok(mut cache) = self.cache.write() {
            cache.insert(tenant_id, (Instant::now(), placement.clone()));
        }
        Ok(placement)
    }

    /// Open a transaction bound to `tenant_id`. The caller must have obtained the id from a
    /// verified token, verified service metadata or a validated event envelope.
    pub async fn begin(&self, tenant_id: Uuid) -> Result<TenantTx, ApiError> {
        let placement = self.placement(tenant_id).await?;
        match placement.tenant_status.as_str() {
            "active" => {}
            "migrating" => return Err(ApiError::TenantUnavailable("This workspace is being migrated. Try again shortly.")),
            "suspended" => return Err(ApiError::TenantUnavailable("This workspace is suspended.")),
            _ => return Err(ApiError::NotFound("workspace")),
        }
        let pool = self.pools.get(&destination_key(&placement.destination)).ok_or_else(|| {
            ApiError::internal(anyhow::anyhow!("no connection configured for database destination {:?}", placement.destination))
        })?;
        let mut tx = pool.begin().await?;
        // Transaction-local: cannot leak to the next user of this pooled connection.
        sqlx::query("SELECT set_config('app.tenant_id', $1, true)").bind(tenant_id.to_string()).execute(&mut *tx).await?;
        Ok(TenantTx { tx, tenant_id })
    }
}

/// A transaction in which row-level security is scoped to one tenant.
pub struct TenantTx {
    tx: Transaction<'static, Postgres>,
    tenant_id: Uuid,
}

impl TenantTx {
    pub fn tenant_id(&self) -> Uuid {
        self.tenant_id
    }

    pub fn conn(&mut self) -> &mut PgConnection {
        &mut self.tx
    }

    pub async fn commit(self) -> Result<(), ApiError> {
        self.tx.commit().await.map_err(ApiError::from)
    }

    pub async fn rollback(self) -> Result<(), ApiError> {
        self.tx.rollback().await.map_err(ApiError::from)
    }
}
