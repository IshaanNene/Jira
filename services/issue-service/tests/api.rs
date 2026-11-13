use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use auth_middleware::{PolicyClient, PolicySnapshot, PolicySource, ProjectRole, RedisHandle};
use axum::{
    body::Body,
    http::{header, Method, Request, StatusCode},
    Router,
};
use error_handling::{ApiError, ApiResult};
use issue_service::{
    directory::{Directory, ProjectInfo},
    http,
    workflows::{Evaluation, IssueFacts, PostFunction, Scope, StatusInfo, TransitionOption, WorkflowInfo, Workflows},
    AppState, MIGRATOR,
};
use serde_json::{json, Value};
use tenant_context::DbRouter;
use test_utilities::{TestAuth, TestDb};
use tower::ServiceExt;
use uuid::Uuid;

/// Stand-in for the project, user and permission services.
#[derive(Default)]
struct World {
    projects: Mutex<HashMap<(Uuid, String), ProjectInfo>>,
    org_members: Mutex<HashSet<Uuid>>,
    roles: Mutex<HashMap<(Uuid, Uuid), HashMap<Uuid, ProjectRole>>>, // (tenant, user) -> project -> role
    /// Workflow rules for the fake engine: (from, to) -> message when the move is refused.
    denials: Mutex<HashMap<(String, String), String>>,
    /// Post-functions run when an issue enters a status.
    effects: Mutex<HashMap<String, Vec<PostFunction>>>,
    /// The facts most recently reported to the engine.
    last_facts: Mutex<Option<IssueFacts>>,
}

fn fake_statuses() -> Vec<StatusInfo> {
    [
        ("todo", "To Do", "todo"),
        ("in_progress", "In Progress", "in_progress"),
        ("in_review", "In Review", "in_progress"),
        ("done", "Done", "done"),
    ]
    .into_iter()
    .map(|(key, name, category)| StatusInfo { key: key.into(), name: name.into(), category: category.into() })
    .collect()
}

/// Stand-in for workflow-service: four statuses, free movement unless a rule is configured.
#[async_trait]
impl Workflows for World {
    async fn info(&self, _scope: Scope<'_>) -> ApiResult<WorkflowInfo> {
        Ok(WorkflowInfo { statuses: fake_statuses(), initial: "todo".into() })
    }
    async fn transitions(&self, _scope: Scope<'_>, from: &str, facts: &IssueFacts) -> ApiResult<Vec<TransitionOption>> {
        *self.last_facts.lock().unwrap() = Some(facts.clone());
        let denials = self.denials.lock().unwrap();
        Ok(fake_statuses()
            .into_iter()
            .filter(|s| s.key != from && !denials.contains_key(&(from.to_owned(), s.key.clone())))
            .map(|s| TransitionOption { id: format!("to_{}", s.key), name: format!("Move to {}", s.name), to: s })
            .collect())
    }
    async fn evaluate(&self, _scope: Scope<'_>, from: &str, to: &str, facts: &IssueFacts) -> ApiResult<Evaluation> {
        *self.last_facts.lock().unwrap() = Some(facts.clone());
        let Some(target) = fake_statuses().into_iter().find(|s| s.key == to) else {
            return Ok(Evaluation::Denied { message: "That status is not part of this issue's workflow.".into() });
        };
        if let Some(message) = self.denials.lock().unwrap().get(&(from.to_owned(), to.to_owned())) {
            return Ok(Evaluation::Denied { message: message.clone() });
        }
        Ok(Evaluation::Allowed { to: target, post_functions: self.effects.lock().unwrap().get(to).cloned().unwrap_or_default() })
    }
}

#[async_trait]
impl Directory for World {
    async fn project(&self, tenant_id: Uuid, key: &str, _fresh: bool) -> ApiResult<ProjectInfo> {
        self.projects.lock().unwrap().get(&(tenant_id, key.to_owned())).cloned().ok_or(ApiError::NotFound("project"))
    }
    async fn is_org_member(&self, user_id: Uuid, _organization_id: Uuid) -> ApiResult<bool> {
        Ok(self.org_members.lock().unwrap().contains(&user_id))
    }
}

#[async_trait]
impl PolicySource for World {
    async fn fetch(&self, tenant_id: Uuid, user_id: Uuid) -> ApiResult<PolicySnapshot> {
        Ok(PolicySnapshot {
            version: 0,
            roles: self.roles.lock().unwrap().get(&(tenant_id, user_id)).cloned().unwrap_or_default(),
            default_roles: HashMap::new(),
        })
    }
}

struct Harness {
    db: TestDb,
    router: Arc<DbRouter>,
    app: Router,
    auth: TestAuth,
    world: Arc<World>,
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
    let world = Arc::new(World::default());
    let router = Arc::new(DbRouter::single(db.app.clone()));
    let state = AppState {
        db: Arc::clone(&router),
        verifier: Arc::clone(&auth.verifier),
        policy: Arc::new(PolicyClient::new(world.clone(), RedisHandle::default())),
        directory: world.clone(),
        workflows: world.clone(),
        blobs: Arc::new(object_store::memory::InMemory::new()),
        allowed_origins: Arc::new(vec![]),
    };
    Harness { db, router, app: http::router(state), auth, world, tenant: Uuid::now_v7(), org: Uuid::now_v7() }
}

impl Harness {
    fn project(&self, key: &str) -> Uuid {
        let id = Uuid::now_v7();
        self.world.projects.lock().unwrap().insert(
            (self.tenant, key.to_owned()),
            ProjectInfo { id, key: key.to_owned(), archived: false, component_ids: HashSet::new(), version_ids: HashSet::new() },
        );
        id
    }

    /// An organization member holding `roles` on the given projects. Returns (id, token).
    fn user(&self, roles: &[(Uuid, ProjectRole)]) -> (Uuid, String) {
        let id = Uuid::now_v7();
        self.world.org_members.lock().unwrap().insert(id);
        self.world.roles.lock().unwrap().insert((self.tenant, id), roles.iter().copied().collect());
        (id, self.auth.org_token(id, &format!("{id}@example.com"), self.tenant, self.org, "member").unwrap())
    }

    async fn call(&self, method: Method, path: &str, token: &str, body: Option<Value>, if_match: Option<i64>) -> Reply {
        let mut req = Request::builder()
            .method(method)
            .uri(path)
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::AUTHORIZATION, format!("Bearer {token}"));
        if let Some(v) = if_match {
            req = req.header(header::IF_MATCH, format!("\"{v}\""));
        }
        let response = self
            .app
            .clone()
            .oneshot(req.body(body.map_or_else(Body::empty, |b| Body::from(b.to_string()))).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let etag = response.headers().get(header::ETAG).map(|v| v.to_str().unwrap().to_owned());
        let bytes = axum::body::to_bytes(response.into_body(), 1 << 22).await.unwrap();
        Reply { status, body: serde_json::from_slice(&bytes).unwrap_or(Value::Null), etag }
    }

    /// Organization admin token (may manage custom fields; administers every project).
    fn org_admin(&self) -> String {
        let id = Uuid::now_v7();
        self.world.org_members.lock().unwrap().insert(id);
        self.auth.org_token(id, &format!("{id}@example.com"), self.tenant, self.org, "admin").unwrap()
    }

    /// Multipart upload of one file part.
    async fn upload(&self, token: &str, key: &str, filename: &str, bytes: &[u8]) -> Reply {
        let boundary = "----testboundary";
        let mut body = format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\nContent-Type: image/png\r\n\r\n").into_bytes();
        body.extend_from_slice(bytes);
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        let req = Request::builder()
            .method(Method::POST)
            .uri(format!("/api/v1/issues/{key}/attachments"))
            .header(header::CONTENT_TYPE, format!("multipart/form-data; boundary={boundary}"))
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::from(body))
            .unwrap();
        let response = self.app.clone().oneshot(req).await.unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 1 << 22).await.unwrap();
        Reply { status, body: serde_json::from_slice(&bytes).unwrap_or(Value::Null), etag: None }
    }

    async fn create(&self, token: &str, body: Value) -> Reply {
        self.call(Method::POST, "/api/v1/issues", token, Some(body), None).await
    }

    async fn task(&self, token: &str, project: &str, summary: &str) -> String {
        let reply = self.create(token, json!({ "project_key": project, "type": "task", "summary": summary })).await;
        assert_eq!(reply.status, StatusCode::CREATED, "{}", reply.body);
        reply.body["key"].as_str().unwrap().to_owned()
    }
}

