//! Indexing and search against the local OpenSearch (`make infra`), with issue-service and
//! the permission service replaced by in-memory fakes.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use api_contracts::issue::v1::Document;
use async_trait::async_trait;
use auth_middleware::{PolicyClient, PolicySnapshot, PolicySource, ProjectRole, RedisHandle};
use axum::{body::Body, http::Request, Router};
use error_handling::ApiResult;
use event_bus::{Event, Handler};
use event_contracts::topics;
use search_service::{
    http,
    index::Index,
    indexer::Indexer,
    source::{Fetched, Source},
    AppState,
};
use serde_json::{json, Value};
use test_utilities::TestAuth;
use tower::ServiceExt;
use uuid::Uuid;

fn opensearch_url() -> String {
    std::env::var("TEST_OPENSEARCH_URL").unwrap_or_else(|_| "http://127.0.0.1:59200".to_owned())
}

/// issue-service: documents by (tenant, id).
#[derive(Default)]
struct Issues(Mutex<HashMap<(Uuid, Uuid), Document>>);

#[async_trait]
impl Source for Issues {
    async fn documents(&self, tenant_id: Uuid, ids: &[Uuid]) -> ApiResult<Fetched> {
        let all = self.0.lock().unwrap();
        let mut fetched = Fetched::default();
        for id in ids {
            match all.get(&(tenant_id, *id)) {
                Some(d) => fetched.documents.push(d.clone()),
                None => fetched.missing.push(*id),
            }
        }
        Ok(fetched)
    }
    async fn issue_ids(&self, tenant_id: Uuid, after: Option<Uuid>, limit: u32) -> ApiResult<Vec<Uuid>> {
        let mut ids: Vec<Uuid> =
            self.0.lock().unwrap().keys().filter(|(t, id)| *t == tenant_id && Some(*id) > after).map(|(_, id)| *id).collect();
        ids.sort();
        ids.truncate(limit as usize);
        Ok(ids)
    }
}

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

struct Harness {
    app: Router,
    indexer: Indexer,
    issues: Arc<Issues>,
    roles: Arc<Roles>,
    auth: TestAuth,
    index: Index,
}

async fn harness() -> Harness {
    let mut index = Index::new(&opensearch_url(), &format!("test-issues-{}", Uuid::now_v7())).unwrap();
    index.refresh = true;
    index.ensure(1, 0).await.expect("OpenSearch reachable (make infra)");
    let issues = Arc::new(Issues::default());
    let roles = Arc::new(Roles::default());
    let auth = TestAuth::new().unwrap();
    let state = AppState {
        index: index.clone(),
        verifier: Arc::clone(&auth.verifier),
        policy: Arc::new(PolicyClient::new(roles.clone(), RedisHandle::default())),
        allowed_origins: Arc::new(vec![]),
    };
    Harness {
        app: http::router(state),
        indexer: Indexer { index: index.clone(), source: issues.clone() },
        issues,
        roles,
        auth,
        index,
    }
}

impl Harness {
    fn issue(&self, tenant: Uuid, project: Uuid, key: &str, summary: &str, description: &str, version: i64) -> Uuid {
        let id = Uuid::now_v7();
        self.put(tenant, id, project, key, summary, description, version);
        id
    }

    #[allow(clippy::too_many_arguments)]
    fn put(&self, tenant: Uuid, id: Uuid, project: Uuid, key: &str, summary: &str, description: &str, version: i64) {
        let doc = Document {
            id: id.to_string(),
            key: key.into(),
            project_id: project.to_string(),
            project_key: key.split('-').next().unwrap().into(),
            r#type: "story".into(),
            status: "todo".into(),
            status_category: "todo".into(),
            priority: "medium".into(),
            summary: summary.into(),
            description_text: description.into(),
            comments_text: String::new(),
            assignee_id: String::new(),
            reporter_id: Uuid::now_v7().to_string(),
            labels: vec![],
            created_at: "2026-11-01T10:00:00Z".into(),
            updated_at: "2026-11-02T10:00:00Z".into(),
            version,
            resolved_at: String::new(),
        };
        self.issues.0.lock().unwrap().insert((tenant, id), doc);
    }

