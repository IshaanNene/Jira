use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use agile_service::{
    http,
    sources::{Card, Issues, ProjectRef, Projects, OFF_BOARD_TYPES},
    AppState, MIGRATOR,
};
use async_trait::async_trait;
use auth_middleware::{PolicyClient, PolicySnapshot, PolicySource, ProjectRole, RedisHandle};
use axum::{
    body::Body,
    http::{header, Method, Request, StatusCode},
    Router,
};
use error_handling::{ApiError, ApiResult};
use serde_json::{json, Value};
use tenant_context::DbRouter;
use test_utilities::{TestAuth, TestDb};
use time::OffsetDateTime;
use tower::ServiceExt;
use uuid::Uuid;

/// Stand-in for the project, issue and permission services.
#[derive(Default)]
struct World {
    projects: Mutex<HashMap<(Uuid, String), ProjectRef>>,
    issues: Mutex<Vec<(Uuid, Uuid, Card)>>, // (tenant, project, card)
    roles: Mutex<HashMap<(Uuid, Uuid), HashMap<Uuid, ProjectRole>>>,
}

#[async_trait]
impl Projects for World {
    async fn project(&self, tenant_id: Uuid, key: &str) -> ApiResult<ProjectRef> {
        self.projects.lock().unwrap().get(&(tenant_id, key.to_owned())).cloned().ok_or(ApiError::NotFound("project"))
    }
}

#[async_trait]
impl Issues for World {
    async fn open_cards(&self, tenant_id: Uuid, project_id: Uuid, done_since: OffsetDateTime) -> ApiResult<(Vec<Card>, bool)> {
        let cards = self
            .issues
            .lock()
            .unwrap()
            .iter()
            .filter(|(t, p, c)| {
                *t == tenant_id
                    && *p == project_id
                    && !OFF_BOARD_TYPES.contains(&c.issue_type.as_str())
                    && (!c.done() || c.resolved_at.is_some_and(|at| at >= done_since))
            })
            .map(|(_, _, c)| c.clone())
            .collect();
        Ok((cards, false))
    }
    async fn cards_by_id(&self, tenant_id: Uuid, project_id: Uuid, ids: &[Uuid]) -> ApiResult<Vec<Card>> {
        Ok(self
            .issues
            .lock()
            .unwrap()
            .iter()
            .filter(|(t, p, c)| *t == tenant_id && *p == project_id && ids.contains(&c.id))
            .map(|(_, _, c)| c.clone())
            .collect())
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
    app: Router,
    auth: TestAuth,
    world: Arc<World>,
    tenant: Uuid,
    org: Uuid,
}

struct Reply {
    status: StatusCode,
    body: Value,
}

async fn harness() -> Harness {
    let db = TestDb::create(&MIGRATOR).await.expect("test database");
    let auth = TestAuth::new().unwrap();
    let world = Arc::new(World::default());
    let state = AppState {
        db: Arc::new(DbRouter::single(db.app.clone())),
        verifier: Arc::clone(&auth.verifier),
        policy: Arc::new(PolicyClient::new(world.clone(), RedisHandle::default())),
        projects: world.clone(),
        issues: world.clone(),
        allowed_origins: Arc::new(vec![]),
    };
    Harness { db, app: http::router(state), auth, world, tenant: Uuid::now_v7(), org: Uuid::now_v7() }
}

impl Harness {
    fn project(&self, key: &str) -> Uuid {
        let id = Uuid::now_v7();
        self.world
            .projects
            .lock()
            .unwrap()
            .insert((self.tenant, key.to_owned()), ProjectRef { id, key: key.to_owned(), archived: false });
        id
    }

    fn user(&self, roles: &[(Uuid, ProjectRole)]) -> String {
        self.user_in(self.tenant, roles)
    }

    fn user_in(&self, tenant: Uuid, roles: &[(Uuid, ProjectRole)]) -> String {
        let id = Uuid::now_v7();
        self.world.roles.lock().unwrap().insert((tenant, id), roles.iter().copied().collect());
        self.auth.org_token(id, &format!("{id}@example.com"), tenant, self.org, "member").unwrap()
    }

