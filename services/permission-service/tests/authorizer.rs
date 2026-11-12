use std::sync::Arc;

use api_contracts::permission::v1::{
    authorizer_server::Authorizer, DeleteProjectRequest, GetPolicySnapshotRequest, ListProjectMembersRequest,
    RemoveProjectRoleRequest, SetProjectDefaultRoleRequest, SetProjectRoleRequest,
};
use auth_middleware::RedisHandle;
use permission_service::{grpc::AuthorizerService, MIGRATOR};
use tenant_context::DbRouter;
use test_utilities::TestDb;
use tonic::Request;
use uuid::Uuid;

async fn service() -> (TestDb, AuthorizerService) {
    let db = TestDb::create(&MIGRATOR).await.expect("test database");
    let svc = AuthorizerService { db: Arc::new(DbRouter::single(db.app.clone())), redis: RedisHandle::default() };
    (db, svc)
}

async fn set(svc: &AuthorizerService, tenant: Uuid, project: Uuid, user: Uuid, role: &str) -> Result<i64, tonic::Status> {
    svc.set_project_role(Request::new(SetProjectRoleRequest {
        tenant_id: tenant.to_string(),
        project_id: project.to_string(),
        user_id: user.to_string(),
        role: role.into(),
    }))
    .await
    .map(|r| r.into_inner().version)
}

async fn snapshot(svc: &AuthorizerService, tenant: Uuid, user: Uuid) -> api_contracts::permission::v1::PolicySnapshot {
    svc.get_policy_snapshot(Request::new(GetPolicySnapshotRequest { tenant_id: tenant.to_string(), user_id: user.to_string() }))
        .await
        .unwrap()
        .into_inner()
}

#[tokio::test]
async fn bindings_versions_and_snapshots() {
    let (db, svc) = service().await;
    let (tenant, project, other_project, ada, bob) =
        (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());

    assert_eq!(snapshot(&svc, tenant, ada).await.version, 0);
    assert_eq!(set(&svc, tenant, project, ada, "administrator").await.unwrap(), 1);
    assert_eq!(set(&svc, tenant, project, ada, "administrator").await.unwrap(), 1, "no-op does not bump the version");
    assert_eq!(set(&svc, tenant, project, ada, "member").await.unwrap(), 2);
    assert_eq!(set(&svc, tenant, other_project, bob, "viewer").await.unwrap(), 3);
    assert!(set(&svc, tenant, project, bob, "root").await.is_err());

    let version = svc
        .set_project_default_role(Request::new(SetProjectDefaultRoleRequest {
            tenant_id: tenant.to_string(),
            project_id: project.to_string(),
            role: "viewer".into(),
        }))
        .await
        .unwrap()
        .into_inner()
        .version;
    assert_eq!(version, 4);

    let snap = snapshot(&svc, tenant, ada).await;
    assert_eq!(snap.version, 4);
    assert_eq!(snap.roles.len(), 1, "only Ada's own bindings");
    assert_eq!((snap.roles[0].project_id.as_str(), snap.roles[0].role.as_str()), (project.to_string().as_str(), "member"));
    assert_eq!(snap.default_roles[0].role, "viewer");

    let members = svc
        .list_project_members(Request::new(ListProjectMembersRequest {
            tenant_id: tenant.to_string(),
            project_id: project.to_string(),
        }))
        .await
        .unwrap()
        .into_inner();
    assert_eq!((members.members.len(), members.default_role.as_str()), (1, "viewer"));

    svc.remove_project_role(Request::new(RemoveProjectRoleRequest {
        tenant_id: tenant.to_string(),
        project_id: project.to_string(),
        user_id: ada.to_string(),
    }))
    .await
    .unwrap();
    assert!(snapshot(&svc, tenant, ada).await.roles.is_empty());

    svc.delete_project(Request::new(DeleteProjectRequest { tenant_id: tenant.to_string(), project_id: project.to_string() }))
        .await
        .unwrap();
    assert!(snapshot(&svc, tenant, ada).await.default_roles.is_empty());

    let (events,): (i64,) =
        sqlx::query_as("SELECT count(*) FROM outbox WHERE event_type = 'permission.updated'").fetch_one(&db.owner).await.unwrap();
    assert_eq!(events, 6, "one event per real change");
    db.cleanup().await;
}

#[tokio::test]
async fn tenants_never_see_each_others_bindings() {
    let (db, svc) = service().await;
    let (tenant_a, tenant_b, project, user) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    set(&svc, tenant_a, project, user, "administrator").await.unwrap();

    // Same user and project ids asked for under another tenant: nothing.
    let other = snapshot(&svc, tenant_b, user).await;
    assert!(other.roles.is_empty());
    assert_eq!(other.version, 0);

    let (visible,): (i64,) = sqlx::query_as("SELECT count(*) FROM project_role_bindings").fetch_one(&db.app).await.unwrap();
    assert_eq!(visible, 0, "no tenant set, no rows");
    let unprotected: Vec<(String,)> = sqlx::query_as(
        "SELECT c.relname::text FROM pg_class c JOIN pg_attribute a ON a.attrelid = c.oid AND a.attname = 'tenant_id' \
         WHERE c.relkind = 'r' AND c.relnamespace = 'public'::regnamespace AND c.relname <> 'outbox' \
           AND NOT (c.relrowsecurity AND c.relforcerowsecurity)",
    )
    .fetch_all(&db.owner)
    .await
    .unwrap();
    assert!(unprotected.is_empty(), "{unprotected:?}");
    db.cleanup().await;
}