fn text_doc(text: &str) -> Value {
    json!({ "type": "doc", "content": [{ "type": "paragraph", "content": [{ "type": "text", "text": text }] }] })
}

fn keys(reply: &Reply) -> Vec<String> {
    reply.body["items"].as_array().unwrap().iter().map(|i| i["key"].as_str().unwrap().to_owned()).collect()
}

#[tokio::test]
async fn keys_are_sequential_per_project_and_safe_under_concurrency() {
    let h = harness().await;
    let (pay, ops) = (h.project("PAY"), h.project("OPS"));
    let (_, token) = h.user(&[(pay, ProjectRole::Member), (ops, ProjectRole::Member)]);

    assert_eq!(h.task(&token, "PAY", "first").await, "PAY-1");
    assert_eq!(h.task(&token, "pay", "second").await, "PAY-2");
    assert_eq!(h.task(&token, "OPS", "other project").await, "OPS-1");

    // A failed creation does not burn a number.
    let failed = h.create(&token, json!({ "project_key": "PAY", "type": "subtask", "summary": "orphan" })).await;
    assert_eq!(failed.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(h.task(&token, "PAY", "third").await, "PAY-3");

    let mut handles = Vec::new();
    for n in 0..25 {
        let (app, token) = (h.app.clone(), token.clone());
        handles.push(tokio::spawn(async move {
            let req = Request::builder()
                .method(Method::POST)
                .uri("/api/v1/issues")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(
                    json!({ "project_key": "PAY", "type": "task", "summary": format!("concurrent {n}") }).to_string(),
                ))
                .unwrap();
            let response = app.oneshot(req).await.unwrap();
            assert_eq!(response.status(), StatusCode::CREATED);
            let bytes = axum::body::to_bytes(response.into_body(), 1 << 20).await.unwrap();
            serde_json::from_slice::<Value>(&bytes).unwrap()["key"].as_str().unwrap().to_owned()
        }));
    }
    let mut numbers = Vec::new();
    for handle in handles {
        numbers.push(handle.await.unwrap().trim_start_matches("PAY-").parse::<i64>().unwrap());
    }
    numbers.sort_unstable();
    assert_eq!(numbers, (4..=28).collect::<Vec<i64>>(), "no duplicates and no gaps");
    h.db.cleanup().await;
}

#[tokio::test]
async fn creation_validates_input_and_hierarchy() {
    let h = harness().await;
    let (pay, ops) = (h.project("PAY"), h.project("OPS"));
    let (me, token) = h.user(&[(pay, ProjectRole::Member), (ops, ProjectRole::Member)]);
    let stranger = Uuid::now_v7();

    let full = h
        .create(
            &token,
            json!({ "project_key": "PAY", "type": "story", "summary": "  Checkout  ", "priority": "high", "assignee_id": me,
                    "labels": ["Backend", "backend"], "story_points": 5, "start_date": "2026-11-01", "due_date": "2026-11-30",
                    "description": text_doc("Details") }),
        )
        .await;
    assert_eq!(full.status, StatusCode::CREATED, "{}", full.body);
    assert_eq!(full.body["summary"], "Checkout");
    assert_eq!(full.body["labels"], json!(["backend"]));
    assert_eq!((full.body["status"].as_str(), full.body["status_category"].as_str()), (Some("todo"), Some("todo")));
    assert_eq!(full.body["reporter_id"], me.to_string());
    assert_eq!(full.etag.as_deref(), Some("\"1\""));

    let epic = h.create(&token, json!({ "project_key": "PAY", "type": "epic", "summary": "Payments" })).await.body["key"]
        .as_str()
        .unwrap()
        .to_owned();
    let other_project_issue = h.task(&token, "OPS", "elsewhere").await;
    let story_in_epic =
        h.create(&token, json!({ "project_key": "PAY", "type": "story", "summary": "In epic", "parent_key": epic })).await;
    assert_eq!(story_in_epic.body["parent"]["key"], epic);
    let subtask =
        h.create(&token, json!({ "project_key": "PAY", "type": "subtask", "summary": "Sub", "parent_key": "PAY-1" })).await;
    assert_eq!(subtask.status, StatusCode::CREATED, "{}", subtask.body);

    let script_link = json!({ "type": "doc", "content": [{ "type": "paragraph", "content": [
        { "type": "text", "text": "x", "marks": [{ "type": "link", "attrs": { "href": "javascript:alert(1)" } }] }] }] });
    for bad in [
        json!({ "project_key": "PAY", "type": "task", "summary": "   " }),
        json!({ "project_key": "PAY", "type": "task", "summary": "x".repeat(256) }),
        json!({ "project_key": "PAY", "type": "saga", "summary": "x" }),
        json!({ "project_key": "PAY", "type": "task", "summary": "x", "priority": "urgent" }),
        json!({ "project_key": "PAY", "type": "task", "summary": "x", "labels": ["has space"] }),
        json!({ "project_key": "PAY", "type": "task", "summary": "x", "story_points": -1 }),
        json!({ "project_key": "PAY", "type": "task", "summary": "x", "start_date": "2026-12-01", "due_date": "2026-11-01" }),
        json!({ "project_key": "PAY", "type": "task", "summary": "x", "assignee_id": stranger }),
        json!({ "project_key": "PAY", "type": "task", "summary": "x", "description": script_link }),
        json!({ "project_key": "PAY", "type": "epic", "summary": "x", "parent_key": epic }),
        json!({ "project_key": "PAY", "type": "task", "summary": "x", "parent_key": "PAY-1" }),
        json!({ "project_key": "PAY", "type": "subtask", "summary": "x", "parent_key": epic }),
        json!({ "project_key": "PAY", "type": "subtask", "summary": "x", "parent_key": other_project_issue }),
        json!({ "project_key": "PAY", "type": "subtask", "summary": "x", "parent_key": "PAY-9999" }),
    ] {
        let reply = h.create(&token, bad.clone()).await;
        assert_eq!(reply.status, StatusCode::UNPROCESSABLE_ENTITY, "{bad} -> {}", reply.body);
    }
    let smuggled = h
        .create(
            &token,
            json!({ "project_key": "PAY", "type": "task", "summary": "x", "reporter_id": stranger, "tenant_id": Uuid::now_v7() }),
        )
        .await;
    assert!(smuggled.status.is_client_error(), "unknown fields are refused");
    assert_eq!(
        h.create(&token, json!({ "project_key": "NOPE", "type": "task", "summary": "x" })).await.status,
        StatusCode::NOT_FOUND
    );
    h.db.cleanup().await;
}

