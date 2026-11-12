use std::sync::Arc;

use api_contracts::workflow::v1::{
    workflow_engine_server::WorkflowEngine, EvaluateRequest, Facts, TransitionsRequest, WorkflowRequest,
};
use auth_middleware::{PolicyClient, ProjectRole, RedisHandle, StaticPolicy};
use axum::{
    body::Body,
    http::{header, Method, Request, StatusCode},
    Router,
};
use serde_json::{json, Value};
use tenant_context::DbRouter;
use test_utilities::{TestAuth, TestDb};
use tower::ServiceExt;
use uuid::Uuid;
use workflow_service::{grpc::EngineService, http, AppState, MIGRATOR};

struct Harness {
    db: TestDb,
    app: Router,
    auth: TestAuth,
    engine: EngineService,
    policy: Arc<StaticPolicy>,
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
    let router = Arc::new(DbRouter::single(db.app.clone()));
    let policy = Arc::new(StaticPolicy::default());
    let state = AppState {
        db: Arc::clone(&router),
        verifier: Arc::clone(&auth.verifier),
        policy: Arc::new(PolicyClient::new(policy.clone(), RedisHandle::default())),
        allowed_origins: Arc::new(vec![]),
    };
    Harness {
        db,
        app: http::router(state),
        auth,
        engine: EngineService { db: router },
        policy,
        tenant: Uuid::now_v7(),
        org: Uuid::now_v7(),
    }
}

