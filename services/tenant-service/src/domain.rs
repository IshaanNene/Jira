//! Tenant registry rules that do not need a database: tiers and placement decisions.

use std::fmt;

use error_handling::ApiError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    Standard,
    Business,
    Enterprise,
}

impl Tier {
    pub fn parse(value: &str) -> Result<Self, ApiError> {
        match value {
            "" | "standard" => Ok(Tier::Standard),
            "business" => Ok(Tier::Business),
            "enterprise" => Ok(Tier::Enterprise),
            _ => Err(ApiError::invalid("deployment_tier", "unknown_tier", "Tier must be standard, business or enterprise.")),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Tier::Standard => "standard",
            Tier::Business => "business",
            Tier::Enterprise => "enterprise",
        }
    }
}

impl fmt::Display for Tier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Outcome of deciding where a new tenant's data goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlacementDecision {
    pub cluster_id: Option<String>,
    pub destination: String,
    pub tenant_status: &'static str,
    pub provisioning_status: &'static str,
}

/// A shared cluster that could host a standard tenant.
#[derive(Debug, Clone)]
pub struct SharedCluster {
    pub id: String,
    pub destination: String,
    pub tenant_count: i64,
    pub capacity: i64,
}

/// * standard → least-loaded shared cluster with free capacity; usable immediately.
/// * business → a dedicated database; enterprise → a dedicated cluster. Both need
///   infrastructure to be created first, so the tenant starts in `provisioning/pending`
///   and is not routable until an operator workflow marks it provisioned.
pub fn decide_placement(
    tier: Tier,
    region: &str,
    tenant_id: uuid::Uuid,
    shared: &[SharedCluster],
) -> Result<PlacementDecision, ApiError> {
    match tier {
        Tier::Standard => {
            let cluster = shared
                .iter()
                .filter(|c| c.tenant_count < c.capacity)
                .min_by_key(|c| c.tenant_count)
                .ok_or(ApiError::TenantUnavailable("No database capacity is available in this region."))?;
            Ok(PlacementDecision {
                cluster_id: Some(cluster.id.clone()),
                destination: cluster.destination.clone(),
                tenant_status: "active",
                provisioning_status: "provisioned",
            })
        }
        Tier::Business => Ok(PlacementDecision {
            cluster_id: None,
            destination: format!("dedicated-db/{region}/{}", tenant_id.simple()),
            tenant_status: "provisioning",
            provisioning_status: "pending",
        }),
        Tier::Enterprise => Ok(PlacementDecision {
            cluster_id: None,
            destination: format!("dedicated-cluster/{region}/{}", tenant_id.simple()),
            tenant_status: "provisioning",
            provisioning_status: "pending",
        }),
    }
}

/// Regions are short lowercase identifiers such as `eu-west-1`.
pub fn validate_region(region: &str) -> Result<(), ApiError> {
    let ok = !region.is_empty()
        && region.len() <= 32
        && region.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    if ok {
        Ok(())
    } else {
        Err(ApiError::invalid("region", "invalid_region", "Region is not a valid identifier."))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cluster(id: &str, count: i64, capacity: i64) -> SharedCluster {
        SharedCluster { id: id.into(), destination: format!("shared/{id}"), tenant_count: count, capacity }
    }

    #[test]
    fn standard_goes_to_least_loaded_cluster_with_capacity() {
        let clusters = [cluster("a", 50, 100), cluster("b", 10, 100), cluster("full", 5, 5)];
        let d = decide_placement(Tier::Standard, "eu-1", uuid::Uuid::now_v7(), &clusters).unwrap();
        assert_eq!(d.cluster_id.as_deref(), Some("b"));
        assert_eq!(d.destination, "shared/b");
        assert_eq!((d.tenant_status, d.provisioning_status), ("active", "provisioned"));
    }

    #[test]
    fn standard_fails_when_every_cluster_is_full() {
        let clusters = [cluster("full", 5, 5)];
        assert!(decide_placement(Tier::Standard, "eu-1", uuid::Uuid::now_v7(), &clusters).is_err());
        assert!(decide_placement(Tier::Standard, "eu-1", uuid::Uuid::now_v7(), &[]).is_err());
    }

    #[test]
    fn dedicated_tiers_are_not_routable_until_provisioned() {
        let id = uuid::Uuid::now_v7();
        let b = decide_placement(Tier::Business, "eu-1", id, &[]).unwrap();
        assert!(b.destination.starts_with("dedicated-db/eu-1/"));
        assert_eq!(b.tenant_status, "provisioning");
        let e = decide_placement(Tier::Enterprise, "eu-1", id, &[]).unwrap();
        assert!(e.destination.starts_with("dedicated-cluster/eu-1/"));
        assert_eq!(e.provisioning_status, "pending");
    }

    #[test]
    fn tier_and_region_validation() {
        assert_eq!(Tier::parse("").unwrap(), Tier::Standard);
        assert!(Tier::parse("platinum").is_err());
        assert!(validate_region("eu-west-1").is_ok());
        assert!(validate_region("EU West; DROP").is_err());
        assert!(validate_region("").is_err());
    }
}
