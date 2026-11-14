//! Ingestion, chart queries and dashboards against the local ClickHouse and PostgreSQL
//! (`make infra`), with issue-service and permission-service replaced by in-memory fakes.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use analytics_service::{
    clickhouse::{self, ClickHouse, Params},
    http,
    ingest::Ingester,
    source::Source,
    AppState, MIGRATOR,
};
use api_contracts::issue::v1::Document;
use async_trait::async_trait;
use auth_middleware::{PolicyClient, PolicySnapshot, PolicySource, ProjectRole, RedisHandle};
use axum::{
    body::Body,
    http::{Method, Request, StatusCode},
    Router,
};
use error_handling::ApiResult;
use event_bus::{Event, Handler};
use event_contracts::topics;
use serde_json::{json, Value};
use tenant_context::DbRouter;
use test_utilities::{TestAuth, TestDb};
use time::{format_description::well_known::Rfc3339, Duration, OffsetDateTime};
use tower::ServiceExt;
use uuid::Uuid;

fn settings(database: &str) -> clickhouse::Settings {
    clickhouse::Settings {
        url: std::env::var("TEST_CLICKHOUSE_URL").unwrap_or_else(|_| "http://127.0.0.1:58123".to_owned()),
        database: database.to_owned(),
    }
}

fn user(database: &str, name: &str) -> ClickHouse {
    ClickHouse::new(&settings(database), name, name).unwrap()
}

#[derive(Default)]
struct Issues(Mutex<HashMap<(Uuid, Uuid), Document>>);

#[async_trait]
impl Source for Issues {
    async fn document(&self, tenant_id: Uuid, issue_id: Uuid) -> ApiResult<Option<Document>> {
        Ok(self.0.lock().unwrap().get(&(tenant_id, issue_id)).cloned())
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
    ingester: Ingester,
    issues: Arc<Issues>,
    roles: Arc<Roles>,
    auth: TestAuth,
    db: TestDb,
    owner: ClickHouse,
    database: String,
}

fn ts(t: OffsetDateTime) -> String {
    t.format(&Rfc3339).unwrap()
}

async fn harness() -> Harness {
    let database = format!("test_analytics_{}", Uuid::now_v7().simple());
    let admin = user("default", "analytics_owner");
    admin.execute(&format!("CREATE DATABASE {database}")).await.expect("ClickHouse reachable (make infra)");
    let owner = user(&database, "analytics_owner");
    clickhouse::migrate(&owner, "analytics_reader", "analytics_writer").await.unwrap();

    let db = TestDb::create(&MIGRATOR).await.expect("test database");
    let issues = Arc::new(Issues::default());
    let roles = Arc::new(Roles::default());
    let auth = TestAuth::new().unwrap();
    let state = AppState {
        db: Arc::new(DbRouter::single(db.app.clone())),
        reader: user(&database, "analytics_reader"),
        verifier: Arc::clone(&auth.verifier),
        policy: Arc::new(PolicyClient::new(roles.clone(), RedisHandle::default())),
        allowed_origins: Arc::new(vec![]),
    };
    Harness {
        app: http::router(state),
        ingester: Ingester { store: user(&database, "analytics_writer"), source: issues.clone() },
        issues,
        roles,
        auth,
        db,
        owner,
        database,
    }
}

impl Harness {
    async fn cleanup(self) {
        let _ = self.owner.execute(&format!("DROP DATABASE {}", self.database)).await;
        self.db.cleanup().await;
    }

    #[allow(clippy::too_many_arguments)]
    fn put(
        &self,
        tenant: Uuid,
        id: Uuid,
        project: Uuid,
        status: &str,
        category: &str,
        kind: &str,
        created: OffsetDateTime,
        resolved: Option<OffsetDateTime>,
        version: i64,
    ) {
        let doc = Document {
            id: id.to_string(),
            key: "X-1".into(),
            project_id: project.to_string(),
            project_key: "X".into(),
            r#type: kind.into(),
            status: status.into(),
            status_category: category.into(),
            priority: "medium".into(),
            assignee_id: String::new(),
            reporter_id: Uuid::now_v7().to_string(),
            created_at: ts(created),
            updated_at: ts(created),
            version,
            resolved_at: resolved.map(ts).unwrap_or_default(),
            ..Document::default()
        };
        self.issues.0.lock().unwrap().insert((tenant, id), doc);
    }

