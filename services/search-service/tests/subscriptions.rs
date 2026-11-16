//! Filter subscriptions against a real PostgreSQL and OpenSearch (`make infra`): the API, and
//! the runner that runs a due filter as its subscriber and queues a digest in the outbox.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use auth_middleware::{OrgRole, PolicyClient, PolicySnapshot, PolicySource, ProjectRole, RedisHandle};
use axum::{
    body::Body,
    http::{Method, Request},
    Router,
};
use error_handling::ApiResult;
use search_service::{
    http,
    index::{Index, IssueDoc},
    people::People,
    subscriptions, AppState, MIGRATOR,
};
use serde_json::{json, Value};
use tenant_context::DbRouter;
use test_utilities::{TestAuth, TestDb};
use tower::ServiceExt;
use uuid::Uuid;

#[derive(Default)]
struct Roles(Mutex<HashMap<(Uuid, Uuid), HashMap<Uuid, ProjectRole>>>);

#[async_trait]
impl PolicySource for Roles {
    async fn fetch(&self, tenant_id: Uuid, user_id: Uuid) -> ApiResult<PolicySnapshot> {
        Ok(PolicySnapshot {
            version: 0,
            roles: self.0.lock().unwrap().get(&(tenant_id, user_id)).cloned().unwrap_or_default(),
            default_roles: HashMap::new(),
        })
    }
}

/// user-service: who is an active member, with which role.
#[derive(Default)]
struct Members(Mutex<HashMap<(Uuid, Uuid), OrgRole>>);

#[async_trait]
impl People for Members {
    async fn find(&self, _tenant_id: Uuid, _terms: &[String]) -> ApiResult<HashMap<String, Vec<Uuid>>> {
        Ok(HashMap::new())
    }

    async fn active_role(&self, tenant_id: Uuid, user_id: Uuid) -> ApiResult<Option<OrgRole>> {
        Ok(self.0.lock().unwrap().get(&(tenant_id, user_id)).copied())
    }
}

struct Harness {
    db: TestDb,
    state: AppState,
    app: Router,
    auth: TestAuth,
    roles: Arc<Roles>,
    members: Arc<Members>,
    index: Index,
}

async fn harness() -> Harness {
    let url = std::env::var("TEST_OPENSEARCH_URL").unwrap_or_else(|_| "http://127.0.0.1:59200".to_owned());
    let mut index = Index::new(&url, &format!("test-subscriptions-{}", Uuid::now_v7())).unwrap();
    index.refresh = true;
    index.ensure(1, 0).await.expect("OpenSearch reachable (make infra)");
    let db = TestDb::create(&MIGRATOR).await.expect("test database");
    let auth = TestAuth::new().unwrap();
    let roles = Arc::new(Roles::default());
    let members = Arc::new(Members::default());
    let state = AppState {
        index: index.clone(),
        db: Arc::new(DbRouter::single(db.app.clone())),
        verifier: Arc::clone(&auth.verifier),
        policy: Arc::new(PolicyClient::new(roles.clone(), RedisHandle::default())),
        people: members.clone(),
        allowed_origins: Arc::new(vec![]),
    };
    Harness { app: http::router(state.clone()), state, db, auth, roles, members, index }
}