#[tokio::test]
async fn project_roles_gate_every_operation() {
    let h = harness().await;
    let (pay, secret) = (h.project("PAY"), h.project("SEC"));
    let (_, admin) = h.user(&[(pay, ProjectRole::Administrator), (secret, ProjectRole::Administrator)]);
    let (_, member) = h.user(&[(pay, ProjectRole::Member)]);
    let (_, viewer) = h.user(&[(pay, ProjectRole::Viewer)]);
    let (_, outsider) = h.user(&[]);
    let key = h.task(&member, "PAY", "work").await;
    let hidden = h.task(&admin, "SEC", "classified").await;
    let path = format!("/api/v1/issues/{key}");

    // Viewers read but do not write.
    assert_eq!(h.call(Method::GET, &path, &viewer, None, None).await.body["my_role"], "viewer");
    assert_eq!(
        h.create(&viewer, json!({ "project_key": "PAY", "type": "task", "summary": "x" })).await.status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        h.call(Method::PATCH, &path, &viewer, Some(json!({ "summary": "x" })), Some(1)).await.status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        h.call(Method::PATCH, &path, &viewer, Some(json!({ "status": "done" })), Some(1)).await.status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        h.call(Method::POST, &format!("{path}/comments"), &viewer, Some(json!({ "body": text_doc("hi") })), None).await.status,
        StatusCode::FORBIDDEN
    );

    // No role at all: the project and its issues do not exist as far as the caller can tell.
    for (method, p) in [
        (Method::GET, path.clone()),
        (Method::GET, format!("{path}/comments")),
        (Method::GET, format!("{path}/history")),
        (Method::GET, format!("{path}/links")),
        (Method::PUT, format!("{path}/watch")),
        (Method::DELETE, path.clone()),
        (Method::GET, "/api/v1/issues?project=PAY".to_owned()),
    ] {
        assert_eq!(h.call(method.clone(), &p, &outsider, None, None).await.status, StatusCode::NOT_FOUND, "{method} {p}");
    }
    assert_eq!(
        h.call(Method::PATCH, &path, &outsider, Some(json!({ "summary": "x" })), Some(1)).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        h.create(&outsider, json!({ "project_key": "PAY", "type": "task", "summary": "x" })).await.status,
        StatusCode::NOT_FOUND
    );
    // A member of PAY probing an issue of a project they are not in gets the same answer as for a missing key.
    let probe = h.call(Method::GET, &format!("/api/v1/issues/{hidden}"), &member, None, None).await;
    let missing = h.call(Method::GET, "/api/v1/issues/SEC-999", &member, None, None).await;
    assert_eq!((probe.status, &probe.body["detail"]), (StatusCode::NOT_FOUND, &missing.body["detail"]));

    // Deleting is for project administrators, and not while child issues exist.
    assert_eq!(h.call(Method::DELETE, &path, &member, None, None).await.status, StatusCode::FORBIDDEN);
    h.create(&member, json!({ "project_key": "PAY", "type": "subtask", "summary": "child", "parent_key": key })).await;
    assert_eq!(h.call(Method::DELETE, &path, &admin, None, None).await.status, StatusCode::CONFLICT);
    assert_eq!(h.call(Method::DELETE, "/api/v1/issues/PAY-2", &admin, None, None).await.status, StatusCode::NO_CONTENT);
    assert_eq!(h.call(Method::DELETE, &path, &admin, None, None).await.status, StatusCode::NO_CONTENT);
    assert_eq!(h.call(Method::GET, &path, &admin, None, None).await.status, StatusCode::NOT_FOUND);
    assert_eq!(h.task(&member, "PAY", "after delete").await, "PAY-3", "numbers of deleted issues are never reused");

    // Archived projects are read-only.
    h.world.projects.lock().unwrap().get_mut(&(h.tenant, "PAY".to_owned())).unwrap().archived = true;
    assert_eq!(
        h.create(&member, json!({ "project_key": "PAY", "type": "task", "summary": "x" })).await.status,
        StatusCode::CONFLICT
    );
    assert_eq!(
        h.call(Method::PATCH, "/api/v1/issues/PAY-3", &member, Some(json!({ "summary": "x" })), Some(1)).await.status,
        StatusCode::CONFLICT
    );
    assert_eq!(h.call(Method::GET, "/api/v1/issues/PAY-3", &member, None, None).await.status, StatusCode::OK);
    h.db.cleanup().await;
}

#[tokio::test]
async fn updates_use_optimistic_concurrency_and_record_history() {
    let h = harness().await;
    let pay = h.project("PAY");
    let (ada, token) = h.user(&[(pay, ProjectRole::Member)]);
    let (bob, _) = h.user(&[(pay, ProjectRole::Member)]);
    let key = h.task(&token, "PAY", "Original").await;
    let path = format!("/api/v1/issues/{key}");

    assert_eq!(
        h.call(Method::PATCH, &path, &token, Some(json!({ "summary": "x" })), None).await.status,
        StatusCode::PRECONDITION_REQUIRED
    );

    let first = h
        .call(
            Method::PATCH,
            &path,
            &token,
            Some(json!({ "summary": "Renamed", "priority": "highest", "assignee_id": bob, "labels": ["api"], "story_points": 3, "due_date": "2026-12-24",
                         "description": text_doc("Now with details") })),
            Some(1),
        )
        .await;
    assert_eq!(first.status, StatusCode::OK, "{}", first.body);
    assert_eq!((first.body["version"].as_i64(), first.etag.as_deref()), (Some(2), Some("\"2\"")));
    assert_eq!(first.body["assignee_id"], bob.to_string());

    // Someone still looking at version 1 is refused and shown what the issue looks like now.
    let stale = h.call(Method::PATCH, &path, &token, Some(json!({ "summary": "Lost update" })), Some(1)).await;
    assert_eq!(stale.status, StatusCode::CONFLICT);
    assert_eq!(
        (stale.body["code"].as_str(), stale.body["current"]["summary"].as_str()),
        (Some("version_conflict"), Some("Renamed"))
    );

    // Sending the same values again changes nothing and does not bump the version.
    let noop = h.call(Method::PATCH, &path, &token, Some(json!({ "summary": "Renamed", "priority": "highest" })), Some(2)).await;
    assert_eq!(noop.body["version"], 2);

    // Done sets the resolution; reopening clears it. null clears nullable fields; absent leaves them.
    let done = h.call(Method::PATCH, &path, &token, Some(json!({ "status": "done" })), Some(2)).await;
    assert_eq!((done.body["status_category"].as_str(), done.body["resolution"].as_str()), (Some("done"), Some("done")));
    assert!(done.body["resolved_at"].is_string());
    assert_eq!(done.body["due_date"], "2026-12-24", "untouched fields are kept");
    let reopened = h
        .call(
            Method::PATCH,
            &path,
            &token,
            Some(json!({ "status": "in_progress", "assignee_id": null, "due_date": null, "story_points": null })),
            Some(3),
        )
        .await;
    assert_eq!(reopened.status, StatusCode::OK, "{}", reopened.body);
    assert!(
        reopened.body["resolution"].is_null() && reopened.body["assignee_id"].is_null() && reopened.body["due_date"].is_null()
    );
    assert!(reopened.body["story_points"].is_null());

    for bad in [
        json!({ "status": "blocked" }),
        json!({ "type": "subtask" }),
        json!({ "parent_key": key }),
        json!({ "summary": "" }),
        json!({ "version": 99 }),
    ] {
        let reply = h.call(Method::PATCH, &path, &token, Some(bad.clone()), Some(4)).await;
        assert!(reply.status.is_client_error(), "{bad} -> {}", reply.status);
    }

    let history = h.call(Method::GET, &format!("{path}/history"), &token, None, None).await;
    let entries = history.body["items"].as_array().unwrap();
    assert_eq!(entries.len(), 4, "created + three real updates");
    let fields = |entry: &Value| {
        entry["changes"].as_array().unwrap().iter().map(|c| c["field"].as_str().unwrap().to_owned()).collect::<Vec<_>>()
    };
    assert_eq!(fields(&entries[0]), ["story_points", "due_date", "status", "assignee", "resolution"]);
    assert_eq!(fields(&entries[1]), ["status", "resolution"]);
    // History records what people saw: status names, not keys.
    assert_eq!(entries[1]["changes"][0], json!({ "field": "status", "from": "To Do", "to": "Done" }));
    assert_eq!(entries[2]["actor_id"], ada.to_string());

    let events: Vec<(String,)> =
        sqlx::query_as("SELECT event_type FROM outbox ORDER BY created_at, event_type").fetch_all(&h.db.owner).await.unwrap();
    let events: Vec<&str> = events.iter().map(|(e,)| e.as_str()).collect();
    assert_eq!(
        events,
        [
            "issue.created",
            "issue.assigned",
            "issue.updated",
            "issue.transitioned",
            "issue.updated",
            "issue.assigned",
            "issue.transitioned",
            "issue.updated"
        ]
    );
    h.db.cleanup().await;
}