impl Harness {
    fn token(&self, org_role: &str) -> String {
        let id = Uuid::now_v7();
        self.auth.org_token(id, &format!("{id}@example.com"), self.tenant, self.org, org_role).unwrap()
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

    fn request(&self, project: Uuid, issue_type: &str) -> WorkflowRequest {
        WorkflowRequest { tenant_id: self.tenant.to_string(), project_id: project.to_string(), issue_type: issue_type.into() }
    }

    async fn workflow_name(&self, project: Uuid, issue_type: &str) -> String {
        self.engine.get_workflow(tonic::Request::new(self.request(project, issue_type))).await.unwrap().into_inner().name
    }

    /// Create a workflow from `definition` and publish it. Returns its id.
    async fn published(&self, admin: &str, name: &str, definition: Value) -> String {
        let created = self.call(Method::POST, "/api/v1/workflows", admin, Some(json!({ "name": name })), None).await;
        assert_eq!(created.status, StatusCode::CREATED, "{}", created.body);
        let id = created.body["id"].as_str().unwrap().to_owned();
        let saved = self
            .call(
                Method::PUT,
                &format!("/api/v1/workflows/{id}/draft"),
                admin,
                Some(json!({ "definition": definition })),
                Some(1),
            )
            .await;
        assert_eq!(saved.status, StatusCode::OK, "{}", saved.body);
        let published = self.call(Method::POST, &format!("/api/v1/workflows/{id}/publish"), admin, None, Some(2)).await;
        assert_eq!(published.status, StatusCode::OK, "{}", published.body);
        id
    }
}

fn review_flow() -> Value {
    json!({
        "statuses": [
            { "key": "backlog", "name": "Backlog", "category": "todo", "x": 0, "y": 0 },
            { "key": "doing", "name": "Doing", "category": "in_progress", "x": 200, "y": 0 },
            { "key": "shipped", "name": "Shipped", "category": "done", "x": 400, "y": 0 }
        ],
        "initial": "backlog",
        "transitions": [
            { "id": "start", "name": "Start", "from": ["backlog"], "to": "doing", "post_functions": [{ "type": "assign_to_current_user" }] },
            { "id": "ship", "name": "Ship", "from": ["doing"], "to": "shipped",
              "conditions": [{ "type": "role_at_least", "role": "administrator" }],
              "validators": [{ "type": "field_required", "field": "due_date" }],
              "post_functions": [{ "type": "set_resolution", "value": "shipped" }] },
            { "id": "back", "name": "Back to backlog", "from": [], "to": "backlog" }
        ]
    })
}

#[tokio::test]
async fn draft_publish_and_version_lifecycle() {
    let h = harness().await;
    let (admin, member) = (h.token("admin"), h.token("member"));

    assert_eq!(
        h.call(Method::POST, "/api/v1/workflows", &member, Some(json!({ "name": "Nope" })), None).await.status,
        StatusCode::FORBIDDEN
    );
    let created = h.call(Method::POST, "/api/v1/workflows", &admin, Some(json!({ "name": " Delivery " })), None).await;
    assert_eq!(created.status, StatusCode::CREATED);
    assert_eq!(created.body["name"], "Delivery");
    assert_eq!(created.body["draft"]["statuses"].as_array().unwrap().len(), 4, "starts as a copy of the default workflow");
    assert!(created.body["published"].is_null() && created.body["published_version"].is_null());
    assert_eq!(created.body["problems"], json!([]));
    let id = created.body["id"].as_str().unwrap().to_owned();
    let path = format!("/api/v1/workflows/{id}");
    assert_eq!(
        h.call(Method::POST, "/api/v1/workflows", &admin, Some(json!({ "name": "delivery" })), None).await.status,
        StatusCode::CONFLICT
    );

    // Drafts may be incomplete; the reply says what blocks publishing.
    let mut broken = review_flow();
    broken["transitions"][1]["to"] = json!("nowhere");
    assert_eq!(
        h.call(Method::PUT, &format!("{path}/draft"), &admin, Some(json!({ "definition": broken })), None).await.status,
        StatusCode::PRECONDITION_REQUIRED
    );
    let saved = h.call(Method::PUT, &format!("{path}/draft"), &admin, Some(json!({ "definition": broken })), Some(1)).await;
    assert_eq!(saved.status, StatusCode::OK, "{}", saved.body);
    assert_eq!(saved.body["problems"][0]["path"], "transitions[1].to");
    let refused = h.call(Method::POST, &format!("{path}/publish"), &admin, None, Some(2)).await;
    assert_eq!(refused.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(refused.body["errors"][0]["field"], "transitions[1].to");

    // Rules outside the closed set are refused at the door, not stored.
    let mut scripted = review_flow();
    scripted["transitions"][0]["post_functions"] = json!([{ "type": "run_script", "code": "steal()" }]);
    assert!(h
        .call(Method::PUT, &format!("{path}/draft"), &admin, Some(json!({ "definition": scripted })), Some(2))
        .await
        .status
        .is_client_error());

    // Someone editing an older copy does not overwrite.
    let stale =
        h.call(Method::PUT, &format!("{path}/draft"), &admin, Some(json!({ "definition": review_flow() })), Some(1)).await;
    assert_eq!(stale.status, StatusCode::CONFLICT);
    assert_eq!(stale.body["current"]["version"], 2);

    let fixed =
        h.call(Method::PUT, &format!("{path}/draft"), &admin, Some(json!({ "definition": review_flow() })), Some(2)).await;
    assert_eq!(fixed.body["problems"], json!([]));
    assert_eq!(h.call(Method::POST, &format!("{path}/publish"), &member, None, Some(3)).await.status, StatusCode::FORBIDDEN);
    let published = h.call(Method::POST, &format!("{path}/publish"), &admin, None, Some(3)).await;
    assert_eq!(published.status, StatusCode::OK, "{}", published.body);
    assert_eq!((published.body["published_version"].as_i64(), published.body["has_draft"].as_bool()), (Some(1), Some(false)));
    assert_eq!(published.body["published"]["initial"], "backlog");
    assert_eq!(
        h.call(Method::POST, &format!("{path}/publish"), &admin, None, Some(4)).await.status,
        StatusCode::CONFLICT,
        "nothing to publish"
    );

    // A second round: the published version keeps governing until the new draft is published.
    let mut v2 = review_flow();
    v2["statuses"][1]["name"] = json!("In flight");
    h.call(Method::PUT, &format!("{path}/draft"), &admin, Some(json!({ "definition": v2 })), Some(4)).await;
    let during = h.call(Method::GET, &path, &member, None, None).await;
    assert_eq!(
        (during.body["published"]["statuses"][1]["name"].as_str(), during.body["draft"]["statuses"][1]["name"].as_str()),
        (Some("Doing"), Some("In flight"))
    );
    let discarded = h.call(Method::DELETE, &format!("{path}/draft"), &admin, None, Some(5)).await;
    assert_eq!(discarded.body["has_draft"], false);
    h.call(Method::PUT, &format!("{path}/draft"), &admin, Some(json!({ "definition": v2 })), Some(6)).await;
    assert_eq!(h.call(Method::POST, &format!("{path}/publish"), &admin, None, Some(7)).await.body["published_version"], 2);

    let versions = h.call(Method::GET, &format!("{path}/versions"), &member, None, None).await;
    let numbers: Vec<i64> = versions.body["items"].as_array().unwrap().iter().map(|v| v["version"].as_i64().unwrap()).collect();
    assert_eq!(numbers, [2, 1]);
    // Published versions are immutable history.
    let (v1_name,): (String,) =
        sqlx::query_as("SELECT definition->'statuses'->1->>'name' FROM workflow_versions WHERE version = 1")
            .fetch_one(&h.db.owner)
            .await
            .unwrap();
    assert_eq!(v1_name, "Doing");

    let copy =
        h.call(Method::POST, "/api/v1/workflows", &admin, Some(json!({ "name": "Delivery copy", "copy_from": id })), None).await;
    assert_eq!(copy.body["draft"]["statuses"][1]["name"], "In flight");
    let (events,): (i64,) = sqlx::query_as("SELECT count(*) FROM outbox WHERE event_type = 'workflow.published'")
        .fetch_one(&h.db.owner)
        .await
        .unwrap();
    assert_eq!(events, 2);
    h.db.cleanup().await;
}

#[tokio::test]
async fn assignments_resolve_from_most_to_least_specific() {
    let h = harness().await;
    let admin = h.token("admin");
    let (project, other_project) = (Uuid::now_v7(), Uuid::now_v7());
    let tenant_default = h.published(&admin, "Tenant default", review_flow()).await;
    let project_wide = h.published(&admin, "Project wide", review_flow()).await;
    let bugs_only = h.published(&admin, "Bugs", review_flow()).await;
    let assign = |project_id: Option<Uuid>, issue_type: Option<&'static str>, workflow: Option<String>| {
        let (h, admin) = (&h, admin.clone());
        async move {
            h.call(
                Method::PUT,
                "/api/v1/workflow-assignments",
                &admin,
                Some(json!({ "project_id": project_id, "issue_type": issue_type, "workflow_id": workflow })),
                None,
            )
            .await
        }
    };

    assert_eq!(h.workflow_name(project, "bug").await, "Default workflow", "nothing assigned: built-in default");
    assert_eq!(assign(None, None, Some(tenant_default.clone())).await.status, StatusCode::NO_CONTENT);
    assert_eq!(h.workflow_name(project, "bug").await, "Tenant default");
    assign(Some(project), None, Some(project_wide.clone())).await;
    assert_eq!(h.workflow_name(project, "bug").await, "Project wide");
    assign(Some(project), Some("bug"), Some(bugs_only.clone())).await;
    assert_eq!(h.workflow_name(project, "bug").await, "Bugs");
    assert_eq!(h.workflow_name(project, "task").await, "Project wide");
    assert_eq!(h.workflow_name(other_project, "bug").await, "Tenant default");

    // Re-assigning replaces; null removes and falls back.
    assign(Some(project), Some("bug"), Some(tenant_default.clone())).await;
    assert_eq!(h.workflow_name(project, "bug").await, "Tenant default");
    assign(Some(project), Some("bug"), None).await;
    assert_eq!(h.workflow_name(project, "bug").await, "Project wide");
    let listed = h.call(Method::GET, &format!("/api/v1/workflow-assignments?project_id={project}"), &admin, None, None).await;
    assert_eq!(listed.body["items"].as_array().unwrap().len(), 1);

    // Unpublished or unknown workflows cannot be assigned; a workflow in use cannot be deleted.
    let draft_only = h.call(Method::POST, "/api/v1/workflows", &admin, Some(json!({ "name": "Draft only" })), None).await.body
        ["id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(assign(Some(project), None, Some(draft_only)).await.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(assign(Some(project), None, Some(Uuid::now_v7().to_string())).await.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(assign(Some(project), Some("saga"), Some(bugs_only.clone())).await.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        h.call(Method::DELETE, &format!("/api/v1/workflows/{project_wide}"), &admin, None, None).await.status,
        StatusCode::CONFLICT
    );
    assert_eq!(
        h.call(Method::DELETE, &format!("/api/v1/workflows/{bugs_only}"), &admin, None, None).await.status,
        StatusCode::NO_CONTENT
    );

    // Status list for filters: union across what can apply to the project.
    let statuses = h.call(Method::GET, &format!("/api/v1/workflow-statuses?project_id={project}"), &admin, None, None).await;
    let keys: Vec<&str> = statuses.body["items"].as_array().unwrap().iter().map(|s| s["key"].as_str().unwrap()).collect();
    assert_eq!(keys, ["backlog", "doing", "shipped"]);
    h.db.cleanup().await;
}

#[tokio::test]
async fn project_admins_assign_for_their_project_only() {
    let h = harness().await;
    let (admin, member) = (h.token("admin"), h.token("member"));
    let (mine, theirs) = (Uuid::now_v7(), Uuid::now_v7());
    let workflow = h.published(&admin, "Flow", review_flow()).await;
    // The member administers `mine` and can only browse nothing else.
    h.policy.0.lock().unwrap().roles.insert(mine, ProjectRole::Administrator);
    let assign = |project_id: Option<Uuid>| {
        let (h, member, workflow) = (&h, member.clone(), workflow.clone());
        async move {
            h.call(
                Method::PUT,
                "/api/v1/workflow-assignments",
                &member,
                Some(json!({ "project_id": project_id, "workflow_id": workflow })),
                None,
            )
            .await
            .status
        }
    };
    assert_eq!(assign(Some(mine)).await, StatusCode::NO_CONTENT);
    assert_eq!(assign(Some(theirs)).await, StatusCode::NOT_FOUND, "a project the caller cannot see");
    assert_eq!(assign(None).await, StatusCode::FORBIDDEN, "workspace defaults are for organization admins");
    assert_eq!(
        h.call(Method::GET, &format!("/api/v1/workflow-statuses?project_id={theirs}"), &member, None, None).await.status,
        StatusCode::NOT_FOUND
    );

    h.policy.0.lock().unwrap().roles.insert(mine, ProjectRole::Member);
    let (h2, member2) = (&h, h.token("member"));
    let demoted = h2
        .call(
            Method::PUT,
            "/api/v1/workflow-assignments",
            &member2,
            Some(json!({ "project_id": mine, "workflow_id": workflow })),
            None,
        )
        .await;
    assert_eq!(demoted.status, StatusCode::FORBIDDEN);
    h.db.cleanup().await;
}

#[tokio::test]
async fn the_engine_decides_transitions_from_the_published_version() {
    let h = harness().await;
    let admin = h.token("admin");
    let project = Uuid::now_v7();
    let workflow = h.published(&admin, "Flow", review_flow()).await;
    h.call(
        Method::PUT,
        "/api/v1/workflow-assignments",
        &admin,
        Some(json!({ "project_id": project, "workflow_id": workflow })),
        None,
    )
    .await;
    let facts = |role: &str, fields: &[&str]| Facts {
        actor_role: role.into(),
        present_fields: fields.iter().map(|f| (*f).to_owned()).collect(),
        children_done: true,
        ..Facts::default()
    };
    let evaluate = |from: &'static str, to: &'static str, f: Facts| {
        let h = &h;
        async move {
            h.engine
                .evaluate_transition(tonic::Request::new(EvaluateRequest {
                    workflow: Some(h.request(project, "task")),
                    from_status: from.into(),
                    to_status: to.into(),
                    facts: Some(f),
                }))
                .await
                .unwrap()
                .into_inner()
        }
    };

    let info = h.engine.get_workflow(tonic::Request::new(h.request(project, "task"))).await.unwrap().into_inner();
    assert_eq!((info.initial_status.as_str(), info.version, info.statuses.len()), ("backlog", 1, 3));

    let start = evaluate("backlog", "doing", facts("member", &[])).await;
    assert!(start.allowed);
    assert_eq!(start.post_functions[0].r#type, "assign_to_current_user");
    assert_eq!(start.to.unwrap().category, "in_progress");

    let skip = evaluate("backlog", "shipped", facts("administrator", &["due_date"])).await;
    assert_eq!((skip.allowed, skip.denial_code.as_str()), (false, "no_transition"));
    let not_admin = evaluate("doing", "shipped", facts("member", &["due_date"])).await;
    assert_eq!(not_admin.denial_code, "condition_failed");
    let no_date = evaluate("doing", "shipped", facts("administrator", &[])).await;
    assert_eq!(
        (no_date.denial_code.as_str(), no_date.denial_message.as_str()),
        ("validator_failed", "Due date is required to move this issue to Shipped.")
    );
    let shipped = evaluate("doing", "shipped", facts("administrator", &["due_date"])).await;
    assert!(shipped.allowed);
    assert_eq!(
        (shipped.post_functions[0].r#type.as_str(), shipped.post_functions[0].value.as_str()),
        ("set_resolution", "shipped")
    );

    let options = h
        .engine
        .list_transitions(tonic::Request::new(TransitionsRequest {
            workflow: Some(h.request(project, "task")),
            from_status: "doing".into(),
            facts: Some(facts("member", &[])),
        }))
        .await
        .unwrap()
        .into_inner();
    let names: Vec<&str> = options.transitions.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, ["Back to backlog"], "Ship is hidden from non-administrators");

    // A draft changes nothing until it is published.
    let mut open = review_flow();
    open["transitions"][1]["conditions"] = json!([]);
    h.call(Method::PUT, &format!("/api/v1/workflows/{workflow}/draft"), &admin, Some(json!({ "definition": open })), Some(3))
        .await;
    assert_eq!(evaluate("doing", "shipped", facts("member", &["due_date"])).await.denial_code, "condition_failed");
    h.call(Method::POST, &format!("/api/v1/workflows/{workflow}/publish"), &admin, None, Some(4)).await;
    assert!(evaluate("doing", "shipped", facts("member", &["due_date"])).await.allowed);

    // Another tenant asking about the same project id gets the built-in default.
    let foreign =
        WorkflowRequest { tenant_id: Uuid::now_v7().to_string(), project_id: project.to_string(), issue_type: "task".into() };
    assert_eq!(h.engine.get_workflow(tonic::Request::new(foreign)).await.unwrap().into_inner().name, "Default workflow");
    let other_tenant = h.auth.org_token(Uuid::now_v7(), "o@example.com", Uuid::now_v7(), Uuid::now_v7(), "owner").unwrap();
    assert!(h.call(Method::GET, "/api/v1/workflows", &other_tenant, None, None).await.body["items"]
        .as_array()
        .unwrap()
        .is_empty());
    assert_eq!(
        h.call(Method::GET, &format!("/api/v1/workflows/{workflow}"), &other_tenant, None, None).await.status,
        StatusCode::NOT_FOUND
    );
    h.db.cleanup().await;
}
