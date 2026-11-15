//! Saved filters against a real PostgreSQL (`make infra`). No search runs here, so the index
//! and the other services are never reached.

use std::{collections::HashMap, sync::Arc};

use async_trait::async_trait;
use auth_middleware::{PolicyClient, PolicySnapshot, PolicySource, RedisHandle};
use axum::{
    body::Body,
    http::{Method, Request},
    Router,
};
use error_handling::ApiResult;
use search_service::{http, index::Index, people::People, AppState, MIGRATOR};
use serde_json::{json, Value};
use tenant_context::DbRouter;
use test_utilities::{TestAuth, TestDb};
use tower::ServiceExt;
use uuid::Uuid;

struct NoRoles;

#[async_trait]
impl PolicySource for NoRoles {
    async fn fetch(&self, _tenant_id: Uuid, _user_id: Uuid) -> ApiResult<PolicySnapshot> {
        Ok(PolicySnapshot { version: 0, roles: HashMap::new(), default_roles: HashMap::new() })
    }
}

struct NoPeople;

#[async_trait]
impl People for NoPeople {
    async fn find(&self, _tenant_id: Uuid, _terms: &[String]) -> ApiResult<HashMap<String, Vec<Uuid>>> {
        Ok(HashMap::new())
    }
}

struct Harness {
    db: TestDb,
    app: Router,
    auth: TestAuth,
}

async fn harness() -> Harness {
    let db = TestDb::create(&MIGRATOR).await.expect("test database");
    let auth = TestAuth::new().unwrap();
    let state = AppState {
        index: Index::new("http://127.0.0.1:1", "unused").unwrap(),
        db: Arc::new(DbRouter::single(db.app.clone())),
        verifier: Arc::clone(&auth.verifier),
        policy: Arc::new(PolicyClient::new(Arc::new(NoRoles), RedisHandle::default())),
        people: Arc::new(NoPeople),
        allowed_origins: Arc::new(vec![]),
    };
    Harness { app: http::router(state), db, auth }
}

impl Harness {
    fn token(&self, user: Uuid, tenant: Uuid, role: &str) -> String {
        self.auth.org_token(user, "x@example.com", tenant, Uuid::now_v7(), role).unwrap()
    }

