//! Integration tests against a real PostgreSQL (see `make test`).

use tenant_service::{
    domain::Tier,
    store::{self, TenantRow},
    MIGRATOR,
};
use test_utilities::TestDb;
use uuid::Uuid;

async fn db() -> TestDb {
    let db = TestDb::create(&MIGRATOR).await.expect("test database");
    store::ensure_default_cluster(&db.app, "local-1", "shared/default").await.unwrap();
    db
}

#[tokio::test]
async fn provision_is_idempotent_per_organization() {
    let db = db().await;
    let org = Uuid::now_v7();
    let first = store::provision(&db.app, org, Tier::Standard, "local-1").await.unwrap();
    let second = store::provision(&db.app, org, Tier::Standard, "local-1").await.unwrap();
    assert_eq!(first, second);
    assert_eq!(first.database_destination, "shared/default");
    assert_eq!((first.tenant_status.as_str(), first.provisioning_status.as_str()), ("active", "provisioned"));

    let (events,): (i64,) =
        sqlx::query_as("SELECT count(*) FROM outbox WHERE event_type = 'tenant.provisioned'").fetch_one(&db.app).await.unwrap();
    assert_eq!(events, 1, "the retry must not emit a second event");
    db.cleanup().await;
}

#[tokio::test]
async fn concurrent_provisions_for_one_organization_yield_one_tenant() {
    let db = db().await;
    let org = Uuid::now_v7();
    let tasks: Vec<_> = (0..8)
        .map(|_| {
            let pool = db.app.clone();
            tokio::spawn(async move { store::provision(&pool, org, Tier::Standard, "local-1").await })
        })
        .collect();
    let mut rows: Vec<TenantRow> = Vec::new();
    for t in tasks {
        rows.push(t.await.unwrap().unwrap());
    }
    assert!(rows.windows(2).all(|w| w[0].tenant_id == w[1].tenant_id));
    let (count,): (i64,) = sqlx::query_as("SELECT count(*) FROM tenants").fetch_one(&db.app).await.unwrap();
    assert_eq!(count, 1);
    db.cleanup().await;
}

#[tokio::test]
async fn resolve_by_tenant_and_organization() {
    let db = db().await;
    let org = Uuid::now_v7();
    let created = store::provision(&db.app, org, Tier::Standard, "local-1").await.unwrap();
    assert_eq!(store::by_tenant(&db.app, created.tenant_id).await.unwrap(), created);
    assert_eq!(store::by_organization(&db.app, org).await.unwrap(), created);
    assert!(store::by_tenant(&db.app, Uuid::now_v7()).await.is_err());
    db.cleanup().await;
}

#[tokio::test]
async fn dedicated_tiers_start_unroutable() {
    let db = db().await;
    let row = store::provision(&db.app, Uuid::now_v7(), Tier::Enterprise, "local-1").await.unwrap();
    assert_eq!(row.tenant_status, "provisioning");
    assert_eq!(row.provisioning_status, "pending");
    assert!(row.database_destination.starts_with("dedicated-cluster/local-1/"));
    assert!(row.database_cluster_id.is_none());
    db.cleanup().await;
}

#[tokio::test]
async fn region_without_capacity_is_refused() {
    let db = db().await;
    assert!(store::provision(&db.app, Uuid::now_v7(), Tier::Standard, "mars-1").await.is_err());
    db.cleanup().await;
}

#[tokio::test]
async fn abandon_only_applies_to_fresh_tenants() {
    let db = db().await;
    let org = Uuid::now_v7();
    let fresh = store::provision(&db.app, org, Tier::Standard, "local-1").await.unwrap();
    store::abandon(&db.app, fresh.tenant_id).await.unwrap();
    store::abandon(&db.app, fresh.tenant_id).await.unwrap(); // idempotent
    assert_eq!(store::by_tenant(&db.app, fresh.tenant_id).await.unwrap().tenant_status, "deleted");
    assert!(store::provision(&db.app, org, Tier::Standard, "local-1").await.is_err(), "abandoned org id is not reused");

    let old = store::provision(&db.app, Uuid::now_v7(), Tier::Standard, "local-1").await.unwrap();
    sqlx::query("UPDATE tenants SET created_at = now() - interval '1 hour' WHERE tenant_id = $1")
        .bind(old.tenant_id)
        .execute(&db.owner)
        .await
        .unwrap();
    assert!(store::abandon(&db.app, old.tenant_id).await.is_err());
    assert_eq!(store::by_tenant(&db.app, old.tenant_id).await.unwrap().tenant_status, "active");
    db.cleanup().await;
}