    /// Adds an issue to the fake issue service. Returns its id.
    fn issue(&self, project: Uuid, number: i64, issue_type: &str, points: Option<f64>) -> Uuid {
        let id = Uuid::now_v7();
        let card = Card {
            id,
            key: format!("DEMO-{number}"),
            number,
            issue_type: issue_type.into(),
            status: "todo".into(),
            status_category: "todo".into(),
            priority: "medium".into(),
            summary: format!("Issue {number}"),
            assignee_id: None,
            story_points: points,
            resolved_at: None,
            parent_key: None,
            parent_summary: None,
            version: 1,
            labels: vec![],
        };
        self.world.issues.lock().unwrap().push((self.tenant, project, card));
        id
    }

    fn set_status(&self, issue: Uuid, status: &str, category: &str) {
        let mut issues = self.world.issues.lock().unwrap();
        let card = &mut issues.iter_mut().find(|(_, _, c)| c.id == issue).unwrap().2;
        card.status = status.into();
        card.status_category = category.into();
        card.resolved_at = (category == "done").then(OffsetDateTime::now_utc);
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
        let bytes = axum::body::to_bytes(response.into_body(), 1 << 22).await.unwrap();
        Reply { status, body: serde_json::from_slice(&bytes).unwrap_or(Value::Null) }
    }

    async fn get(&self, path: &str, token: &str) -> Value {
        let reply = self.call(Method::GET, path, token, None, None).await;
        assert_eq!(reply.status, StatusCode::OK, "GET {path}: {}", reply.body);
        reply.body
    }

    async fn post(&self, path: &str, token: &str, body: Value) -> Reply {
        self.call(Method::POST, path, token, Some(body), None).await
    }

    async fn board(&self, admin: &str, kind: &str) -> String {
        let created = self
            .post("/api/v1/boards", admin, json!({ "project_key": "DEMO", "name": format!("{kind} board"), "kind": kind }))
            .await;
        assert_eq!(created.status, StatusCode::CREATED, "{}", created.body);
        created.body["id"].as_str().unwrap().to_owned()
    }

    async fn outbox(&self) -> Vec<String> {
        sqlx::query_scalar("SELECT event_type FROM outbox ORDER BY created_at, id").fetch_all(&self.db.app).await.unwrap()
    }
}

fn keys(cards: &Value) -> Vec<&str> {
    cards.as_array().unwrap().iter().map(|c| c["key"].as_str().unwrap()).collect()
}

#[tokio::test]
async fn boards_are_configured_by_project_administrators() {
    let h = harness().await;
    let project = h.project("DEMO");
    let admin = h.user(&[(project, ProjectRole::Administrator)]);
    let member = h.user(&[(project, ProjectRole::Member)]);
    let outsider = h.user(&[]);
    let new_board = json!({ "project_key": "DEMO", "name": " Team board ", "kind": "kanban" });

    assert_eq!(h.post("/api/v1/boards", &member, new_board.clone()).await.status, StatusCode::FORBIDDEN);
    assert_eq!(h.post("/api/v1/boards", &outsider, new_board.clone()).await.status, StatusCode::NOT_FOUND);
    let bad_kind = h.post("/api/v1/boards", &admin, json!({ "project_key": "DEMO", "name": "X", "kind": "waterfall" })).await;
    assert_eq!(bad_kind.status, StatusCode::UNPROCESSABLE_ENTITY);

    let created = h.post("/api/v1/boards", &admin, new_board.clone()).await;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.body);
    assert_eq!(created.body["name"], "Team board");
    assert_eq!(created.body["columns"].as_array().unwrap().len(), 3);
    assert_eq!(created.body["permissions"]["manage_board"], true);
    let id = created.body["id"].as_str().unwrap();
    let path = format!("/api/v1/boards/{id}");
    assert_eq!(h.post("/api/v1/boards", &admin, new_board).await.status, StatusCode::CONFLICT);

    // Members see the board but cannot manage it; people without access cannot tell it exists.
    let listed = h.get("/api/v1/boards?project_key=DEMO", &member).await;
    assert_eq!(
        listed["items"][0]["permissions"],
        json!({ "manage_board": false, "manage_sprints": true, "move_issues": true, "transition_issues": true })
    );
    assert_eq!(h.call(Method::GET, &path, &outsider, None, None).await.status, StatusCode::NOT_FOUND);