    async fn event(&self, tenant: Uuid, topic: &str, event_type: &str, payload: Value) {
        let raw = json!({
            "event_id": Uuid::now_v7(), "event_type": event_type, "schema_version": 1, "tenant_id": tenant,
            "aggregate_type": "issue", "aggregate_id": Uuid::now_v7(), "aggregate_version": 1,
            "occurred_at": "2026-11-13T09:00:00Z", "correlation_id": null, "causation_id": null,
            "actor": {"type": "system", "id": "test"}, "payload": payload
        });
        self.indexer.handle(&Event::parse(topic, &serde_json::to_vec(&raw).unwrap()).unwrap()).await.unwrap();
    }

    async fn changed(&self, tenant: Uuid, issue: Uuid) {
        self.event(
            tenant,
            topics::ISSUE,
            "issue.updated",
            json!({ "issue_id": issue, "key": "X", "project_id": Uuid::nil(), "changed_fields": [] }),
        )
        .await;
    }

    fn token(&self, tenant: Uuid, user: Uuid, role: &str) -> String {
        self.auth.org_token(user, "x@example.com", tenant, Uuid::now_v7(), role).unwrap()
    }

    fn viewer(&self, tenant: Uuid, projects: &[Uuid]) -> String {
        let user = Uuid::now_v7();
        self.roles.0.lock().unwrap().insert((tenant, user), projects.iter().map(|p| (*p, ProjectRole::Viewer)).collect());
        self.token(tenant, user, "member")
    }