#[tokio::test]
async fn listing_filters_paginates_and_is_injection_safe() {
    let h = harness().await;
    let pay = h.project("PAY");
    let (me, token) = h.user(&[(pay, ProjectRole::Member)]);
    for i in 1..=5 {
        h.task(&token, "PAY", &format!("Task {i}")).await;
    }
    h.create(&token, json!({ "project_key": "PAY", "type": "bug", "summary": "100% broken_thing", "priority": "highest", "labels": ["urgent"], "assignee_id": me })).await;
    h.call(Method::PATCH, "/api/v1/issues/PAY-2", &token, Some(json!({ "status": "done" })), Some(1)).await;
    let list = |query: &'static str| {
        let (h, token) = (&h, token.clone());
        async move {
            let reply = h.call(Method::GET, &format!("/api/v1/issues?project=PAY{query}"), &token, None, None).await;
            assert_eq!(reply.status, StatusCode::OK, "{query}: {}", reply.body);
            reply
        }
    };

    assert_eq!(keys(&list("").await), ["PAY-6", "PAY-5", "PAY-4", "PAY-3", "PAY-2", "PAY-1"]);
    assert!(list("").await.body["items"][0].get("description").is_none(), "lists omit descriptions");
    assert_eq!(keys(&list("&type=bug").await), ["PAY-6"]);
    assert_eq!(keys(&list("&status=done,in_progress").await), ["PAY-2"]);
    assert_eq!(keys(&list("&status_category=done").await), ["PAY-2"]);
    assert_eq!(list("&status_category=todo,in_progress").await.body["items"].as_array().unwrap().len(), 5);
    assert!(
        keys(&list("&status=no_such_status").await).is_empty(),
        "status keys are workflow-defined; an unknown one just matches nothing"
    );
    assert_eq!(list("").await.body["items"][0]["status_name"], "To Do");
    assert_eq!(keys(&list("&priority=highest").await), ["PAY-6"]);
    assert_eq!(keys(&list("&assignee=me").await), ["PAY-6"]);
    assert_eq!(list("&assignee=unassigned").await.body["items"].as_array().unwrap().len(), 5);
    assert_eq!(keys(&list("&label=URGENT").await), ["PAY-6"]);
    assert_eq!(keys(&list("&q=task%203").await), ["PAY-3"]);
    assert_eq!(keys(&list("&q=pay-4").await), ["PAY-4"]);
    assert_eq!(keys(&list("&q=100%25").await), ["PAY-6"], "% is literal");
    assert_eq!(keys(&list("&q=broken_thing").await), ["PAY-6"]);
    assert!(keys(&list("&q=broken%5Fthin_").await).is_empty(), "_ is literal, not a wildcard");
    assert!(keys(&list("&q=%27%20OR%201%3D1%20--").await).is_empty());
    assert!(keys(&list("&label=%27%29%3B%20DROP%20TABLE%20issues%3B--").await).is_empty());
    let (count,): (i64,) = sqlx::query_as("SELECT count(*) FROM issues").fetch_one(&h.db.owner).await.unwrap();
    assert_eq!(count, 6);

    let page1 = list("&limit=4").await;
    assert_eq!(keys(&page1), ["PAY-6", "PAY-5", "PAY-4", "PAY-3"]);
    let cursor = page1.body["next_cursor"].as_str().unwrap().to_owned();
    let page2 = h.call(Method::GET, &format!("/api/v1/issues?project=PAY&limit=4&cursor={cursor}"), &token, None, None).await;
    assert_eq!(keys(&page2), ["PAY-2", "PAY-1"]);
    assert!(page2.body["next_cursor"].is_null());

    for bad in ["&status_category=nope", "&type=nope", "&assignee=nope", "&cursor=nope"] {
        assert_eq!(
            h.call(Method::GET, &format!("/api/v1/issues?project=PAY{bad}"), &token, None, None).await.status,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    h.db.cleanup().await;
}

#[tokio::test]
async fn comments_belong_to_their_authors() {
    let h = harness().await;
    let pay = h.project("PAY");
    let (_, ada) = h.user(&[(pay, ProjectRole::Member)]);
    let (bob_id, bob) = h.user(&[(pay, ProjectRole::Member)]);
    let (_, admin) = h.user(&[(pay, ProjectRole::Administrator)]);
    let key = h.task(&ada, "PAY", "Discuss").await;
    let base = format!("/api/v1/issues/{key}/comments");

    let mention = json!({ "type": "doc", "content": [{ "type": "paragraph", "content": [
        { "type": "text", "text": "ping " }, { "type": "mention", "attrs": { "id": bob_id, "label": "Bob" } }] }] });
    let created = h.call(Method::POST, &base, &ada, Some(json!({ "body": mention })), None).await;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.body);
    let id = created.body["id"].as_str().unwrap().to_owned();

    let empty = json!({ "type": "doc", "content": [{ "type": "paragraph" }] });
    assert_eq!(
        h.call(Method::POST, &base, &ada, Some(json!({ "body": empty })), None).await.status,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        h.call(Method::POST, &base, &ada, Some(json!({ "body": "<script>alert(1)</script>" })), None).await.status,
        StatusCode::UNPROCESSABLE_ENTITY
    );

    // Nobody but the author edits a comment — not even a project administrator.
    let edit = json!({ "body": text_doc("edited") });
    assert_eq!(
        h.call(Method::PATCH, &format!("{base}/{id}"), &bob, Some(edit.clone()), None).await.status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        h.call(Method::PATCH, &format!("{base}/{id}"), &admin, Some(edit.clone()), None).await.status,
        StatusCode::FORBIDDEN
    );
    let edited = h.call(Method::PATCH, &format!("{base}/{id}"), &ada, Some(edit), None).await;
    assert!(edited.body["edited_at"].is_string());

    // Members delete their own; administrators may remove anyone's.
    assert_eq!(h.call(Method::DELETE, &format!("{base}/{id}"), &bob, None, None).await.status, StatusCode::FORBIDDEN);
    let second = h.call(Method::POST, &base, &bob, Some(json!({ "body": text_doc("mine") })), None).await.body["id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(h.call(Method::GET, &base, &bob, None, None).await.body["items"].as_array().unwrap().len(), 2);
    assert_eq!(h.call(Method::DELETE, &format!("{base}/{second}"), &bob, None, None).await.status, StatusCode::NO_CONTENT);
    assert_eq!(h.call(Method::DELETE, &format!("{base}/{id}"), &admin, None, None).await.status, StatusCode::NO_CONTENT);
    assert!(h.call(Method::GET, &base, &ada, None, None).await.body["items"].as_array().unwrap().is_empty());
    // A comment id cannot be reached through another issue's path.
    let other = h.task(&ada, "PAY", "Other").await;
    let third = h.call(Method::POST, &base, &ada, Some(json!({ "body": text_doc("x") })), None).await.body["id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        h.call(Method::DELETE, &format!("/api/v1/issues/{other}/comments/{third}"), &ada, None, None).await.status,
        StatusCode::NOT_FOUND
    );

    let (mentioned,): (Value,) = sqlx::query_as("SELECT payload->'payload'->'mentioned_user_ids' FROM outbox WHERE event_type = 'comment.created' ORDER BY created_at LIMIT 1")
        .fetch_one(&h.db.owner)
        .await
        .unwrap();
    assert_eq!(mentioned, json!([bob_id]));
    h.db.cleanup().await;
}

#[tokio::test]
async fn links_respect_project_visibility() {
    let h = harness().await;
    let (pay, secret) = (h.project("PAY"), h.project("SEC"));
    let (_, both) = h.user(&[(pay, ProjectRole::Member), (secret, ProjectRole::Member)]);
    let (_, pay_only) = h.user(&[(pay, ProjectRole::Member)]);
    let a = h.task(&both, "PAY", "A").await;
    let b = h.task(&both, "PAY", "B").await;
    let hidden = h.task(&both, "SEC", "Hidden").await;
    let links = format!("/api/v1/issues/{a}/links");

    assert_eq!(
        h.call(Method::POST, &links, &both, Some(json!({ "link_type": "blocks", "target_key": b })), None).await.status,
        StatusCode::CREATED
    );
    assert_eq!(
        h.call(Method::POST, &links, &both, Some(json!({ "link_type": "blocks", "target_key": b })), None).await.status,
        StatusCode::CONFLICT
    );
    assert_eq!(
        h.call(Method::POST, &links, &both, Some(json!({ "link_type": "relates", "target_key": hidden })), None).await.status,
        StatusCode::CREATED
    );
    for bad in [
        json!({ "link_type": "blocks", "target_key": a }),
        json!({ "link_type": "loves", "target_key": b }),
        json!({ "link_type": "blocks", "target_key": "PAY-999" }),
    ] {
        assert_eq!(h.call(Method::POST, &links, &both, Some(bad), None).await.status, StatusCode::UNPROCESSABLE_ENTITY);
    }
    // Linking to an issue in an invisible project fails exactly like linking to a missing one.
    let probe =
        h.call(Method::POST, &links, &pay_only, Some(json!({ "link_type": "depends_on", "target_key": hidden })), None).await;
    let missing =
        h.call(Method::POST, &links, &pay_only, Some(json!({ "link_type": "depends_on", "target_key": "SEC-999" })), None).await;
    assert_eq!((probe.status, &probe.body), (StatusCode::UNPROCESSABLE_ENTITY, &missing.body));

    let outward = h.call(Method::GET, &links, &both, None, None).await;
    let labels: Vec<&str> = outward.body["items"].as_array().unwrap().iter().map(|l| l["label"].as_str().unwrap()).collect();
    assert_eq!(labels, ["blocks", "relates to"]);
    let inward = h.call(Method::GET, &format!("/api/v1/issues/{b}/links"), &both, None, None).await;
    assert_eq!(
        (inward.body["items"][0]["label"].as_str(), inward.body["items"][0]["issue"]["key"].as_str()),
        (Some("is blocked by"), Some(a.as_str()))
    );
    // The link into the secret project is simply absent for someone who cannot see it.
    let filtered = h.call(Method::GET, &links, &pay_only, None, None).await;
    assert_eq!(filtered.body["items"].as_array().unwrap().len(), 1);

    let id = outward.body["items"][0]["id"].as_str().unwrap();
    assert_eq!(h.call(Method::DELETE, &format!("{links}/{id}"), &both, None, None).await.status, StatusCode::NO_CONTENT);
    assert_eq!(h.call(Method::DELETE, &format!("{links}/{id}"), &both, None, None).await.status, StatusCode::NOT_FOUND);
    h.db.cleanup().await;
}

#[tokio::test]
async fn watching_and_tenant_isolation() {
    let h = harness().await;
    let pay = h.project("PAY");
    let (_, ada) = h.user(&[(pay, ProjectRole::Member)]);
    let (_, bob) = h.user(&[(pay, ProjectRole::Viewer)]);
    let key = h.task(&ada, "PAY", "Watch me").await;
    let path = format!("/api/v1/issues/{key}");

    let mine = h.call(Method::GET, &path, &ada, None, None).await;
    assert_eq!(
        (mine.body["watching"].as_bool(), mine.body["watcher_count"].as_i64()),
        (Some(true), Some(1)),
        "reporters watch automatically"
    );
    assert_eq!(h.call(Method::PUT, &format!("{path}/watch"), &bob, None, None).await.status, StatusCode::NO_CONTENT);
    h.call(Method::PUT, &format!("{path}/watch"), &bob, None, None).await;
    assert_eq!(h.call(Method::GET, &path, &bob, None, None).await.body["watcher_count"], 2);
    h.call(Method::DELETE, &format!("{path}/watch"), &bob, None, None).await;
    assert_eq!(h.call(Method::GET, &path, &bob, None, None).await.body["watching"], false);

    // Another tenant with a project of the same key and an administrator of it.
    let other_tenant = Uuid::now_v7();
    let other_project = Uuid::now_v7();
    h.world.projects.lock().unwrap().insert(
        (other_tenant, "PAY".into()),
        ProjectInfo {
            id: other_project,
            key: "PAY".into(),
            archived: false,
            component_ids: HashSet::new(),
            version_ids: HashSet::new(),
        },
    );
    let eve = Uuid::now_v7();
    h.world
        .roles
        .lock()
        .unwrap()
        .insert((other_tenant, eve), [(other_project, ProjectRole::Administrator), (pay, ProjectRole::Administrator)].into());
    let eve_token = h.auth.org_token(eve, "eve@example.com", other_tenant, Uuid::now_v7(), "owner").unwrap();
    assert_eq!(h.call(Method::GET, &path, &eve_token, None, None).await.status, StatusCode::NOT_FOUND);
    assert!(keys(&h.call(Method::GET, "/api/v1/issues?project=PAY", &eve_token, None, None).await).is_empty());
    assert_eq!(
        h.call(Method::PATCH, &path, &eve_token, Some(json!({ "summary": "pwned" })), Some(1)).await.status,
        StatusCode::NOT_FOUND
    );
    // Her tenant's numbering starts from 1, independent of the first tenant's.
    assert_eq!(h.task(&eve_token, "PAY", "theirs").await, "PAY-1");
    assert_eq!(h.call(Method::GET, &path, &ada, None, None).await.body["summary"], "Watch me");

    let (visible,): (i64,) = sqlx::query_as("SELECT count(*) FROM issues").fetch_one(&h.db.app).await.unwrap();
    assert_eq!(visible, 0, "no tenant set, no rows");
    let unprotected: Vec<(String,)> = sqlx::query_as(
        "SELECT c.relname::text FROM pg_class c JOIN pg_attribute a ON a.attrelid = c.oid AND a.attname = 'tenant_id' \
         WHERE c.relkind = 'r' AND c.relnamespace = 'public'::regnamespace AND c.relname <> 'outbox' \
           AND NOT (c.relrowsecurity AND c.relforcerowsecurity)",
    )
    .fetch_all(&h.db.owner)
    .await
    .unwrap();
    assert!(unprotected.is_empty(), "{unprotected:?}");
    h.db.cleanup().await;
}

#[tokio::test]
async fn components_and_versions_must_belong_to_the_project() {
    let h = harness().await;
    let pay = h.project("PAY");
    let (_, token) = h.user(&[(pay, ProjectRole::Member)]);
    let (api, web, v1, foreign) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    {
        let mut projects = h.world.projects.lock().unwrap();
        let info = projects.get_mut(&(h.tenant, "PAY".to_owned())).unwrap();
        info.component_ids = [api, web].into();
        info.version_ids = [v1].into();
    }

    let created = h
        .create(
            &token,
            json!({ "project_key": "PAY", "type": "task", "summary": "x", "component_ids": [api, api], "fix_version_ids": [v1] }),
        )
        .await;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.body);
    assert_eq!(created.body["component_ids"], json!([api]), "duplicates collapse");
    assert_eq!(created.body["fix_version_ids"], json!([v1]));

    let bad = h.create(&token, json!({ "project_key": "PAY", "type": "task", "summary": "x", "component_ids": [foreign] })).await;
    assert_eq!(bad.status, StatusCode::UNPROCESSABLE_ENTITY);
    // A version id is not a component id.
    let swapped = h.create(&token, json!({ "project_key": "PAY", "type": "task", "summary": "x", "component_ids": [v1] })).await;
    assert_eq!(swapped.status, StatusCode::UNPROCESSABLE_ENTITY);

    let updated = h
        .call(
            Method::PATCH,
            "/api/v1/issues/PAY-1",
            &token,
            Some(json!({ "component_ids": [web, api], "fix_version_ids": [] })),
            Some(1),
        )
        .await;
    assert_eq!(updated.status, StatusCode::OK, "{}", updated.body);
    assert_eq!((updated.body["component_ids"].clone(), updated.body["fix_version_ids"].clone()), (json!([web, api]), json!([])));
    let rejected =
        h.call(Method::PATCH, "/api/v1/issues/PAY-1", &token, Some(json!({ "fix_version_ids": [foreign] })), Some(2)).await;
    assert_eq!(rejected.status, StatusCode::UNPROCESSABLE_ENTITY);
    h.db.cleanup().await;
}