    let columns = json!({ "columns": [
        { "id": "todo", "name": "To Do", "category": "todo" },
        { "id": "doing", "name": "Doing", "category": "in_progress", "wip_limit": 3 },
        { "id": "review", "name": "Review", "statuses": ["in_review"] },
        { "id": "done", "name": "Done", "category": "done" }
    ] });
    assert_eq!(h.call(Method::PATCH, &path, &member, Some(columns.clone()), Some(1)).await.status, StatusCode::FORBIDDEN);
    assert_eq!(h.call(Method::PATCH, &path, &admin, Some(columns.clone()), None).await.status, StatusCode::PRECONDITION_REQUIRED);
    let ambiguous =
        json!({ "columns": [{ "id": "a", "name": "A", "statuses": ["x"] }, { "id": "b", "name": "B", "statuses": ["x"] }] });
    let refused = h.call(Method::PATCH, &path, &admin, Some(ambiguous), Some(1)).await;
    assert_eq!(refused.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(refused.body["errors"][0]["code"], "duplicate_status");

    let updated = h.call(Method::PATCH, &path, &admin, Some(columns.clone()), Some(1)).await;
    assert_eq!(updated.status, StatusCode::OK, "{}", updated.body);
    assert_eq!(updated.body["columns"][1]["wip_limit"], 3);
    assert_eq!(updated.body["version"], 2);
    let stale = h.call(Method::PATCH, &path, &admin, Some(columns), Some(1)).await;
    assert_eq!(stale.status, StatusCode::CONFLICT);
    assert_eq!(stale.body["code"], "version_conflict");

    assert_eq!(h.call(Method::DELETE, &path, &admin, None, None).await.status, StatusCode::NO_CONTENT);
    assert_eq!(h.call(Method::GET, &path, &admin, None, None).await.status, StatusCode::NOT_FOUND);
    assert_eq!(h.outbox().await, ["board.updated", "board.updated", "board.updated"]);
    h.db.cleanup().await;
}

#[tokio::test]
async fn backlog_order_is_kept_and_changed_by_moves() {
    let h = harness().await;
    let project = h.project("DEMO");
    let admin = h.user(&[(project, ProjectRole::Administrator)]);
    let viewer = h.user(&[(project, ProjectRole::Viewer)]);
    let ids: Vec<Uuid> = (1..=5).map(|n| h.issue(project, n, "task", None)).collect();
    let epic = h.issue(project, 6, "epic", None);
    let board = h.board(&admin, "scrum").await;
    let backlog_path = format!("/api/v1/boards/{board}/backlog");
    let move_path = format!("/api/v1/boards/{board}/move");

    // First sight: creation order; epics are not planned on boards.
    let backlog = h.get(&backlog_path, &viewer).await;
    assert_eq!(keys(&backlog["backlog"]), ["DEMO-1", "DEMO-2", "DEMO-3", "DEMO-4", "DEMO-5"]);

    // Two issues to the top (in the order given), one after another issue, one to the bottom.
    let top = json!({ "issue_ids": [ids[4], ids[3]], "before_id": ids[0] });
    assert_eq!(h.post(&move_path, &admin, top).await.status, StatusCode::NO_CONTENT);
    assert_eq!(
        h.post(&move_path, &admin, json!({ "issue_ids": [ids[0]], "after_id": ids[1] })).await.status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(h.post(&move_path, &admin, json!({ "issue_ids": [ids[4]] })).await.status, StatusCode::NO_CONTENT);
    let backlog = h.get(&backlog_path, &admin).await;
    assert_eq!(keys(&backlog["backlog"]), ["DEMO-4", "DEMO-2", "DEMO-1", "DEMO-3", "DEMO-5"]);

    // An issue created later joins at the bottom without disturbing the order.
    h.issue(project, 7, "bug", None);
    let backlog = h.get(&backlog_path, &admin).await;
    assert_eq!(keys(&backlog["backlog"]), ["DEMO-4", "DEMO-2", "DEMO-1", "DEMO-3", "DEMO-5", "DEMO-7"]);

    // Finished issues leave the backlog.
    h.set_status(ids[1], "done", "done");
    assert_eq!(keys(&h.get(&backlog_path, &admin).await["backlog"]), ["DEMO-4", "DEMO-1", "DEMO-3", "DEMO-5", "DEMO-7"]);

    let code = |reply: &Reply| reply.body["errors"][0]["code"].as_str().unwrap_or_default().to_owned();
    assert_eq!(h.post(&move_path, &viewer, json!({ "issue_ids": [ids[0]] })).await.status, StatusCode::FORBIDDEN);
    assert_eq!(code(&h.post(&move_path, &admin, json!({ "issue_ids": [Uuid::now_v7()] })).await), "unknown_issue");
    assert_eq!(code(&h.post(&move_path, &admin, json!({ "issue_ids": [epic] })).await), "not_plannable");
    assert_eq!(code(&h.post(&move_path, &admin, json!({ "issue_ids": [] })).await), "invalid_selection");
    assert_eq!(code(&h.post(&move_path, &admin, json!({ "issue_ids": [ids[0]], "after_id": ids[0] })).await), "invalid_position");
    assert_eq!(
        code(&h.post(&move_path, &admin, json!({ "issue_ids": [ids[0]], "sprint_id": Uuid::now_v7() })).await),
        "unknown_sprint"
    );
    h.db.cleanup().await;
}

#[tokio::test]
async fn sprint_lifecycle_from_planning_to_report() {
    let h = harness().await;
    let project = h.project("DEMO");
    let admin = h.user(&[(project, ProjectRole::Administrator)]);
    let viewer = h.user(&[(project, ProjectRole::Viewer)]);
    let a = h.issue(project, 1, "story", Some(5.0));
    let b = h.issue(project, 2, "task", Some(3.0));
    let c = h.issue(project, 3, "bug", None);
    let d = h.issue(project, 4, "task", Some(2.0));
    let board = h.board(&admin, "scrum").await;
    let kanban = h.board(&admin, "kanban").await;
    let sprints_path = format!("/api/v1/boards/{board}/sprints");
    let move_path = format!("/api/v1/boards/{board}/move");

    assert_eq!(h.post(&format!("/api/v1/boards/{kanban}/sprints"), &admin, json!({})).await.status, StatusCode::CONFLICT);
    assert_eq!(h.post(&sprints_path, &viewer, json!({})).await.status, StatusCode::FORBIDDEN);
    let first = h.post(&sprints_path, &admin, json!({ "goal": "Ship login" })).await;
    assert_eq!(first.status, StatusCode::CREATED, "{}", first.body);
    assert_eq!(first.body["name"], "DEMO Sprint 1");
    assert_eq!(first.body["state"], "future");
    let s1 = first.body["id"].as_str().unwrap().to_owned();
    let second = h.post(&sprints_path, &admin, json!({ "name": "Hardening" })).await;
    let s2 = second.body["id"].as_str().unwrap().to_owned();
    let dates = json!({ "start_date": "2026-05-10", "end_date": "2026-05-01" });
    assert_eq!(h.post(&sprints_path, &admin, dates).await.status, StatusCode::UNPROCESSABLE_ENTITY);

    // An empty sprint cannot start.
    let start = format!("/api/v1/sprints/{s1}/start");
    let empty = h.post(&start, &admin, json!({})).await;
    assert_eq!(empty.body["errors"][0]["code"], "empty_sprint", "{}", empty.body);

    // Plan three issues into sprint 1 and one into sprint 2.
    assert_eq!(
        h.post(&move_path, &admin, json!({ "issue_ids": [a, b, c], "sprint_id": s1 })).await.status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(h.post(&move_path, &admin, json!({ "issue_ids": [d], "sprint_id": s2 })).await.status, StatusCode::NO_CONTENT);
    let backlog = h.get(&format!("/api/v1/boards/{board}/backlog"), &viewer).await;
    assert_eq!(keys(&backlog["sprints"][0]["issues"]), ["DEMO-1", "DEMO-2", "DEMO-3"]);
    assert_eq!(keys(&backlog["sprints"][1]["issues"]), ["DEMO-4"]);
    assert!(keys(&backlog["backlog"]).is_empty());

    // Nothing is on the board until a sprint runs.
    let view_path = format!("/api/v1/boards/{board}/view");
    let idle = h.get(&view_path, &viewer).await;
    assert!(idle["sprint"].is_null());
    assert!(idle["columns"].as_array().unwrap().iter().all(|col| col["issues"].as_array().unwrap().is_empty()));

    let started = h.post(&start, &admin, json!({ "end_date": "2099-01-01" })).await;
    assert_eq!(started.status, StatusCode::OK, "{}", started.body);
    assert_eq!(started.body["state"], "active");
    assert_eq!(started.body["commitment"], json!({ "issues": 3, "points": 8.0 }));
    assert_eq!(h.post(&start, &admin, json!({})).await.status, StatusCode::CONFLICT);
    let overlapping = h.post(&format!("/api/v1/sprints/{s2}/start"), &admin, json!({})).await;
    assert_eq!(overlapping.status, StatusCode::CONFLICT);
    assert!(overlapping.body["detail"].as_str().unwrap().contains("already running"), "{}", overlapping.body);
    assert_eq!(h.call(Method::DELETE, &format!("/api/v1/sprints/{s1}"), &admin, None, None).await.status, StatusCode::CONFLICT);

    // The board shows the running sprint, each card in the column of its status.
    h.set_status(a, "done", "done");
    h.set_status(b, "in_review", "in_progress");
    let view = h.get(&view_path, &viewer).await;
    assert_eq!(view["sprint"]["id"], s1.as_str());
    let column =
        |name: &str| view["columns"].as_array().unwrap().iter().find(|col| col["name"] == name).unwrap()["issues"].clone();
    assert_eq!(keys(&column("To Do")), ["DEMO-3"]);
    assert_eq!(keys(&column("In Progress")), ["DEMO-2"]);
    assert_eq!(keys(&column("Done")), ["DEMO-1"]);
    // The Kanban board of the same project shows everything, sprint or not.
    let all = h.get(&format!("/api/v1/boards/{kanban}/view"), &viewer).await;
    assert_eq!(all["columns"].as_array().unwrap().iter().map(|col| col["issues"].as_array().unwrap().len()).sum::<usize>(), 4);

    // Live report while running.
    let report_path = format!("/api/v1/sprints/{s1}/report");
    let live = h.get(&report_path, &viewer).await;
    assert_eq!(live["outcome"]["completed_points"], 5.0);
    assert_eq!(live["outcome"]["incomplete_points"], 3.0);
    let last = live["outcome"]["burndown"].as_array().unwrap().last().unwrap().clone();
    assert_eq!(
        (last["scope_issues"].as_u64(), last["remaining_issues"].as_u64(), last["remaining_points"].as_f64()),
        (Some(3), Some(2), Some(3.0))
    );
    assert_eq!(
        h.call(Method::GET, &format!("/api/v1/sprints/{s2}/report"), &viewer, None, None).await.status,
        StatusCode::CONFLICT
    );

    // Complete: unfinished issues move to the next sprint, finished ones stay.
    let complete = format!("/api/v1/sprints/{s1}/complete");
    let wrong = h.post(&complete, &admin, json!({ "move_incomplete_to": Uuid::now_v7() })).await;
    assert_eq!(wrong.body["errors"][0]["code"], "unknown_sprint", "{}", wrong.body);
    let done = h.post(&complete, &admin, json!({ "move_incomplete_to": s2 })).await;
    assert_eq!(done.status, StatusCode::OK, "{}", done.body);
    assert_eq!(done.body["state"], "closed");
    assert_eq!(h.post(&complete, &admin, json!({})).await.status, StatusCode::CONFLICT);

    let backlog = h.get(&format!("/api/v1/boards/{board}/backlog"), &viewer).await;
    assert_eq!(backlog["sprints"].as_array().unwrap().len(), 1);
    assert_eq!(keys(&backlog["sprints"][0]["issues"]), ["DEMO-2", "DEMO-3", "DEMO-4"]);

    // The report is frozen: reopening the finished issue afterwards does not change it.
    h.set_status(a, "todo", "todo");
    let frozen = h.get(&report_path, &viewer).await;
    assert_eq!(keys(&frozen["outcome"]["completed"]), ["DEMO-1"]);
    assert_eq!(keys(&frozen["outcome"]["incomplete"]), ["DEMO-2", "DEMO-3"]);
    // …and the reopened issue is back in the backlog, since its sprint is over.
    assert_eq!(keys(&h.get(&format!("/api/v1/boards/{board}/backlog"), &viewer).await["backlog"]), ["DEMO-1"]);

    let velocity = h.get(&format!("/api/v1/boards/{board}/velocity"), &viewer).await;
    assert_eq!(
        velocity["sprints"],
        json!([{ "sprint_id": s1, "name": "DEMO Sprint 1", "committed_issues": 3, "committed_points": 8.0, "completed_issues": 1, "completed_points": 5.0 }])
    );

    // Planned sprints can be edited with If-Match and deleted; their issues return to the backlog.
    let sprint_path = format!("/api/v1/sprints/{s2}");
    let renamed = h
        .call(Method::PATCH, &sprint_path, &admin, Some(json!({ "name": "Sprint two", "end_date": "2026-06-01" })), Some(1))
        .await;
    assert_eq!(renamed.status, StatusCode::OK, "{}", renamed.body);
    assert_eq!((renamed.body["name"].as_str(), renamed.body["end_date"].as_str()), (Some("Sprint two"), Some("2026-06-01")));
    assert_eq!(
        h.call(Method::PATCH, &sprint_path, &admin, Some(json!({ "name": "Late" })), Some(1)).await.status,
        StatusCode::CONFLICT
    );
    assert_eq!(h.call(Method::DELETE, &sprint_path, &admin, None, None).await.status, StatusCode::NO_CONTENT);
    let backlog = h.get(&format!("/api/v1/boards/{board}/backlog"), &viewer).await;
    assert_eq!(keys(&backlog["backlog"]), ["DEMO-1", "DEMO-2", "DEMO-3", "DEMO-4"]);

    let events = h.outbox().await;
    for expected in ["sprint.created", "sprint.started", "sprint.completed", "issue.ranked"] {
        assert!(events.iter().any(|e| e == expected), "missing {expected} in {events:?}");
    }
    h.db.cleanup().await;
}

#[tokio::test]
async fn kanban_columns_follow_explicit_mapping_then_category() {
    let h = harness().await;
    let project = h.project("DEMO");
    let admin = h.user(&[(project, ProjectRole::Administrator)]);
    let (a, b, c) = (h.issue(project, 1, "task", None), h.issue(project, 2, "task", None), h.issue(project, 3, "task", None));
    h.set_status(a, "in_review", "in_progress");
    h.set_status(b, "building", "in_progress");
    h.set_status(c, "shipped", "done");
    let board = h.board(&admin, "kanban").await;
    let columns = json!({ "columns": [
        { "id": "doing", "name": "Doing", "category": "in_progress" },
        { "id": "review", "name": "Review", "statuses": ["in_review"] }
    ] });
    assert_eq!(
        h.call(Method::PATCH, &format!("/api/v1/boards/{board}"), &admin, Some(columns), Some(1)).await.status,
        StatusCode::OK
    );
    let view = h.get(&format!("/api/v1/boards/{board}/view"), &admin).await;
    assert_eq!(keys(&view["columns"][0]["issues"]), ["DEMO-2"]);
    assert_eq!(keys(&view["columns"][1]["issues"]), ["DEMO-1"]);
    assert_eq!(keys(&view["unmapped"]), ["DEMO-3"]);
    h.db.cleanup().await;
}

#[tokio::test]
async fn boards_and_sprints_never_cross_tenants() {
    let h = harness().await;
    let project = h.project("DEMO");
    let admin = h.user(&[(project, ProjectRole::Administrator)]);
    let issue = h.issue(project, 1, "task", None);
    let board = h.board(&admin, "scrum").await;
    let sprint =
        h.post(&format!("/api/v1/boards/{board}/sprints"), &admin, json!({})).await.body["id"].as_str().unwrap().to_owned();
    h.get(&format!("/api/v1/boards/{board}/backlog"), &admin).await;

    // Same project id, same role, different tenant: nothing is visible or writable.
    let other_tenant = Uuid::now_v7();
    let intruder = h.user_in(other_tenant, &[(project, ProjectRole::Administrator)]);
    for path in
        [format!("/api/v1/boards/{board}"), format!("/api/v1/boards/{board}/backlog"), format!("/api/v1/sprints/{sprint}")]
    {
        assert_eq!(h.call(Method::GET, &path, &intruder, None, None).await.status, StatusCode::NOT_FOUND, "{path}");
    }
    let moved = h.post(&format!("/api/v1/boards/{board}/move"), &intruder, json!({ "issue_ids": [issue] })).await;
    assert_eq!(moved.status, StatusCode::NOT_FOUND);
    assert_eq!(h.post(&format!("/api/v1/sprints/{sprint}/start"), &intruder, json!({})).await.status, StatusCode::NOT_FOUND);

    // The database itself refuses to show rows without the tenant set.
    for table in ["boards", "sprints", "issue_placements"] {
        let visible: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {table}")).fetch_one(&h.db.app).await.unwrap();
        assert_eq!(visible, 0, "{table} leaked rows without a tenant");
    }
    h.db.cleanup().await;
}

fn event(tenant: Uuid, topic: &str, event_type: &str, payload: Value) -> event_bus::Event {
    let raw = json!({
        "event_id": Uuid::now_v7(), "event_type": event_type, "schema_version": 1, "tenant_id": tenant,
        "aggregate_type": "issue", "aggregate_id": Uuid::now_v7(), "aggregate_version": 1,
        "occurred_at": "2026-11-13T09:00:00Z", "correlation_id": null, "causation_id": null,
        "actor": {"type": "user", "id": Uuid::now_v7()}, "payload": payload
    });
    event_bus::Event::parse(topic, &serde_json::to_vec(&raw).unwrap()).unwrap()
}

#[tokio::test]
async fn deleted_issues_and_projects_leave_the_boards() {
    use agile_service::cleanup::Cleanup;
    use event_bus::Handler;
    use event_contracts::topics;

    let h = harness().await;
    let project = h.project("DEMO");
    let admin = h.user(&[(project, ProjectRole::Administrator)]);
    let (a, b, c) =
        (h.issue(project, 1, "story", Some(5.0)), h.issue(project, 2, "task", Some(3.0)), h.issue(project, 3, "task", None));
    let board = h.board(&admin, "scrum").await;
    let sprint =
        h.post(&format!("/api/v1/boards/{board}/sprints"), &admin, json!({})).await.body["id"].as_str().unwrap().to_owned();
    let moved =
        h.post(&format!("/api/v1/boards/{board}/move"), &admin, json!({ "issue_ids": [a, b], "sprint_id": sprint })).await;
    assert_eq!(moved.status, StatusCode::NO_CONTENT);
    h.post(&format!("/api/v1/boards/{board}/move"), &admin, json!({ "issue_ids": [c], "sprint_id": null })).await;
    assert_eq!(h.post(&format!("/api/v1/sprints/{sprint}/start"), &admin, json!({})).await.status, StatusCode::OK);

    let cleanup = Cleanup { db: Arc::new(DbRouter::single(h.db.app.clone())) };
    let placements = || async {
        let ids: Vec<Uuid> =
            sqlx::query_scalar("SELECT issue_id FROM issue_placements ORDER BY issue_id").fetch_all(&h.db.owner).await.unwrap();
        ids
    };
    let removed = || async {
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM sprint_scope_events WHERE action = 'removed'")
            .fetch_one(&h.db.owner)
            .await
            .unwrap();
        n
    };

    // An issue in the running sprint leaves its scope; one in the backlog just disappears.
    h.world.issues.lock().unwrap().retain(|(_, _, card)| card.id != a);
    let deleted =
        event(h.tenant, topics::ISSUE, "issue.deleted", json!({ "issue_id": a, "key": "DEMO-1", "project_id": project }));
    cleanup.handle(&deleted).await.unwrap();
    cleanup.handle(&deleted).await.unwrap(); // redelivered: no second scope entry
    assert_eq!(removed().await, 1);
    cleanup
        .handle(&event(
            h.tenant,
            topics::ISSUE,
            "issue.deleted",
            json!({ "issue_id": c, "key": "DEMO-3", "project_id": project }),
        ))
        .await
        .unwrap();
    assert_eq!(removed().await, 1, "backlog issues are not sprint scope");
    assert_eq!(placements().await, [b]);

    // Another tenant's event about the same ids changes nothing here.
    cleanup
        .handle(&event(
            Uuid::now_v7(),
            topics::ISSUE,
            "issue.deleted",
            json!({ "issue_id": b, "key": "DEMO-2", "project_id": project }),
        ))
        .await
        .unwrap();
    assert_eq!(placements().await, [b]);

    // A deleted project takes its boards and backlog order with it.
    cleanup
        .handle(&event(h.tenant, topics::PROJECT, "project.deleted", json!({ "project_id": project, "key": "DEMO" })))
        .await
        .unwrap();
    assert!(placements().await.is_empty());
    assert_eq!(h.call(Method::GET, &format!("/api/v1/boards/{board}"), &admin, None, None).await.status, StatusCode::NOT_FOUND);
    h.db.cleanup().await;
}