    async fn call(&self, token: &str, method: Method, uri: &str, body: Option<Value>) -> (u16, Value) {
        let req = Request::builder()
            .method(method)
            .uri(uri)
            .header("authorization", format!("Bearer {token}"))
            .header("content-type", "application/json")
            .body(body.map_or_else(Body::empty, |b| Body::from(b.to_string())))
            .unwrap();
        let res = self.app.clone().oneshot(req).await.unwrap();
        let status = res.status().as_u16();
        let bytes = axum::body::to_bytes(res.into_body(), 1 << 20).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    async fn names(&self, token: &str) -> Vec<String> {
        let (status, body) = self.call(token, Method::GET, "/api/v1/search/filters", None).await;
        assert_eq!(status, 200, "{body}");
        body["items"].as_array().unwrap().iter().map(|f| f["name"].as_str().unwrap().to_owned()).collect()
    }
}

const FILTERS: &str = "/api/v1/search/filters";

#[tokio::test]
async fn filters_are_private_until_shared_and_edited_only_by_owner_or_admin() {
    let h = harness().await;
    let (tenant, other_tenant) = (Uuid::now_v7(), Uuid::now_v7());
    let (ada, bob, root) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let ada_t = h.token(ada, tenant, "member");
    let bob_t = h.token(bob, tenant, "member");
    let admin_t = h.token(root, tenant, "admin");
    let outsider_t = h.token(ada, other_tenant, "owner");

    // Invalid names and queries are refused, with the faulty part of the query pointed out.
    let (status, body) = h.call(&ada_t, Method::POST, FILTERS, Some(json!({ "name": " ", "query": "status = done" }))).await;
    assert_eq!(status, 422, "{body}");
    assert_eq!(body["errors"][0]["field"], "name");
    let (status, body) = h.call(&ada_t, Method::POST, FILTERS, Some(json!({ "name": "Broken", "query": "status = " }))).await;
    assert_eq!(status, 422, "{body}");
    assert_eq!(body["errors"][0]["field"], "query");
    assert!(body["errors"][0]["span"].is_array(), "{body}");
    // A client cannot set the owner or tenant.
    let (status, _) =
        h.call(&ada_t, Method::POST, FILTERS, Some(json!({ "name": "x", "query": "status = done", "owner_id": bob }))).await;
    assert_eq!(status, 422);

    let (status, mine) = h
        .call(
            &ada_t,
            Method::POST,
            FILTERS,
            Some(json!({ "name": "My open bugs", "query": "type = bug AND assignee = currentUser()" })),
        )
        .await;
    assert_eq!(status, 201, "{mine}");
    assert_eq!(mine["editable"], true);
    assert_eq!(mine["shared"], false);
    let mine_id = mine["id"].as_str().unwrap().to_owned();
    let (status, body) =
        h.call(&ada_t, Method::POST, FILTERS, Some(json!({ "name": "my OPEN bugs", "query": "type = bug" }))).await;
    assert_eq!(status, 422, "names are unique per person, ignoring case: {body}");
    assert_eq!(body["errors"][0]["code"], "taken");

    // Private: invisible to colleagues (404, not 403) and to other workspaces.
    assert!(h.names(&bob_t).await.is_empty());
    let (status, _) = h.call(&bob_t, Method::GET, &format!("{FILTERS}/{mine_id}"), None).await;
    assert_eq!(status, 404);
    let (status, _) = h.call(&bob_t, Method::DELETE, &format!("{FILTERS}/{mine_id}"), None).await;
    assert_eq!(status, 404);
    assert!(h.names(&outsider_t).await.is_empty());

    // Shared: everyone in the workspace can run it, only the owner or an admin can change it.
    let (status, shared) = h
        .call(
            &ada_t,
            Method::POST,
            FILTERS,
            Some(json!({ "name": "Release blockers", "query": "priority = highest", "shared": true })),
        )
        .await;
    assert_eq!(status, 201);
    let shared_id = shared["id"].as_str().unwrap().to_owned();
    let (status, bob_view) = h.call(&bob_t, Method::GET, &format!("{FILTERS}/{shared_id}"), None).await;
    assert_eq!(status, 200);
    assert_eq!(bob_view["editable"], false);
    let (status, _) = h.call(&bob_t, Method::PATCH, &format!("{FILTERS}/{shared_id}"), Some(json!({ "name": "Mine now" }))).await;
    assert_eq!(status, 403);
    assert!(h.names(&outsider_t).await.is_empty());

    // Own filters come first.
    let (_, own) = h.call(&bob_t, Method::POST, FILTERS, Some(json!({ "name": "Bob's", "query": "status = done" }))).await;
    assert_eq!(h.names(&bob_t).await, ["Bob's", "Release blockers"]);
    let (status, _) = h.call(&ada_t, Method::GET, &format!("{FILTERS}/{}", own["id"].as_str().unwrap()), None).await;
    assert_eq!(status, 404);

    // An admin can tidy up a shared filter but cannot decide who sees it.
    let (status, body) = h
        .call(&admin_t, Method::PATCH, &format!("{FILTERS}/{shared_id}"), Some(json!({ "query": "priority in (highest, high)" })))
        .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["query"], "priority in (highest, high)");
    let (status, _) = h.call(&admin_t, Method::PATCH, &format!("{FILTERS}/{shared_id}"), Some(json!({ "shared": false }))).await;
    assert_eq!(status, 403);
    let (status, _) = h.call(&admin_t, Method::PATCH, &format!("{FILTERS}/{mine_id}"), Some(json!({ "name": "x" }))).await;
    assert_eq!(status, 404, "a private filter stays private from admins");

    // The owner renames, changes the query (validated again) and stops sharing.
    let (status, _) = h.call(&ada_t, Method::PATCH, &format!("{FILTERS}/{mine_id}"), Some(json!({ "query": "((" }))).await;
    assert_eq!(status, 422);
    let (status, _) =
        h.call(&ada_t, Method::PATCH, &format!("{FILTERS}/{shared_id}"), Some(json!({ "name": "My open bugs" }))).await;
    assert_eq!(status, 422, "renaming onto another of one's own filters");
    let (status, body) = h
        .call(&ada_t, Method::PATCH, &format!("{FILTERS}/{shared_id}"), Some(json!({ "name": "Blockers", "shared": false })))
        .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(h.names(&bob_t).await, ["Bob's"]);

    let (status, _) = h.call(&ada_t, Method::DELETE, &format!("{FILTERS}/{mine_id}"), None).await;
    assert_eq!(status, 204);
    assert_eq!(h.names(&ada_t).await, ["Blockers"]);

    // Row-level security: the rows really carry the tenant, and another tenant's session sees none.
    let (count,): (i64,) = sqlx::query_as("SELECT count(*) FROM saved_filters").fetch_one(&h.db.app).await.unwrap();
    assert_eq!(count, 0, "without app.tenant_id set, the runtime role sees nothing");
    h.db.cleanup().await;
}

#[tokio::test]
async fn a_person_can_keep_at_most_a_hundred_filters() {
    let h = harness().await;
    let token = h.token(Uuid::now_v7(), Uuid::now_v7(), "member");
    for i in 0..100 {
        let (status, body) =
            h.call(&token, Method::POST, FILTERS, Some(json!({ "name": format!("f{i}"), "query": "status = done" }))).await;
        assert_eq!(status, 201, "{i}: {body}");
    }
    let (status, _) = h.call(&token, Method::POST, FILTERS, Some(json!({ "name": "one more", "query": "status = done" }))).await;
    assert_eq!(status, 409);
    h.db.cleanup().await;
}