    async fn event(&self, tenant: Uuid, topic: &str, event_type: &str, at: OffsetDateTime, payload: Value) -> Uuid {
        let id = Uuid::now_v7();
        self.replay(id, tenant, topic, event_type, at, payload).await;
        id
    }

    async fn replay(&self, id: Uuid, tenant: Uuid, topic: &str, event_type: &str, at: OffsetDateTime, payload: Value) {
        let raw = json!({
            "event_id": id, "event_type": event_type, "schema_version": 1, "tenant_id": tenant,
            "aggregate_type": "issue", "aggregate_id": Uuid::now_v7(), "aggregate_version": 1,
            "occurred_at": ts(at), "correlation_id": null, "causation_id": null,
            "actor": {"type": "system", "id": "test"}, "payload": payload
        });
        self.ingester.handle(&Event::parse(topic, &serde_json::to_vec(&raw).unwrap()).unwrap()).await.unwrap();
    }

    async fn call(&self, method: Method, path: &str, token: &str, body: Option<Value>) -> (StatusCode, Value) {
        let mut req = Request::builder().method(method).uri(path).header("authorization", format!("Bearer {token}"));
        if body.is_some() {
            req = req.header("content-type", "application/json");
        }
        let req = req.body(body.map_or_else(Body::empty, |b| Body::from(b.to_string()))).unwrap();
        let response = self.app.clone().oneshot(req).await.unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 1 << 20).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    async fn chart(&self, token: &str, query: Value) -> Value {
        let (status, body) = self.call(Method::POST, "/api/v1/analytics/query", token, Some(query)).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body
    }
}

#[tokio::test]
async fn charts_count_only_the_viewers_tenant_and_projects() {
    let h = harness().await;
    let (tenant, other_tenant) = (Uuid::now_v7(), Uuid::now_v7());
    let (pay, hr, elsewhere) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let org = Uuid::now_v7();
    let (admin, member) = (Uuid::now_v7(), Uuid::now_v7());
    let admin_token = h.auth.org_token(admin, "a@example.com", tenant, org, "admin").unwrap();
    let member_token = h.auth.org_token(member, "m@example.com", tenant, org, "member").unwrap();
    h.roles.0.lock().unwrap().insert((tenant, member), HashMap::from([(pay, ProjectRole::Member)]));

    let now = OffsetDateTime::now_utc();
    let days_ago = |d: i64| now - Duration::days(d);
    // PAY: one story done after 3 days in progress, one bug still in progress. HR: one task to do.
    let (done, doing, todo) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    h.put(tenant, done, pay, "done", "done", "story", days_ago(10), Some(days_ago(2)), 3);
    h.put(tenant, doing, pay, "in_progress", "in_progress", "bug", days_ago(4), None, 2);
    h.put(tenant, todo, hr, "todo", "todo", "task", days_ago(1), None, 1);
    let foreign = Uuid::now_v7();
    h.put(other_tenant, foreign, elsewhere, "todo", "todo", "task", days_ago(1), None, 1);

    let created = |id: Uuid, project: Uuid| json!({ "issue_id": id, "key": "X-1", "project_id": project, "issue_type": "task", "summary": "s", "reporter_id": Uuid::now_v7(), "assignee_id": null });
    let first = h.event(tenant, topics::ISSUE, "issue.created", days_ago(10), created(done, pay)).await;
    h.event(tenant, topics::ISSUE, "issue.created", days_ago(4), created(doing, pay)).await;
    h.event(tenant, topics::ISSUE, "issue.created", days_ago(1), created(todo, hr)).await;
    h.event(other_tenant, topics::ISSUE, "issue.created", days_ago(1), created(foreign, elsewhere)).await;
    let moved = |id: Uuid, from: &str, to: &str, from_c: Option<&str>, to_c: Option<&str>| json!({ "issue_id": id, "key": "X-1", "project_id": pay, "from_status": from, "to_status": to, "from_category": from_c, "to_category": to_c });
    h.event(
        tenant,
        topics::ISSUE,
        "issue.transitioned",
        days_ago(5),
        moved(done, "todo", "in_progress", Some("todo"), Some("in_progress")),
    )
    .await;
    h.event(
        tenant,
        topics::ISSUE,
        "issue.transitioned",
        days_ago(2),
        moved(done, "in_progress", "done", Some("in_progress"), Some("done")),
    )
    .await;
    // An event from before categories were sent: the current state fills it in.
    h.event(tenant, topics::ISSUE, "issue.transitioned", days_ago(3), moved(doing, "todo", "in_progress", None, None)).await;
    // Redelivery changes nothing.
    h.replay(first, tenant, topics::ISSUE, "issue.created", days_ago(10), created(done, pay)).await;

    let cvr = h.chart(&admin_token, json!({ "kind": "created_vs_resolved", "days": 14 })).await;
    let days = cvr["days"].as_array().unwrap();
    assert_eq!(days.len(), 14);
    let total = |field: &str| days.iter().map(|d| d[field].as_u64().unwrap()).sum::<u64>();
    assert_eq!((total("created"), total("resolved")), (3, 1), "{cvr}");

    // The member only browses PAY: HR's issue is not counted, even when asked for.
    let mine = h.chart(&member_token, json!({ "kind": "created_vs_resolved", "days": 14 })).await;
    assert_eq!(mine["days"].as_array().unwrap().iter().map(|d| d["created"].as_u64().unwrap()).sum::<u64>(), 2);
    let hr_only =
        h.chart(&member_token, json!({ "kind": "breakdown", "projects": [hr], "group_by": "type", "include_done": true })).await;
    assert_eq!(hr_only["total"], 0);

    let by_category = h.chart(&admin_token, json!({ "kind": "breakdown", "group_by": "status_category" })).await;
    assert_eq!(by_category["total"], 2, "done issues are left out unless asked: {by_category}");
    let keys: Vec<&str> = by_category["groups"].as_array().unwrap().iter().map(|g| g["key"].as_str().unwrap()).collect();
    assert_eq!(keys, ["in_progress", "todo"]);

    let cycle = h.chart(&admin_token, json!({ "kind": "cycle_time", "weeks": 4 })).await;
    let weeks = cycle["weeks"].as_array().unwrap();
    assert_eq!(weeks.len(), 1, "{cycle}");
    assert_eq!((weeks[0]["issues"].as_u64(), weeks[0]["median_days"].as_f64()), (Some(1), Some(3.0)));
    let (category,): (String,) = {
        #[derive(serde::Deserialize)]
        struct Row {
            to_category: String,
        }
        let rows: Vec<Row> = h
            .owner
            .query(
                tenant,
                // The transition only: the issue's created row has no category, and without a
                // filter the order of the two rows is up to ClickHouse.
                "SELECT to_category FROM issue_events FINAL \
                 WHERE issue_id = {issue:UUID} AND event_type = 'issue.transitioned'",
                &Params::new().add("issue", doing),
            )
            .await
            .unwrap();
        (rows[0].to_category.clone(),)
    };
    assert_eq!(category, "in_progress");

    // A deleted issue drops out; so does every issue of a deleted project.
    h.event(tenant, topics::ISSUE, "issue.deleted", now, json!({ "issue_id": doing, "key": "X-2", "project_id": pay })).await;
    let after = h.chart(&admin_token, json!({ "kind": "breakdown", "group_by": "project", "include_done": true })).await;
    assert_eq!(after["total"], 2, "{after}");
    h.event(tenant, topics::PROJECT, "project.deleted", now, json!({ "project_id": hr, "key": "HR" })).await;
    let after = h.chart(&admin_token, json!({ "kind": "breakdown", "group_by": "project", "include_done": true })).await;
    assert_eq!(after["groups"], json!([{ "key": pay.to_string(), "count": 1 }]));

    // Velocity: sprints in the order they finished.
    for (n, (done_count, left)) in [(5, 1), (7, 0)].into_iter().enumerate() {
        let payload = json!({ "sprint_id": Uuid::now_v7(), "board_id": Uuid::now_v7(), "project_id": pay, "name": format!("Sprint {}", n + 1),
            "completed_issues": done_count, "incomplete_issues": left, "moved_to_sprint_id": null });
        h.event(tenant, topics::AGILE, "sprint.completed", days_ago(20 - 7 * n as i64), payload).await;
    }
    let velocity = h.chart(&member_token, json!({ "kind": "velocity", "project": pay, "sprints": 5 })).await;
    let sprints: Vec<(String, u64)> = velocity["sprints"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| (s["name"].as_str().unwrap().to_owned(), s["completed"].as_u64().unwrap()))
        .collect();
    assert_eq!(sprints, [("Sprint 1".to_owned(), 5), ("Sprint 2".to_owned(), 7)]);
    let hidden = h.chart(&member_token, json!({ "kind": "velocity", "project": hr, "sprints": 5 })).await;
    assert_eq!(hidden["sprints"], json!([]));

    // Bad options are refused before any query runs.
    let (status, body) = h
        .call(Method::POST, "/api/v1/analytics/query", &admin_token, Some(json!({ "kind": "created_vs_resolved", "days": 9999 })))
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["errors"][0]["field"], "days");