impl Harness {
    fn member(&self, tenant: Uuid, user: Uuid, projects: &[Uuid]) -> String {
        self.members.0.lock().unwrap().insert((tenant, user), OrgRole::Member);
        self.roles.0.lock().unwrap().insert((tenant, user), projects.iter().map(|p| (*p, ProjectRole::Member)).collect());
        self.auth.org_token(user, "x@example.com", tenant, Uuid::now_v7(), "member").unwrap()
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

    async fn index(&self, tenant: Uuid, project: Uuid, key: &str, kind: &str) {
        let doc = IssueDoc {
            tenant_id: tenant,
            issue_id: Uuid::now_v7(),
            key: key.into(),
            project_id: project,
            project_key: key.split('-').next().unwrap().into(),
            kind: kind.into(),
            status: "todo".into(),
            status_category: "todo".into(),
            priority: "medium".into(),
            summary: format!("Summary of {key}"),
            description: String::new(),
            comments: String::new(),
            assignee_id: None,
            reporter_id: None,
            labels: vec![],
            created_at: "2026-11-01T10:00:00Z".into(),
            updated_at: "2026-11-02T10:00:00Z".into(),
            resolved_at: None,
            version: 1,
            indexed_at: "2026-11-02T10:00:00Z".into(),
            priority_rank: Some(3),
            key_number: key.rsplit('-').next().and_then(|n| n.parse().ok()),
            status_name: "To Do".into(),
            parent_key: None,
            components: vec![],
            fix_versions: vec![],
            due_date: None,
            story_points: None,
        };
        self.index.put(&doc).await.unwrap();
    }

    /// Everything due now, as if the hour had come.
    async fn make_due(&self) {
        sqlx::query("UPDATE subscription_schedule SET next_run_at = now() - interval '1 minute'")
            .execute(&self.db.owner)
            .await
            .unwrap();
    }

    async fn digests(&self) -> Vec<Value> {
        let rows: Vec<(Value,)> =
            sqlx::query_as("SELECT payload FROM outbox WHERE event_type = 'filter.digest' ORDER BY created_at, id")
                .fetch_all(&self.db.owner)
                .await
                .unwrap();
        rows.into_iter().map(|(p,)| p).collect()
    }

    async fn subscribers(&self) -> Vec<Uuid> {
        let rows: Vec<(Uuid,)> =
            sqlx::query_as("SELECT user_id FROM filter_subscriptions ORDER BY user_id").fetch_all(&self.db.owner).await.unwrap();
        rows.into_iter().map(|(u,)| u).collect()
    }
}

#[tokio::test]
async fn a_due_filter_runs_as_its_subscriber_and_ends_when_they_cannot_receive_it() {
    let h = harness().await;
    let (tenant, other) = (Uuid::now_v7(), Uuid::now_v7());
    let (pay, hr) = (Uuid::now_v7(), Uuid::now_v7());
    let (ada, bob, carol) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let ada_t = h.member(tenant, ada, &[pay]);
    let bob_t = h.member(tenant, bob, &[]);
    let carol_t = h.member(tenant, carol, &[pay, hr]);
    let outsider_t = h.auth.org_token(ada, "x@example.com", other, Uuid::now_v7(), "owner").unwrap();
    h.index(tenant, pay, "PAY-1", "bug").await;
    h.index(tenant, pay, "PAY-2", "story").await;
    h.index(tenant, hr, "HR-1", "bug").await;
    h.index(other, pay, "PAY-1", "bug").await;

    let (status, filter) = h
        .call(
            &ada_t,
            Method::POST,
            "/api/v1/search/filters",
            Some(json!({ "name": "Bugs", "query": "type = bug", "shared": true })),
        )
        .await;
    assert_eq!(status, 201, "{filter}");
    let sub = format!("/api/v1/search/filters/{}/subscription", filter["id"].as_str().unwrap());

    // Subscribing: options are checked, one subscription per person and filter.
    let (status, _) = h.call(&ada_t, Method::GET, &sub, None).await;
    assert_eq!(status, 404, "not subscribed yet");
    let (status, body) = h.call(&ada_t, Method::PUT, &sub, Some(json!({ "frequency": "weekly", "hour": 8 }))).await;
    assert_eq!((status, body["errors"][0]["field"].as_str()), (422, Some("weekday")));
    let (status, body) = h.call(&ada_t, Method::PUT, &sub, Some(json!({ "frequency": "daily", "hour": 8 }))).await;
    assert_eq!(status, 200, "{body}");
    assert!(body["next_run_at"].as_str().unwrap().ends_with("T08:00:00Z"), "{body}");
    let (status, body) = h.call(&ada_t, Method::PUT, &sub, Some(json!({ "frequency": "weekly", "hour": 9, "weekday": 1 }))).await;
    assert_eq!((status, body["weekday"].as_u64()), (200, Some(1)), "changed in place");
    for token in [&bob_t, &carol_t] {
        let (status, _) = h.call(token, Method::PUT, &sub, Some(json!({ "frequency": "daily", "hour": 7 }))).await;
        assert_eq!(status, 200);
    }
    let (status, _) = h.call(&outsider_t, Method::PUT, &sub, Some(json!({ "frequency": "daily", "hour": 7 }))).await;
    assert_eq!(status, 404, "another workspace cannot see the filter");
    let mut everyone = vec![ada, bob, carol];
    everyone.sort();
    assert_eq!(h.subscribers().await, everyone);

    // Nothing is due yet.
    assert_eq!(subscriptions::run_due(&h.state, &h.db.app).await.unwrap(), 0);

    // Carol leaves the workspace; then the hour comes.
    h.members.0.lock().unwrap().remove(&(tenant, carol));
    h.make_due().await;
    assert_eq!(subscriptions::run_due(&h.state, &h.db.app).await.unwrap(), 3);
    let digests = h.digests().await;
    assert_eq!(digests.len(), 2, "{digests:?}");
    let digest_for =
        |user: Uuid| digests.iter().find(|d| d["payload"]["recipient_id"] == user.to_string()).unwrap()["payload"].clone();
    let ada_digest = digest_for(ada);
    let keys: Vec<&str> = ada_digest["issues"].as_array().unwrap().iter().map(|i| i["key"].as_str().unwrap()).collect();
    assert_eq!(keys, ["PAY-1"], "only bugs in projects Ada can see, only in this workspace");
    assert_eq!((ada_digest["total"].as_u64(), ada_digest["filter_name"].as_str()), (Some(1), Some("Bugs")));
    assert_eq!(digest_for(bob)["total"], 0, "Bob sees no projects");
    let mut stay = vec![ada, bob];
    stay.sort();
    assert_eq!(h.subscribers().await, stay, "Carol's subscription ended");
    let (last_sent,): (Option<time::OffsetDateTime>,) =
        sqlx::query_as("SELECT last_sent_at FROM filter_subscriptions WHERE user_id = $1")
            .bind(ada)
            .fetch_one(&h.db.owner)
            .await
            .unwrap();
    assert!(last_sent.is_some());

    // Rescheduled into the future, so a second pass finds nothing.
    assert_eq!(subscriptions::run_due(&h.state, &h.db.app).await.unwrap(), 0);
    let (status, body) = h.call(&ada_t, Method::GET, &sub, None).await;
    assert_eq!(status, 200);
    assert!(body["last_sent_at"].is_string());

    // The owner stops sharing: Bob can no longer see the filter, so his subscription ends.
    let (status, _) = h.call(&ada_t, Method::PATCH, &sub.replace("/subscription", ""), Some(json!({ "shared": false }))).await;
    assert_eq!(status, 200);
    h.make_due().await;
    assert_eq!(subscriptions::run_due(&h.state, &h.db.app).await.unwrap(), 2);
    assert_eq!(h.subscribers().await, [ada]);
    assert_eq!(h.digests().await.len(), 3);

    // Unsubscribing; deleting the filter removes any remaining subscription.
    let (status, _) = h.call(&bob_t, Method::DELETE, &sub, None).await;
    assert_eq!(status, 404, "Bob has none any more");
    let (status, _) = h.call(&ada_t, Method::DELETE, &sub, None).await;
    assert_eq!(status, 204);
    assert!(h.subscribers().await.is_empty());
    let (status, _) = h.call(&ada_t, Method::PUT, &sub, Some(json!({ "frequency": "daily", "hour": 7 }))).await;
    assert_eq!(status, 200);
    let (status, _) = h.call(&ada_t, Method::DELETE, &sub.replace("/subscription", ""), None).await;
    assert_eq!(status, 204);
    let (left,): (i64,) = sqlx::query_as("SELECT count(*) FROM subscription_schedule").fetch_one(&h.db.owner).await.unwrap();
    assert_eq!(left, 0);
    assert!(h.subscribers().await.is_empty());
    h.db.cleanup().await;
}
