//! Tenant-scoped database access.
//!
//! [`DbRouter::begin`] is the only way services open a transaction for tenant-owned data.
//! It (1) resolves where the tenant's data lives from trusted registry metadata,
//! (2) picks the matching connection pool, and (3) sets `app.tenant_id` for the
//! transaction so PostgreSQL row-level security applies to every statement in it.

mod pool;
mod registry;
mod router;

pub use pool::{connect_pool, destination_key, pools_from_env, PoolSettings, DEFAULT_DESTINATION};
pub use registry::{RegistryClient, RegistryPlacement};
pub use router::{DbRouter, Placement, PlacementResolver, StaticPlacement, TenantTx};