    // The reader itself cannot see another tenant's rows, whatever the SQL says, and refuses
    // to run without a tenant at all.
    #[derive(serde::Deserialize)]
    struct Count {
        n: u64,
    }
    let reader = user(&h.database, "analytics_reader");
    let sneaky: Vec<Count> = reader
        .query(
            tenant,
            "SELECT count() AS n FROM issue_state FINAL WHERE tenant_id = {other:UUID}",
            &Params::new().add("other", other_tenant),
        )
        .await
        .unwrap();
    assert_eq!(sneaky[0].n, 0);
    let theirs: Vec<Count> =
        reader.query(other_tenant, "SELECT count() AS n FROM issue_state FINAL", &Params::new()).await.unwrap();
    assert_eq!(theirs[0].n, 1);
    assert!(reader.execute("SELECT count() FROM issue_state").await.is_err(), "no tenant setting, no rows");
    assert!(reader.execute("INSERT INTO issue_events (tenant_id) VALUES (generateUUIDv4())").await.is_err(), "read-only");
    h.cleanup().await;
}

#[tokio::test]
async fn dashboards_are_private_until_shared_and_only_their_owner_or_an_admin_edits_them() {
    let h = harness().await;
    let (tenant, org) = (Uuid::now_v7(), Uuid::now_v7());
    let (ada, ben, olive) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let ada_t = h.auth.org_token(ada, "ada@example.com", tenant, org, "member").unwrap();
    let ben_t = h.auth.org_token(ben, "ben@example.com", tenant, org, "member").unwrap();
    let admin_t = h.auth.org_token(olive, "olive@example.com", tenant, org, "admin").unwrap();
    let outsider_t = h.auth.org_token(Uuid::now_v7(), "x@example.com", Uuid::now_v7(), Uuid::now_v7(), "owner").unwrap();

    let (status, bad) = h.call(Method::POST, "/api/v1/dashboards", &ada_t, Some(json!({ "name": "  " }))).await;
    assert_eq!((status, bad["errors"][0]["field"].as_str()), (StatusCode::UNPROCESSABLE_ENTITY, Some("name")));
    let (status, created) = h.call(Method::POST, "/api/v1/dashboards", &ada_t, Some(json!({ "name": "Team health" }))).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let id = created["id"].as_str().unwrap().to_owned();
    let path = format!("/api/v1/dashboards/{id}");
    assert_eq!(created["editable"], true);

    // Private: nobody else sees it, not even an admin, and certainly not another workspace.
    for token in [&ben_t, &admin_t, &outsider_t] {
        assert_eq!(h.call(Method::GET, &path, token, None).await.0, StatusCode::NOT_FOUND);
    }
    let (_, list) = h.call(Method::GET, "/api/v1/dashboards", &ben_t, None).await;
    assert_eq!(list["items"], json!([]));

    let widgets = format!("{path}/widgets");
    let (status, _) = h
        .call(Method::POST, &widgets, &ada_t, Some(json!({ "title": "Raw", "query": { "kind": "sql", "text": "SELECT 1" } })))
        .await;
    assert!(status.is_client_error());
    let (status, _) = h
        .call(
            Method::POST,
            &widgets,
            &ada_t,
            Some(json!({ "title": "Too long", "query": { "kind": "cycle_time", "weeks": 99 } })),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (_, first) = h
        .call(
            Method::POST,
            &widgets,
            &ada_t,
            Some(
                json!({ "title": "Created vs resolved", "size": "full", "query": { "kind": "created_vs_resolved", "days": 30 } }),
            ),
        )
        .await;
    let (_, second) = h
        .call(
            Method::POST,
            &widgets,
            &ada_t,
            Some(json!({ "title": "Open by type", "query": { "kind": "breakdown", "group_by": "type" } })),
        )
        .await;
    let (first, second) = (first["id"].as_str().unwrap().to_owned(), second["id"].as_str().unwrap().to_owned());

    let (status, shared) =
        h.call(Method::PATCH, &path, &ada_t, Some(json!({ "shared": true, "widget_order": [second, first] }))).await;
    assert_eq!(status, StatusCode::OK, "{shared}");
    let titles: Vec<&str> = shared["widgets"].as_array().unwrap().iter().map(|w| w["title"].as_str().unwrap()).collect();
    assert_eq!(titles, ["Open by type", "Created vs resolved"]);
    let (status, _) = h.call(Method::PATCH, &path, &ada_t, Some(json!({ "widget_order": [first] }))).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "every widget, exactly once");

    // Shared: everyone sees it; a member who doesn't own it can't change it; an admin can,
    // but only the owner decides who sees it.
    let (status, seen) = h.call(Method::GET, &path, &ben_t, None).await;
    assert_eq!((status, seen["editable"].as_bool()), (StatusCode::OK, Some(false)));
    assert_eq!(h.call(Method::PATCH, &path, &ben_t, Some(json!({ "name": "Mine now" }))).await.0, StatusCode::FORBIDDEN);
    assert_eq!(h.call(Method::DELETE, &format!("{widgets}/{first}"), &ben_t, None).await.0, StatusCode::FORBIDDEN);
    let (status, renamed) = h.call(Method::PATCH, &path, &admin_t, Some(json!({ "name": "Delivery health" }))).await;
    assert_eq!((status, renamed["name"].as_str()), (StatusCode::OK, Some("Delivery health")));
    assert_eq!(h.call(Method::PATCH, &path, &admin_t, Some(json!({ "shared": false }))).await.0, StatusCode::FORBIDDEN);

    let (status, edited) = h
        .call(
            Method::PATCH,
            &format!("{widgets}/{first}"),
            &ada_t,
            Some(json!({ "query": { "kind": "created_vs_resolved", "days": 90 } })),
        )
        .await;
    assert_eq!((status, edited["query"]["days"].as_u64()), (StatusCode::OK, Some(90)));
    assert_eq!(h.call(Method::DELETE, &format!("{widgets}/{first}"), &ada_t, None).await.0, StatusCode::NO_CONTENT);
    assert_eq!(h.call(Method::DELETE, &format!("{widgets}/{first}"), &ada_t, None).await.0, StatusCode::NOT_FOUND);

    let (_, list) = h.call(Method::GET, "/api/v1/dashboards", &ben_t, None).await;
    assert_eq!((list["items"][0]["widget_count"].as_i64(), list["items"][0]["editable"].as_bool()), (Some(1), Some(false)));
    assert_eq!(h.call(Method::DELETE, &path, &ben_t, None).await.0, StatusCode::FORBIDDEN);
    assert_eq!(h.call(Method::DELETE, &path, &ada_t, None).await.0, StatusCode::NO_CONTENT);
    assert_eq!(h.call(Method::GET, &path, &ada_t, None).await.0, StatusCode::NOT_FOUND);
    h.cleanup().await;
}