    async fn search(&self, token: &str, query: &str) -> (u16, Value) {
        let req = Request::builder()
            .uri(format!("/api/v1/search/issues?{query}"))
            .header("authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let res = self.app.clone().oneshot(req).await.unwrap();
        let status = res.status().as_u16();
        let bytes = axum::body::to_bytes(res.into_body(), 1 << 20).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    async fn keys(&self, token: &str, query: &str) -> Vec<String> {
        let (status, body) = self.search(token, query).await;
        assert_eq!(status, 200, "{body}");
        body["items"].as_array().unwrap().iter().map(|i| i["key"].as_str().unwrap().to_owned()).collect()
    }

    async fn drop_index(&self) {
        reqwest::Client::new().delete(format!("{}/{}", opensearch_url(), self.index.name())).send().await.unwrap();
    }
}

#[tokio::test]
async fn search_finds_only_the_callers_tenant_and_visible_projects() {
    let h = harness().await;
    let (t, other_tenant) = (Uuid::now_v7(), Uuid::now_v7());
    let (pay, hr) = (Uuid::now_v7(), Uuid::now_v7());
    let refunds = h.issue(t, pay, "PAY-1", "Refunds fail for gift cards", "Customers see an error when refunding.", 1);
    let checkout = h.issue(t, pay, "PAY-2", "Checkout button misaligned", "The refund link overlaps it.", 1);
    let salary = h.issue(t, hr, "HR-1", "Salary refunds for contractors", "", 1);
    let foreign = h.issue(other_tenant, pay, "PAY-1", "Refunds fail everywhere", "", 1);
    for (tenant, id) in [(t, refunds), (t, checkout), (t, salary), (other_tenant, foreign)] {
        h.changed(tenant, id).await;
    }

    let pay_only = h.viewer(t, &[pay]);
    let keys = h.keys(&pay_only, "q=refunds").await;
    assert_eq!(keys, ["PAY-1", "PAY-2"], "summary matches rank first; HR and the other tenant are invisible");

    let owner = h.token(t, Uuid::now_v7(), "owner");
    let mut all = h.keys(&owner, "q=refund").await;
    all.sort();
    assert_eq!(all, ["HR-1", "PAY-1", "PAY-2"]);
    assert_eq!(h.keys(&owner, &format!("q=refund&project_id={hr}")).await, ["HR-1"]);
    assert!(
        h.keys(&pay_only, &format!("q=refund&project_id={hr}")).await.is_empty(),
        "asking for a hidden project returns nothing"
    );
    assert!(h.keys(&h.viewer(t, &[]), "q=refund").await.is_empty());

    // Keys, typo tolerance, prefixes, and highlights that are data, not markup.
    assert_eq!(h.keys(&owner, "q=pay-2").await.first().map(String::as_str), Some("PAY-2"));
    assert!(h.keys(&owner, "q=refnds").await.contains(&"PAY-1".to_owned()));
    assert!(h.keys(&owner, "q=checko").await.contains(&"PAY-2".to_owned()));
    let (_, body) = h.search(&pay_only, "q=refund").await;
    let first = &body["items"][0];
    assert!(first["summary_parts"].as_array().unwrap().iter().any(|p| p["matched"] == true && p["text"] == "Refunds"), "{first}");
    assert_eq!(body["items"][1]["snippet"]["field"], "description");

    // Paging by cursor.
    let (_, page) = h.search(&owner, "q=refund&limit=2").await;
    let cursor = page["next_cursor"].as_str().unwrap().to_owned();
    let (_, rest) = h.search(&owner, &format!("q=refund&limit=2&cursor={cursor}")).await;
    assert_eq!((page["items"].as_array().unwrap().len(), rest["items"].as_array().unwrap().len()), (2, 1));
    assert_eq!(h.search(&owner, "q=x&cursor=garbage").await.0, 422);
    assert_eq!(h.search(&owner, &format!("q={}", "a".repeat(201))).await.0, 422);
    h.drop_index().await;
}

#[tokio::test]
async fn the_index_follows_changes_deletions_and_reindexes() {
    let h = harness().await;
    let t = Uuid::now_v7();
    let project = Uuid::now_v7();
    let owner = h.token(t, Uuid::now_v7(), "owner");
    let a = h.issue(t, project, "OPS-1", "Rotate certificates", "", 2);
    h.changed(t, a).await;
    assert_eq!(h.keys(&owner, "q=certificates").await, ["OPS-1"]);

    // An edit is picked up; an older version arriving late does not win.
    h.put(t, a, project, "OPS-1", "Rotate signing keys", "", 3);
    h.changed(t, a).await;
    let mut stale = search_service::index::IssueDoc::from_document(t, h.issues.0.lock().unwrap()[&(t, a)].clone()).unwrap();
    stale.summary = "Rotate certificates".into();
    stale.version = 2;
    assert_eq!(h.index.put(&stale).await.unwrap(), search_service::index::Written::Stale);
    assert!(h.keys(&owner, "q=certificates").await.is_empty());
    assert_eq!(h.keys(&owner, "q=signing").await, ["OPS-1"]);

    // Deleted issue (event, or gone at the source), deleted project.
    h.issues.0.lock().unwrap().remove(&(t, a));
    h.event(t, topics::ISSUE, "issue.deleted", json!({ "issue_id": a, "key": "OPS-1", "project_id": project })).await;
    assert!(h.keys(&owner, "q=signing").await.is_empty());
    let b = h.issue(t, project, "OPS-2", "Upgrade database", "", 1);
    h.changed(t, b).await;
    h.issues.0.lock().unwrap().remove(&(t, b));
    h.changed(t, b).await;
    assert!(h.keys(&owner, "q=database").await.is_empty());
    let c = h.issue(t, project, "OPS-3", "Archive logs", "", 1);
    h.changed(t, c).await;
    h.event(t, topics::PROJECT, "project.deleted", json!({ "project_id": project, "key": "OPS" })).await;
    assert!(h.keys(&owner, "q=logs").await.is_empty());

    // A full reindex restores everything and drops what no longer exists.
    let other = Uuid::now_v7();
    for n in 0..5 {
        h.issue(t, other, &format!("WEB-{n}"), &format!("Landing page {n}"), "", 1);
    }
    h.issues.0.lock().unwrap().remove(&(t, c));
    assert_eq!(h.indexer.reindex(t).await.unwrap(), 5);
    assert_eq!(h.keys(&owner, "q=landing&limit=50").await.len(), 5);
    assert!(h.keys(&owner, "").await.iter().all(|k| k.starts_with("WEB-")));
    h.drop_index().await;
}