#[tokio::test]
async fn custom_fields_are_defined_by_admins_and_validated_on_issues() {
    let h = harness().await;
    let (pay, ops) = (h.project("PAY"), h.project("OPS"));
    let (me, member) = h.user(&[(pay, ProjectRole::Member), (ops, ProjectRole::Member)]);
    let admin = h.org_admin();
    let fields = "/api/v1/custom-fields";

    // Only organization admins define fields; everyone can read the definitions.
    let denied = h
        .call(Method::POST, fields, &member, Some(json!({ "name": "Severity", "field_type": "select", "options": ["S1"] })), None)
        .await;
    assert_eq!(denied.status, StatusCode::FORBIDDEN);
    let severity = h
        .call(Method::POST, fields, &admin, Some(json!({ "name": "Severity", "field_type": "select", "options": ["S1", "S2", "S3"], "required": true, "project_keys": ["pay"], "issue_types": ["bug"] })), None)
        .await;
    assert_eq!(severity.status, StatusCode::CREATED, "{}", severity.body);
    assert_eq!(severity.body["project_keys"], json!(["PAY"]));
    let sid = severity.body["id"].as_str().unwrap().to_owned();
    let mk = |name: &'static str, field_type: &'static str| {
        let (h, admin) = (&h, admin.clone());
        async move {
            h.call(Method::POST, fields, &admin, Some(json!({ "name": name, "field_type": field_type })), None).await.body["id"]
                .as_str()
                .unwrap()
                .to_owned()
        }
    };
    let (owner, estimate, docs) = (mk("Owner", "user").await, mk("Estimate", "number").await, mk("Docs", "url").await);
    assert_eq!(
        h.call(Method::POST, fields, &admin, Some(json!({ "name": "severity", "field_type": "text" })), None).await.status,
        StatusCode::CONFLICT
    );
    for bad in [
        json!({ "name": "X", "field_type": "blob" }),
        json!({ "name": "X", "field_type": "select" }),
        json!({ "name": "", "field_type": "text" }),
    ] {
        assert_eq!(h.call(Method::POST, fields, &admin, Some(bad), None).await.status, StatusCode::UNPROCESSABLE_ENTITY);
    }
    assert_eq!(h.call(Method::GET, fields, &member, None, None).await.body["items"].as_array().unwrap().len(), 4);

    // Required applies only where the field applies: bugs in PAY.
    let missing = h.create(&member, json!({ "project_key": "PAY", "type": "bug", "summary": "b" })).await;
    assert_eq!(missing.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(missing.body["errors"][0]["field"], format!("custom_fields.{sid}"));
    assert_eq!(
        h.create(&member, json!({ "project_key": "PAY", "type": "task", "summary": "t" })).await.status,
        StatusCode::CREATED
    );
    assert_eq!(
        h.create(&member, json!({ "project_key": "OPS", "type": "bug", "summary": "b" })).await.status,
        StatusCode::CREATED
    );
    // ...and it cannot be set where it does not apply.
    let misplaced =
        h.create(&member, json!({ "project_key": "OPS", "type": "bug", "summary": "b", "custom_fields": { &sid: "S1" } })).await;
    assert_eq!(misplaced.status, StatusCode::UNPROCESSABLE_ENTITY);

    let bug = h
        .create(&member, json!({ "project_key": "PAY", "type": "bug", "summary": "b", "custom_fields": { &sid: "S2", &owner: me, &estimate: 2.5, &docs: "https://example.com/runbook" } }))
        .await;
    assert_eq!(bug.status, StatusCode::CREATED, "{}", bug.body);
    assert_eq!(bug.body["custom_fields"][&sid], "S2");
    let path = format!("/api/v1/issues/{}", bug.body["key"].as_str().unwrap());

    for bad in [
        json!({ &sid: "S9" }),
        json!({ &sid: null }),
        json!({ &estimate: "lots" }),
        json!({ &docs: "javascript:alert(1)" }),
        json!({ &owner: Uuid::now_v7() }),
        json!({ Uuid::now_v7().to_string(): "x" }),
    ] {
        let reply = h.call(Method::PATCH, &path, &member, Some(json!({ "custom_fields": bad })), Some(1)).await;
        assert_eq!(reply.status, StatusCode::UNPROCESSABLE_ENTITY, "{bad}");
    }
    // Only the named field changes; null clears an optional one.
    let updated =
        h.call(Method::PATCH, &path, &member, Some(json!({ "custom_fields": { &sid: "S1", &estimate: null } })), Some(1)).await;
    assert_eq!(updated.status, StatusCode::OK, "{}", updated.body);
    assert_eq!(updated.body["custom_fields"][&sid], "S1");
    assert!(updated.body["custom_fields"].get(&estimate).is_none());
    assert_eq!(updated.body["custom_fields"][&docs], "https://example.com/runbook");
    let history = h.call(Method::GET, &format!("{path}/history"), &member, None, None).await;
    assert_eq!(history.body["items"][0]["changes"][0], json!({ "field": "Severity", "from": "S2", "to": "S1", "custom": true }));
    assert_eq!(
        h.call(Method::GET, "/api/v1/issues?project=PAY&type=bug", &member, None, None).await.body["items"][0]["custom_fields"]
            [&sid],
        "S1"
    );

    // Editing a definition; archiving hides it and frees the name, values stay stored.
    let renamed = h
        .call(
            Method::PATCH,
            &format!("{fields}/{sid}"),
            &admin,
            Some(json!({ "name": "Impact", "options": ["S1", "S2", "S3", "S4"], "required": false })),
            None,
        )
        .await;
    assert_eq!(
        (renamed.status, renamed.body["field_type"].as_str(), renamed.body["project_keys"].clone()),
        (StatusCode::OK, Some("select"), json!([]))
    );
    assert_eq!(h.call(Method::DELETE, &format!("{fields}/{sid}"), &member, None, None).await.status, StatusCode::FORBIDDEN);
    assert_eq!(h.call(Method::DELETE, &format!("{fields}/{sid}"), &admin, None, None).await.status, StatusCode::NO_CONTENT);
    assert_eq!(h.call(Method::GET, fields, &member, None, None).await.body["items"].as_array().unwrap().len(), 3);
    assert_eq!(
        h.call(Method::PATCH, &path, &member, Some(json!({ "custom_fields": { &sid: "S3" } })), Some(2)).await.status,
        StatusCode::UNPROCESSABLE_ENTITY
    );

    // Another tenant sees none of these definitions.
    let other = h.auth.org_token(Uuid::now_v7(), "o@example.com", Uuid::now_v7(), Uuid::now_v7(), "owner").unwrap();
    assert!(h.call(Method::GET, fields, &other, None, None).await.body["items"].as_array().unwrap().is_empty());
    assert_eq!(h.call(Method::DELETE, &format!("{fields}/{owner}"), &other, None, None).await.status, StatusCode::NOT_FOUND);
    h.db.cleanup().await;
}

#[tokio::test]
async fn attachments_are_validated_authorized_and_served_as_downloads() {
    let h = harness().await;
    let (pay, secret) = (h.project("PAY"), h.project("SEC"));
    let (_, ada) = h.user(&[(pay, ProjectRole::Member), (secret, ProjectRole::Member)]);
    let (_, bob) = h.user(&[(pay, ProjectRole::Member)]);
    let (_, viewer) = h.user(&[(pay, ProjectRole::Viewer)]);
    let (_, admin) = h.user(&[(pay, ProjectRole::Administrator)]);
    let key = h.task(&ada, "PAY", "With files").await;
    let hidden = h.task(&ada, "SEC", "Hidden").await;
    let png: Vec<u8> = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A].into_iter().chain(std::iter::repeat_n(7, 2000)).collect();

    let up = h.upload(&ada, &key, "../../shot.png", &png).await;
    assert_eq!(up.status, StatusCode::CREATED, "{}", up.body);
    assert_eq!(
        (up.body["filename"].as_str(), up.body["content_type"].as_str(), up.body["size_bytes"].as_i64()),
        (Some("shot.png"), Some("image/png"), Some(2008))
    );
    let id = up.body["id"].as_str().unwrap().to_owned();

    // Refused: no permission, dangerous or empty files, oversize.
    assert_eq!(h.upload(&viewer, &key, "a.png", &png).await.status, StatusCode::FORBIDDEN);
    assert_eq!(h.upload(&bob, &hidden, "a.png", &png).await.status, StatusCode::NOT_FOUND);
    for (name, bytes) in [
        ("evil.html", &b"<html>"[..]),
        ("evil.svg", b"<svg/>"),
        ("notes.txt", b"<script>alert(1)</script>"),
        ("tool.exe", b"MZ"),
        ("empty.txt", b""),
    ] {
        assert_eq!(h.upload(&ada, &key, name, bytes).await.status, StatusCode::UNPROCESSABLE_ENTITY, "{name}");
    }
    let huge = vec![1_u8; 11 * 1024 * 1024];
    assert!(h.upload(&ada, &key, "big.bin", &huge).await.status.is_client_error());

    let list = h.call(Method::GET, &format!("/api/v1/issues/{key}/attachments"), &viewer, None, None).await;
    assert_eq!(list.body["items"].as_array().unwrap().len(), 1);
    assert!(list.body["items"][0].get("object_key").is_none(), "storage paths are never exposed");

    // Download: exact bytes, forced download, no sniffing.
    let req = Request::builder()
        .uri(format!("/api/v1/issues/{key}/attachments/{id}/content"))
        .header(header::AUTHORIZATION, format!("Bearer {viewer}"))
        .body(Body::empty())
        .unwrap();
    let response = h.app.clone().oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CONTENT_DISPOSITION], "attachment; filename=\"shot.png\"");
    assert_eq!(response.headers()[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
    assert_eq!(response.headers()[header::CONTENT_TYPE], "image/png");
    assert_eq!(axum::body::to_bytes(response.into_body(), 1 << 22).await.unwrap().as_ref(), png.as_slice());

    // The attachment id is only reachable through its own issue, by someone who can see it.
    let other_issue = h.task(&ada, "PAY", "Other").await;
    assert_eq!(
        h.call(Method::GET, &format!("/api/v1/issues/{other_issue}/attachments/{id}/content"), &ada, None, None).await.status,
        StatusCode::NOT_FOUND
    );
    let (_, outsider) = h.user(&[]);
    assert_eq!(
        h.call(Method::GET, &format!("/api/v1/issues/{key}/attachments/{id}/content"), &outsider, None, None).await.status,
        StatusCode::NOT_FOUND
    );

    // Members delete their own uploads; administrators any.
    let path = format!("/api/v1/issues/{key}/attachments/{id}");
    assert_eq!(h.call(Method::DELETE, &path, &bob, None, None).await.status, StatusCode::FORBIDDEN);
    assert_eq!(h.call(Method::DELETE, &path, &admin, None, None).await.status, StatusCode::NO_CONTENT);
    assert_eq!(h.call(Method::GET, &format!("{path}/content"), &ada, None, None).await.status, StatusCode::NOT_FOUND);
    let (stored,): (String,) = sqlx::query_as("SELECT object_key FROM attachments LIMIT 1").fetch_one(&h.db.owner).await.unwrap();
    assert_eq!(stored, format!("t/{}/{id}", h.tenant), "key is tenant-prefixed and server-generated");
    let history = h.call(Method::GET, &format!("/api/v1/issues/{key}/history"), &ada, None, None).await;
    assert_eq!(history.body["items"][0]["changes"][0]["field"], "attachment");
    h.db.cleanup().await;
}

#[tokio::test]
async fn status_changes_are_decided_by_the_workflow_engine() {
    let h = harness().await;
    let pay = h.project("PAY");
    let (ada, member) = h.user(&[(pay, ProjectRole::Member)]);
    let (_, viewer) = h.user(&[(pay, ProjectRole::Viewer)]);
    let key = h.task(&member, "PAY", "Governed").await;
    let path = format!("/api/v1/issues/{key}");
    h.world.denials.lock().unwrap().insert(("todo".into(), "done".into()), "An issue cannot move from To Do to Done.".into());
    h.world
        .denials
        .lock()
        .unwrap()
        .insert(("in_progress".into(), "in_review".into()), "Due date is required to move this issue to In Review.".into());
    h.world.effects.lock().unwrap().insert("in_progress".into(), vec![PostFunction::AssignToCurrentUser]);
    h.world
        .effects
        .lock()
        .unwrap()
        .insert("done".into(), vec![PostFunction::SetResolution("shipped".into()), PostFunction::ClearAssignee]);

    // What the caller is offered follows the engine; viewers are offered nothing.
    let offered = h.call(Method::GET, &format!("{path}/transitions"), &member, None, None).await;
    let targets: Vec<&str> = offered.body["items"].as_array().unwrap().iter().map(|t| t["to"]["key"].as_str().unwrap()).collect();
    assert_eq!(targets, ["in_progress", "in_review"]);
    assert_eq!(offered.body["items"][0]["name"], "Move to In Progress");
    assert_eq!(h.call(Method::GET, &format!("{path}/transitions"), &viewer, None, None).await.body["items"], json!([]));

    // A refused move changes nothing and carries the engine's explanation.
    let refused = h.call(Method::PATCH, &path, &member, Some(json!({ "status": "done", "summary": "Sneaky" })), Some(1)).await;
    assert_eq!(refused.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        refused.body["errors"][0],
        json!({ "field": "status", "code": "transition_denied", "message": "An issue cannot move from To Do to Done." })
    );
    let unchanged = h.call(Method::GET, &path, &member, None, None).await;
    assert_eq!(
        (unchanged.body["status"].as_str(), unchanged.body["summary"].as_str(), unchanged.body["version"].as_i64()),
        (Some("todo"), Some("Governed"), Some(1))
    );

    // Post-functions are applied with the move, in one version, and recorded.
    let started = h.call(Method::PATCH, &path, &member, Some(json!({ "status": "in_progress" })), Some(1)).await;
    assert_eq!(started.status, StatusCode::OK, "{}", started.body);
    assert_eq!(
        (started.body["status_name"].as_str(), started.body["status_category"].as_str()),
        (Some("In Progress"), Some("in_progress"))
    );
    assert_eq!(started.body["assignee_id"], ada.to_string(), "assign_to_current_user");
    assert_eq!(started.body["version"], 2);

    // The engine sees the issue as the request will leave it: fields set in the same
    // request count, and so do actor facts.
    h.world.denials.lock().unwrap().remove(&("in_progress".to_owned(), "in_review".to_owned()));
    let reviewed = h
        .call(
            Method::PATCH,
            &path,
            &member,
            Some(json!({ "status": "in_review", "due_date": "2026-12-01", "labels": ["x"] })),
            Some(2),
        )
        .await;
    assert_eq!(reviewed.status, StatusCode::OK, "{}", reviewed.body);
    let facts = h.world.last_facts.lock().unwrap().clone().unwrap();
    assert_eq!(facts.actor_role, "member");
    assert!(facts.actor_is_assignee && facts.actor_is_reporter && facts.children_done);
    for field in ["assignee", "due_date", "labels"] {
        assert!(facts.present_fields.iter().any(|f| f == field), "{field} missing from {:?}", facts.present_fields);
    }
    assert!(!facts.present_fields.iter().any(|f| f == "description" || f == "story_points"));

    // An open child makes children_done false.
    h.create(&member, json!({ "project_key": "PAY", "type": "subtask", "summary": "child", "parent_key": key })).await;
    h.call(Method::GET, &format!("{path}/transitions"), &member, None, None).await;
    assert!(!h.world.last_facts.lock().unwrap().clone().unwrap().children_done);

    // A workflow-chosen resolution replaces the default, and clear_assignee runs.
    let done = h.call(Method::PATCH, &path, &member, Some(json!({ "status": "done" })), Some(3)).await;
    assert_eq!((done.body["resolution"].as_str(), done.body["assignee_id"].is_null()), (Some("shipped"), true));
    let history = h.call(Method::GET, &format!("{path}/history"), &member, None, None).await;
    let last: Vec<&str> =
        history.body["items"][0]["changes"].as_array().unwrap().iter().map(|c| c["field"].as_str().unwrap()).collect();
    assert_eq!(last, ["status", "assignee", "resolution"]);

    // Viewers cannot transition even where the workflow would allow it.
    assert_eq!(
        h.call(Method::PATCH, &path, &viewer, Some(json!({ "status": "todo" })), Some(4)).await.status,
        StatusCode::FORBIDDEN
    );
    // Setting the status an issue already has is not a transition.
    let same = h.call(Method::PATCH, &path, &member, Some(json!({ "status": "done" })), Some(4)).await;
    assert_eq!((same.status, same.body["version"].as_i64()), (StatusCode::OK, Some(4)));
    h.db.cleanup().await;
}

#[tokio::test]
async fn cards_are_served_to_other_services_per_project() {
    use api_contracts::issue::v1::{issues_server::Issues, ListCardsRequest};
    use issue_service::grpc::IssuesService;

    let h = harness().await;
    let (pay, ops) = (h.project("PAY"), h.project("OPS"));
    let (_, member) = h.user(&[(pay, ProjectRole::Member), (ops, ProjectRole::Member)]);
    let epic = h.create(&member, json!({ "project_key": "PAY", "type": "epic", "summary": "Checkout" })).await.body["key"]
        .as_str()
        .unwrap()
        .to_owned();
    let story = h
        .create(
            &member,
            json!({ "project_key": "PAY", "type": "story", "summary": "Pay by card", "parent_key": epic, "story_points": 5 }),
        )
        .await;
    assert_eq!(story.status, StatusCode::CREATED, "{}", story.body);
    let finished = h.task(&member, "PAY", "Old work").await;
    let elsewhere = h.task(&member, "OPS", "Other project").await;
    let done =
        h.call(Method::PATCH, &format!("/api/v1/issues/{finished}"), &member, Some(json!({ "status": "done" })), Some(1)).await;
    assert_eq!(done.status, StatusCode::OK, "{}", done.body);

    let service = IssuesService { db: Arc::clone(&h.router) };
    let request = |project: Uuid| ListCardsRequest {
        tenant_id: h.tenant.to_string(),
        project_id: project.to_string(),
        ..ListCardsRequest::default()
    };
    let list = |req: ListCardsRequest| async { service.list_cards(tonic::Request::new(req)).await.unwrap().into_inner() };

    // Everything in the project, oldest first, with what a card needs.
    let all = list(request(pay)).await;
    assert_eq!(all.cards.iter().map(|c| c.key.as_str()).collect::<Vec<_>>(), ["PAY-1", "PAY-2", "PAY-3"]);
    let card = &all.cards[1];
    assert_eq!(
        (card.r#type.as_str(), card.story_points, card.parent_key.as_str(), card.parent_summary.as_str()),
        ("story", Some(5.0), "PAY-1", "Checkout")
    );
    assert_eq!(
        (all.cards[2].status_category.as_str(), all.cards[2].resolved_at.is_empty(), all.cards[2].version),
        ("done", false, 2)
    );

    // Board filter: no epics, and finished issues only when recent enough.
    let recent =
        list(ListCardsRequest { exclude_types: vec!["epic".into()], done_since: "2000-01-01T00:00:00Z".into(), ..request(pay) })
            .await;
    assert_eq!(recent.cards.iter().map(|c| c.key.as_str()).collect::<Vec<_>>(), ["PAY-2", "PAY-3"]);
    let open_only = list(ListCardsRequest { done_since: "2999-01-01T00:00:00Z".into(), ..request(pay) }).await;
    assert_eq!(open_only.cards.iter().map(|c| c.key.as_str()).collect::<Vec<_>>(), ["PAY-1", "PAY-2"]);

    // By id: only issues of the requested project, and the cap is reported.
    let other = h.call(Method::GET, &format!("/api/v1/issues/{elsewhere}"), &member, None, None).await.body["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let by_id = list(ListCardsRequest { issue_ids: vec![card.id.clone(), other], ..request(pay) }).await;
    assert_eq!(by_id.cards.iter().map(|c| c.key.as_str()).collect::<Vec<_>>(), ["PAY-2"]);
    let capped = list(ListCardsRequest { limit: 2, ..request(pay) }).await;
    assert_eq!((capped.cards.len(), capped.truncated), (2, true));

    // Another tenant sees nothing; malformed ids are rejected.
    let foreign = list(ListCardsRequest { tenant_id: Uuid::now_v7().to_string(), ..request(pay) }).await;
    assert!(foreign.cards.is_empty());
    let bad = service.list_cards(tonic::Request::new(ListCardsRequest { tenant_id: "nope".into(), ..request(pay) })).await;
    assert_eq!(bad.unwrap_err().code(), tonic::Code::InvalidArgument);
    h.db.cleanup().await;
}

#[tokio::test]
async fn audiences_and_search_documents_are_served_to_other_services() {
    use api_contracts::issue::v1::{issues_server::Issues, GetAudienceRequest, GetDocumentsRequest, ListIssueIdsRequest};
    use issue_service::grpc::IssuesService;

    let h = harness().await;
    let pay = h.project("PAY");
    let (alice, member) = h.user(&[(pay, ProjectRole::Member)]);
    let (bob, bob_token) = h.user(&[(pay, ProjectRole::Member)]);
    let key = h.task(&member, "PAY", "Refunds fail").await;
    let path = format!("/api/v1/issues/{key}");
    let edit = h
        .call(
            Method::PATCH,
            &path,
            &member,
            Some(json!({ "description": text_doc("Seen at checkout"), "assignee_id": bob, "labels": ["billing"] })),
            Some(1),
        )
        .await;
    assert_eq!(edit.status, StatusCode::OK, "{}", edit.body);
    h.call(Method::PUT, &format!("{path}/watch"), &bob_token, None, None).await;
    for text in ["First note", "Second note"] {
        let c = h.call(Method::POST, &format!("{path}/comments"), &member, Some(json!({ "body": text_doc(text) })), None).await;
        assert_eq!(c.status, StatusCode::CREATED, "{}", c.body);
    }
    let second = h.task(&member, "PAY", "Second issue").await;
    let mut ids = Vec::new();
    for k in [&key, &second] {
        ids.push(
            h.call(Method::GET, &format!("/api/v1/issues/{k}"), &member, None, None).await.body["id"]
                .as_str()
                .unwrap()
                .to_owned(),
        );
    }
    let (first_id, second_id) = (ids[0].clone(), ids[1].clone());

    let service = IssuesService { db: Arc::clone(&h.router) };
    let tenant = h.tenant.to_string();

    let audience = service
        .get_audience(tonic::Request::new(GetAudienceRequest { tenant_id: tenant.clone(), issue_id: first_id.clone() }))
        .await
        .unwrap()
        .into_inner();
    assert!(audience.found);
    assert_eq!(
        (audience.key.as_str(), audience.reporter_id, audience.assignee_id),
        (key.as_str(), alice.to_string(), bob.to_string())
    );
    assert!(audience.watcher_ids.contains(&bob.to_string()) && audience.watcher_ids.contains(&alice.to_string()));
    let missing = service
        .get_audience(tonic::Request::new(GetAudienceRequest {
            tenant_id: Uuid::now_v7().to_string(),
            issue_id: first_id.clone(),
        }))
        .await
        .unwrap()
        .into_inner();
    assert!(!missing.found, "another tenant cannot see it");

    let gone = Uuid::now_v7().to_string();
    let docs = service
        .get_documents(tonic::Request::new(GetDocumentsRequest {
            tenant_id: tenant.clone(),
            issue_ids: vec![first_id.clone(), gone.clone()],
        }))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(docs.missing_ids, [gone]);
    let d = &docs.documents[0];
    assert_eq!((d.key.as_str(), d.project_key.as_str(), d.description_text.trim()), (key.as_str(), "PAY", "Seen at checkout"));
    assert!(d.comments_text.find("First note").unwrap() < d.comments_text.find("Second note").unwrap(), "newest last");
    assert_eq!((d.labels.clone(), d.assignee_id.clone()), (vec!["billing".to_owned()], bob.to_string()));
    let too_many = GetDocumentsRequest { tenant_id: tenant.clone(), issue_ids: vec![first_id.clone(); 501] };
    assert_eq!(service.get_documents(tonic::Request::new(too_many)).await.unwrap_err().code(), tonic::Code::InvalidArgument);

    let page = |after: &str, limit: u32| {
        let req = ListIssueIdsRequest { tenant_id: tenant.clone(), after_id: after.to_owned(), limit };
        let service = &service;
        async move { service.list_issue_ids(tonic::Request::new(req)).await.unwrap().into_inner().ids }
    };
    assert_eq!(page("", 1000).await, [first_id.clone(), second_id.clone()]);
    assert_eq!(page("", 1).await, std::slice::from_ref(&first_id));
    assert_eq!(page(&first_id, 10).await, [second_id]);
    h.db.cleanup().await;
}

/// Migration 0003 tried to backfill status categories while row-level security was
/// forced, so it changed nothing; 0004 must repair rows written before either ran.
#[tokio::test]
async fn status_categories_of_existing_issues_are_backfilled() {
    let db = TestDb::create_until(&MIGRATOR, 3).await.expect("test database");
    let tenant = Uuid::now_v7();
    let project = Uuid::now_v7();
    let mut tx = db.owner.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.tenant_id', $1, true)").bind(tenant.to_string()).execute(&mut *tx).await.unwrap();
    for (number, status, category) in
        [(1, "todo", "todo"), (2, "in_progress", "todo"), (3, "in_review", "todo"), (4, "done", "todo"), (5, "shipped", "done")]
    {
        sqlx::query(
            "INSERT INTO issues (id, tenant_id, project_id, project_key, number, type_key, status, status_category, priority, summary, reporter_id, creator_id) \
             VALUES ($1, $2, $3, 'PAY', $4, 'task', $5, $6, 'medium', 'x', $2, $2)",
        )
        .bind(Uuid::now_v7())
        .bind(tenant)
        .bind(project)
        .bind(number)
        .bind(status)
        .bind(category)
        .execute(&mut *tx)
        .await
        .unwrap();
    }
    tx.commit().await.unwrap();

    db.migrate(&MIGRATOR).await.expect("remaining migrations");

    let rows: Vec<(String, String)> =
        sqlx::query_as("SELECT status, status_category FROM issues ORDER BY number").fetch_all(&db.owner).await.unwrap();
    let expected =
        [("todo", "todo"), ("in_progress", "in_progress"), ("in_review", "in_progress"), ("done", "done"), ("shipped", "done")];
    assert_eq!(rows, expected.map(|(a, b)| (a.to_owned(), b.to_owned())));
    // Row-level security is back in force for the runtime role.
    let visible: i64 = sqlx::query_scalar("SELECT count(*) FROM issues").fetch_one(&db.app).await.unwrap();
    assert_eq!(visible, 0);
    db.cleanup().await;
}
