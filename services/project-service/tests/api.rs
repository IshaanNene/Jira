use std::{
    collections::HashSet,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use auth_middleware::{PolicyClient, PolicySnapshot, PolicySource, ProjectRole, RedisHandle};
use axum::{
    body::Body,
    http::{header, Method, Request, StatusCode},
    Router,
};
use error_handling::ApiResult;
use project_service::{
    access::{MemberList, ProjectAccess},
    http, AppState, MIGRATOR,
};
use serde_json::{json, Value};
use tenant_context::DbRouter;
use test_utilities::{TestAuth, TestDb};
use tower::ServiceExt;
use uuid::Uuid;

/// In-memory stand-in for the permission and user services, shared by the policy
/// source so that grants made through the API are what authorization then sees.
#[derive(Default)]
struct FakeAccess {
    roles: Mutex<Vec<(Uuid, Uuid, Uuid, ProjectRole)>>, // tenant, project, user, role
    defaults: Mutex<Vec<(Uuid, Uuid, ProjectRole)>>,
    org_members: Mutex<HashSet<Uuid>>,
}

#[async_trait]
impl ProjectAccess for FakeAccess {
    async fn set_role(&self, tenant: Uuid, project: Uuid, user: Uuid, role: ProjectRole) -> ApiResult<()> {
        let mut roles = self.roles.lock().unwrap();
        roles.retain(|(t, p, u, _)| !(*t == tenant && *p == project && *u == user));
        roles.push((tenant, project, user, role));
        Ok(())
    }
    async fn remove_role(&self, tenant: Uuid, project: Uuid, user: Uuid) -> ApiResult<()> {
        self.roles.lock().unwrap().retain(|(t, p, u, _)| !(*t == tenant && *p == project && *u == user));
        Ok(())
    }
    async fn set_default_role(&self, tenant: Uuid, project: Uuid, role: Option<ProjectRole>) -> ApiResult<()> {
        let mut defaults = self.defaults.lock().unwrap();
        defaults.retain(|(t, p, _)| !(*t == tenant && *p == project));
        if let Some(role) = role {
            defaults.push((tenant, project, role));
        }
        Ok(())
    }
    async fn members(&self, tenant: Uuid, project: Uuid) -> ApiResult<MemberList> {
        Ok(MemberList {
            members: self
                .roles
                .lock()
                .unwrap()
                .iter()
                .filter(|(t, p, ..)| *t == tenant && *p == project)
                .map(|(_, _, u, r)| (*u, *r))
                .collect(),
            default_role: self
                .defaults
                .lock()
                .unwrap()
                .iter()
                .find(|(t, p, _)| *t == tenant && *p == project)
                .map(|(_, _, r)| *r),
        })
    }
    async fn delete_project(&self, tenant: Uuid, project: Uuid) -> ApiResult<()> {
        self.roles.lock().unwrap().retain(|(t, p, ..)| !(*t == tenant && *p == project));
        self.defaults.lock().unwrap().retain(|(t, p, _)| !(*t == tenant && *p == project));
        Ok(())
    }
    async fn is_org_member(&self, user: Uuid, _organization: Uuid) -> ApiResult<bool> {
        Ok(self.org_members.lock().unwrap().contains(&user))
    }
}

#[async_trait]
impl PolicySource for FakeAccess {
    async fn fetch(&self, tenant_id: Uuid, user_id: Uuid) -> ApiResult<PolicySnapshot> {
        Ok(PolicySnapshot {
            version: 0,
            roles: self
                .roles
                .lock()
                .unwrap()
                .iter()
                .filter(|(t, _, u, _)| *t == tenant_id && *u == user_id)
                .map(|(_, p, _, r)| (*p, *r))
                .collect(),
            default_roles: self
                .defaults
                .lock()
                .unwrap()
                .iter()
                .filter(|(t, ..)| *t == tenant_id)
                .map(|(_, p, r)| (*p, *r))
                .collect(),
        })
    }
}

struct Harness {
    db: TestDb,
    app: Router,
    auth: TestAuth,
    access: Arc<FakeAccess>,
    tenant: Uuid,
    org: Uuid,
}

struct Reply {
    status: StatusCode,
    body: Value,
    etag: Option<String>,
}

async fn harness() -> Harness {
    let db = TestDb::create(&MIGRATOR).await.expect("test database");
    let auth = TestAuth::new().unwrap();
    let access = Arc::new(FakeAccess::default());
    let state = AppState {
        db: Arc::new(DbRouter::single(db.app.clone())),
        verifier: Arc::clone(&auth.verifier),
        // A fresh client per request path would hide caching bugs; this one is shared like in production.
        policy: Arc::new(PolicyClient::new(access.clone(), RedisHandle::default())),
        access: access.clone(),
        allowed_origins: Arc::new(vec![]),
    };
    Harness { db, app: http::router(state), auth, access, tenant: Uuid::now_v7(), org: Uuid::now_v7() }
}

impl Harness {
    /// A user in this harness's organization, with an organization token for `org_role`.
    fn user(&self, org_role: &str) -> (Uuid, String) {
        let id = Uuid::now_v7();
        self.access.org_members.lock().unwrap().insert(id);
        (id, self.auth.org_token(id, &format!("{id}@example.com"), self.tenant, self.org, org_role).unwrap())
    }

    async fn call(&self, method: Method, path: &str, token: &str, body: Option<Value>, if_match: Option<&str>) -> Reply {
        let mut req = Request::builder()
            .method(method)
            .uri(path)
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::AUTHORIZATION, format!("Bearer {token}"));
        if let Some(v) = if_match {
            req = req.header(header::IF_MATCH, v);
        }
        let response = self
            .app
            .clone()
            .oneshot(req.body(body.map_or_else(Body::empty, |b| Body::from(b.to_string()))).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let etag = response.headers().get(header::ETAG).map(|v| v.to_str().unwrap().to_owned());
        let bytes = axum::body::to_bytes(response.into_body(), 1 << 20).await.unwrap();
        Reply { status, body: serde_json::from_slice(&bytes).unwrap_or(Value::Null), etag }
    }

    async fn create(&self, token: &str, key: &str, access: &str) -> Reply {
        self.call(
            Method::POST,
            "/api/v1/projects",
            token,
            Some(json!({ "name": format!("Project {key}"), "key": key, "project_type": "scrum", "access": access })),
            None,
        )
        .await
    }
}

fn keys(reply: &Reply) -> Vec<String> {
    reply.body["items"].as_array().unwrap().iter().map(|p| p["key"].as_str().unwrap().to_owned()).collect()
}

#[tokio::test]
async fn create_validates_and_grants_the_creator_administration() {
    let h = harness().await;
    let (_, member) = h.user("member");
    let (_, guest) = h.user("guest");

    let created = h.create(&member, "pay", "private").await;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.body);
    assert_eq!((created.body["key"].as_str(), created.body["my_role"].as_str()), (Some("PAY"), Some("administrator")));
    assert_eq!(created.etag.as_deref(), Some("\"1\""));

    assert_eq!(h.create(&member, "PAY", "open").await.status, StatusCode::CONFLICT, "keys are unique per tenant");
    assert_eq!(h.create(&guest, "GST", "open").await.status, StatusCode::FORBIDDEN, "guests cannot create projects");
    for bad in [
        json!({ "name": "X", "key": "OK", "project_type": "scrum", "access": "open" }),
        json!({ "name": "Fine", "key": "1BAD", "project_type": "scrum", "access": "open" }),
        json!({ "name": "Fine", "key": "OK", "project_type": "waterfall", "access": "open" }),
        json!({ "name": "Fine", "key": "OK", "project_type": "scrum", "access": "everyone" }),
    ] {
        assert_eq!(
            h.call(Method::POST, "/api/v1/projects", &member, Some(bad), None).await.status,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    let (events,): (i64,) =
        sqlx::query_as("SELECT count(*) FROM outbox WHERE event_type = 'project.created'").fetch_one(&h.db.owner).await.unwrap();
    assert_eq!(events, 1);
    h.db.cleanup().await;
}

#[tokio::test]
async fn private_projects_are_invisible_without_access() {
    let h = harness().await;
    let (_, ada) = h.user("member");
    let (bob_id, bob) = h.user("member");
    let (_, guest) = h.user("guest");
    let (_, org_admin) = h.user("admin");
    h.create(&ada, "SEC", "private").await;
    h.create(&ada, "PUB", "open").await;

    assert_eq!(keys(&h.call(Method::GET, "/api/v1/projects", &ada, None, None).await), ["PUB", "SEC"]);
    assert_eq!(
        keys(&h.call(Method::GET, "/api/v1/projects", &bob, None, None).await),
        ["PUB"],
        "open projects reach every member"
    );
    assert!(keys(&h.call(Method::GET, "/api/v1/projects", &guest, None, None).await).is_empty(), "guests get no defaults");
    assert_eq!(keys(&h.call(Method::GET, "/api/v1/projects", &org_admin, None, None).await), ["PUB", "SEC"]);

    // Not "forbidden": the private project's existence is not confirmed.
    for path in ["/api/v1/projects/SEC", "/api/v1/projects/SEC/components", "/api/v1/projects/SEC/members"] {
        assert_eq!(h.call(Method::GET, path, &bob, None, None).await.status, StatusCode::NOT_FOUND, "{path}");
    }
    assert_eq!(h.call(Method::POST, "/api/v1/projects/SEC/archive", &bob, None, None).await.status, StatusCode::NOT_FOUND);

    // Members of an open project can browse but not administer it.
    let open = h.call(Method::GET, "/api/v1/projects/PUB", &bob, None, None).await;
    assert_eq!(open.body["my_role"], "member");
    let denied = h.call(Method::PATCH, "/api/v1/projects/PUB", &bob, Some(json!({ "name": "Mine now" })), Some("\"1\"")).await;
    assert_eq!(denied.status, StatusCode::FORBIDDEN);
    assert_eq!(
        h.call(
            Method::PUT,
            &format!("/api/v1/projects/PUB/members/{bob_id}"),
            &bob,
            Some(json!({ "role": "administrator" })),
            None
        )
        .await
        .status,
        StatusCode::FORBIDDEN,
        "no self-promotion"
    );

    // Granting access makes the private project appear.
    let granted = h
        .call(Method::PUT, &format!("/api/v1/projects/SEC/members/{bob_id}"), &ada, Some(json!({ "role": "viewer" })), None)
        .await;
    assert_eq!(granted.status, StatusCode::NO_CONTENT);
    assert_eq!(h.call(Method::GET, "/api/v1/projects/SEC", &bob, None, None).await.body["my_role"], "viewer");
    let stranger = Uuid::now_v7();
    let outsider = h
        .call(Method::PUT, &format!("/api/v1/projects/SEC/members/{stranger}"), &ada, Some(json!({ "role": "member" })), None)
        .await;
    assert_eq!(outsider.status, StatusCode::UNPROCESSABLE_ENTITY, "only organization members can be added");

    let members = h.call(Method::GET, "/api/v1/projects/SEC/members", &ada, None, None).await;
    assert_eq!(members.body["members"].as_array().unwrap().len(), 2);
    assert!(members.body["default_role"].is_null());

    // Closing an open project removes it from other members' lists.
    let closed = h.call(Method::PUT, "/api/v1/projects/PUB/access", &ada, Some(json!({ "default_role": null })), None).await;
    assert_eq!(closed.status, StatusCode::OK, "{}", closed.body);
    assert_eq!(keys(&h.call(Method::GET, "/api/v1/projects", &bob, None, None).await), ["SEC"]);
    assert_eq!(
        h.call(Method::PUT, "/api/v1/projects/PUB/access", &ada, Some(json!({ "default_role": "administrator" })), None)
            .await
            .status,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    h.db.cleanup().await;
}

#[tokio::test]
async fn tenants_are_isolated() {
    let h = harness().await;
    let (_, ada) = h.user("owner");
    h.create(&ada, "PAY", "open").await;
    // An organization owner of a different tenant, same service and database.
    let other = h.auth.org_token(Uuid::now_v7(), "o@example.com", Uuid::now_v7(), Uuid::now_v7(), "owner").unwrap();
    assert!(keys(&h.call(Method::GET, "/api/v1/projects", &other, None, None).await).is_empty());
    assert_eq!(h.call(Method::GET, "/api/v1/projects/PAY", &other, None, None).await.status, StatusCode::NOT_FOUND);
    assert_eq!(h.create(&other, "PAY", "open").await.status, StatusCode::CREATED, "the same key is free in another tenant");
    let (visible,): (i64,) = sqlx::query_as("SELECT count(*) FROM projects").fetch_one(&h.db.app).await.unwrap();
    assert_eq!(visible, 0, "no tenant set, no rows");
    h.db.cleanup().await;
}

#[tokio::test]
async fn update_archive_restore_delete() {
    let h = harness().await;
    let (ada_id, ada) = h.user("member");
    let (_, org_admin) = h.user("admin");
    h.create(&ada, "PAY", "open").await;
    let path = "/api/v1/projects/PAY";

    assert_eq!(
        h.call(Method::PATCH, path, &ada, Some(json!({ "name": "Payments" })), None).await.status,
        StatusCode::PRECONDITION_REQUIRED
    );
    let updated =
        h.call(Method::PATCH, path, &ada, Some(json!({ "name": "Payments", "lead_user_id": ada_id })), Some("\"1\"")).await;
    assert_eq!(updated.status, StatusCode::OK, "{}", updated.body);
    assert_eq!((updated.body["name"].as_str(), updated.body["version"].as_i64()), (Some("Payments"), Some(2)));
    let stale = h.call(Method::PATCH, path, &ada, Some(json!({ "name": "Other" })), Some("\"1\"")).await;
    assert_eq!(stale.status, StatusCode::CONFLICT);
    assert_eq!(stale.body["current"]["name"], "Payments");
    // null clears the lead; omitting it leaves it alone.
    let kept = h.call(Method::PATCH, path, &ada, Some(json!({ "description": "Money" })), Some("\"2\"")).await;
    assert_eq!(kept.body["lead_user_id"], ada_id.to_string());
    let cleared = h.call(Method::PATCH, path, &ada, Some(json!({ "lead_user_id": null })), Some("\"3\"")).await;
    assert!(cleared.body["lead_user_id"].is_null());
    let bad_lead = h.call(Method::PATCH, path, &ada, Some(json!({ "lead_user_id": Uuid::now_v7() })), Some("\"4\"")).await;
    assert_eq!(bad_lead.status, StatusCode::UNPROCESSABLE_ENTITY);

    // Deleting needs an organization admin and an archived project.
    assert_eq!(h.call(Method::DELETE, path, &ada, None, None).await.status, StatusCode::FORBIDDEN);
    assert_eq!(h.call(Method::DELETE, path, &org_admin, None, None).await.status, StatusCode::CONFLICT);
    assert_eq!(h.call(Method::POST, &format!("{path}/archive"), &ada, None, None).await.body["status"], "archived");
    assert!(keys(&h.call(Method::GET, "/api/v1/projects", &ada, None, None).await).is_empty());
    assert_eq!(keys(&h.call(Method::GET, "/api/v1/projects?status=archived", &ada, None, None).await), ["PAY"]);
    assert_eq!(h.call(Method::POST, &format!("{path}/restore"), &ada, None, None).await.body["status"], "active");
    h.call(Method::POST, &format!("{path}/archive"), &ada, None, None).await;
    assert_eq!(h.call(Method::DELETE, path, &org_admin, None, None).await.status, StatusCode::NO_CONTENT);
    assert_eq!(h.call(Method::GET, path, &org_admin, None, None).await.status, StatusCode::NOT_FOUND);
    assert_eq!(h.create(&ada, "PAY", "open").await.status, StatusCode::CONFLICT, "a deleted project's key stays reserved");
    h.db.cleanup().await;
}

#[tokio::test]
async fn components_and_versions() {
    let h = harness().await;
    let (_, ada) = h.user("member");
    let (_, bob) = h.user("member");
    h.create(&ada, "PAY", "open").await;
    let base = "/api/v1/projects/PAY";

    let api = h.call(Method::POST, &format!("{base}/components"), &ada, Some(json!({ "name": "API" })), None).await;
    assert_eq!(api.status, StatusCode::CREATED);
    assert_eq!(
        h.call(Method::POST, &format!("{base}/components"), &ada, Some(json!({ "name": "api" })), None).await.status,
        StatusCode::CONFLICT
    );
    assert_eq!(
        h.call(Method::POST, &format!("{base}/components"), &bob, Some(json!({ "name": "Web" })), None).await.status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        h.call(Method::GET, &format!("{base}/components"), &bob, None, None).await.body["items"].as_array().unwrap().len(),
        1
    );

    let v1 = h
        .call(
            Method::POST,
            &format!("{base}/versions"),
            &ada,
            Some(json!({ "name": "1.0", "start_date": "2026-11-01", "release_date": "2026-12-01" })),
            None,
        )
        .await;
    assert_eq!(v1.status, StatusCode::CREATED, "{}", v1.body);
    assert_eq!((v1.body["release_date"].as_str(), v1.body["status"].as_str()), (Some("2026-12-01"), Some("unreleased")));
    let backwards = h
        .call(
            Method::POST,
            &format!("{base}/versions"),
            &ada,
            Some(json!({ "name": "2.0", "start_date": "2026-12-01", "release_date": "2026-11-01" })),
            None,
        )
        .await;
    assert_eq!(backwards.status, StatusCode::UNPROCESSABLE_ENTITY);

    let id = v1.body["id"].as_str().unwrap();
    let released = h.call(Method::POST, &format!("{base}/versions/{id}/release"), &ada, None, None).await;
    assert_eq!(released.body["status"], "released");
    assert!(released.body["released_at"].is_string());
    assert_eq!(
        h.call(Method::POST, &format!("{base}/versions/{id}/unrelease"), &ada, None, None).await.body["status"],
        "unreleased"
    );
    assert_eq!(
        h.call(Method::POST, &format!("{base}/versions/{}/release", Uuid::now_v7()), &ada, None, None).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(h.call(Method::DELETE, &format!("{base}/versions/{id}"), &ada, None, None).await.status, StatusCode::NO_CONTENT);
    assert_eq!(
        h.call(Method::DELETE, &format!("{base}/components/{}", api.body["id"].as_str().unwrap()), &ada, None, None).await.status,
        StatusCode::NO_CONTENT
    );
    h.db.cleanup().await;
}
