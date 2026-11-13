//! Consumer → database → REST, against a real PostgreSQL with forced RLS.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use auth_middleware::{OrgRole, PolicyClient, PolicySnapshot, PolicySource, ProjectRole, RedisHandle};
use axum::{body::Body, http::Request, Router};
use error_handling::ApiResult;
use event_bus::{Event, Handler};
use event_contracts::topics;
use notification_service::{
    directory::{Directory, Person},
    domain::Audience,
    http,
    notifier::Notifier,
    AppState, MIGRATOR,
};
use serde_json::{json, Value};
use tenant_context::DbRouter;
use test_utilities::{TestAuth, TestDb};
use tower::ServiceExt;
use uuid::Uuid;

#[derive(Default)]
struct World {
    audiences: Mutex<HashMap<Uuid, Audience>>,
    people: Mutex<HashMap<(Uuid, Uuid), Person>>,
    roles: Mutex<HashMap<(Uuid, Uuid), HashMap<Uuid, ProjectRole>>>,
}

#[async_trait]
impl Directory for World {
    async fn audience(&self, _tenant: Uuid, issue_id: Uuid) -> ApiResult<Option<Audience>> {
        Ok(self.audiences.lock().unwrap().get(&issue_id).cloned())
    }
    async fn people(&self, tenant: Uuid, ids: &[Uuid]) -> ApiResult<Vec<Person>> {
        let people = self.people.lock().unwrap();
        Ok(ids.iter().filter_map(|id| people.get(&(tenant, *id)).cloned()).collect())
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
    notifier: Notifier,
    auth: TestAuth,
    world: Arc<World>,
    tenant: Uuid,
}

async fn harness() -> Harness {
    let db = TestDb::create(&MIGRATOR).await.expect("test database");
    let world = Arc::new(World::default());
    let router = Arc::new(DbRouter::single(db.app.clone()));
    let policy = Arc::new(PolicyClient::new(world.clone(), RedisHandle::default()));
    let auth = TestAuth::new().unwrap();
    let state = AppState {
        db: Arc::clone(&router),
        verifier: Arc::clone(&auth.verifier),
        policy: Arc::clone(&policy),
        allowed_origins: Arc::new(vec![]),
    };
    let notifier = Notifier { db: router, directory: world.clone(), policy };
    Harness { db, app: http::router(state), notifier, auth, world, tenant: Uuid::now_v7() }
}

impl Harness {
    /// A workspace member; `project` access as a viewer when given.
    fn person(&self, tenant: Uuid, name: &str, project: Option<Uuid>, active: bool) -> Uuid {
        let id = Uuid::now_v7();
        self.world
            .people
            .lock()
            .unwrap()
            .insert((tenant, id), Person { user_id: id, role: OrgRole::Member, active, display_name: name.into() });
        if let Some(project) = project {
            self.world.roles.lock().unwrap().insert((tenant, id), HashMap::from([(project, ProjectRole::Viewer)]));
        }
        id
    }

    fn token(&self, tenant: Uuid, user: Uuid) -> String {
        self.auth.org_token(user, "x@example.com", tenant, Uuid::now_v7(), "member").unwrap()
    }

    async fn deliver(&self, tenant: Uuid, event_type: &str, actor: Uuid, payload: Value) -> Uuid {
        let event_id = Uuid::now_v7();
        let raw = json!({
            "event_id": event_id, "event_type": event_type, "schema_version": 1, "tenant_id": tenant,
            "aggregate_type": "issue", "aggregate_id": payload["issue_id"], "aggregate_version": 2,
            "occurred_at": time::OffsetDateTime::now_utc().format(&time::format_description::well_known::Rfc3339).unwrap(),
            "correlation_id": null, "causation_id": null, "actor": {"type": "user", "id": actor}, "payload": payload
        });
        let event = Event::parse(topics::ISSUE, &serde_json::to_vec(&raw).unwrap()).unwrap();
        self.notifier.handle(&event).await.unwrap();
        // Redelivery changes nothing.
        self.notifier.handle(&event).await.unwrap();
        event_id
    }

    async fn call(&self, method: &str, path: &str, token: &str) -> (u16, Value) {
        let req = Request::builder()
            .method(method)
            .uri(path)
            .header("authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let res = self.app.clone().oneshot(req).await.unwrap();
        let status = res.status().as_u16();
        let bytes = axum::body::to_bytes(res.into_body(), 1 << 20).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }
}

#[tokio::test]
async fn activity_notifies_the_right_people_once_and_only_while_they_can_see_the_issue() {
    let h = harness().await;
    let t = h.tenant;
    let project = Uuid::now_v7();
    let author = h.person(t, "Ada Author", Some(project), true);
    let reporter = h.person(t, "Rae Reporter", Some(project), true);
    let watcher = h.person(t, "Wes Watcher", Some(project), true);
    let outsider = h.person(t, "Otto Outsider", None, true); // member without access to the project
    let suspended = h.person(t, "Sue Suspended", Some(project), false);
    let issue = Uuid::now_v7();
    h.world.audiences.lock().unwrap().insert(
        issue,
        Audience {
            key: "PAY-7".into(),
            summary: "Refunds fail".into(),
            project_id: project,
            reporter_id: reporter,
            assignee_id: Some(author),
            watcher_ids: vec![watcher, suspended],
        },
    );

    // A comment by the assignee that mentions the watcher and someone without access.
    let comment = Uuid::now_v7();
    h.deliver(
        t,
        "comment.created",
        author,
        json!({ "comment_id": comment, "issue_id": issue, "key": "PAY-7", "project_id": project, "author_id": author,
                "mentioned_user_ids": [watcher, outsider] }),
    )
    .await;
    h.deliver(
        t,
        "issue.transitioned",
        watcher,
        json!({ "issue_id": issue, "key": "PAY-7", "project_id": project, "from_status": "todo", "to_status": "done" }),
    )
    .await;

    let (status, inbox) = h.call("GET", "/api/v1/notifications", &h.token(t, watcher)).await;
    assert_eq!(status, 200);
    let kinds: Vec<&str> = inbox["items"].as_array().unwrap().iter().map(|n| n["kind"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["mentioned"], "mentioned once; their own status change is not news to them");
    assert_eq!(inbox["items"][0]["actor_name"], "Ada Author");
    assert_eq!(inbox["items"][0]["issue_key"], "PAY-7");
    assert_eq!(inbox["unread_count"], 1);

    let (_, inbox) = h.call("GET", "/api/v1/notifications", &h.token(t, reporter)).await;
    let kinds: Vec<&str> = inbox["items"].as_array().unwrap().iter().map(|n| n["kind"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["status_changed", "commented"], "newest first");
    assert_eq!(inbox["items"][0]["detail"]["to_status"], "done");

    for (who, name) in [(outsider, "no project access"), (suspended, "suspended"), (author, "own comment")] {
        let (_, inbox) = h.call("GET", "/api/v1/notifications", &h.token(t, who)).await;
        let kinds: Vec<&str> = inbox["items"].as_array().unwrap().iter().map(|n| n["kind"].as_str().unwrap()).collect();
        if who == author {
            assert_eq!(kinds, ["status_changed"], "{name}");
        } else {
            assert!(kinds.is_empty(), "{name}: {kinds:?}");
        }
    }

    // Each notification announced itself for the live channel.
    let announced: i64 = sqlx::query_scalar("SELECT count(*) FROM outbox WHERE event_type = 'notification.created'")
        .fetch_one(&h.db.owner)
        .await
        .unwrap();
    assert_eq!(announced, 4);
    h.db.cleanup().await;
}

#[tokio::test]
async fn people_read_only_their_own_notifications_in_their_own_workspace() {
    let h = harness().await;
    let t = h.tenant;
    let project = Uuid::now_v7();
    let assigner = h.person(t, "Ann", Some(project), true);
    let me = h.person(t, "Me", Some(project), true);
    let issue = Uuid::now_v7();
    h.world.audiences.lock().unwrap().insert(
        issue,
        Audience {
            key: "PAY-1".into(),
            summary: "S".into(),
            project_id: project,
            reporter_id: assigner,
            assignee_id: None,
            watcher_ids: vec![],
        },
    );
    h.deliver(
        t,
        "issue.assigned",
        assigner,
        json!({ "issue_id": issue, "key": "PAY-1", "project_id": project, "assignee_id": me, "previous_assignee_id": null }),
    )
    .await;

    let mine = h.token(t, me);
    let (_, inbox) = h.call("GET", "/api/v1/notifications", &mine).await;
    let id = inbox["items"][0]["id"].as_str().unwrap().to_owned();
    assert_eq!(inbox["items"][0]["kind"], "assigned");

    // Someone else cannot mark it read; the same user id in another workspace sees nothing.
    let (status, _) = h.call("POST", &format!("/api/v1/notifications/{id}/read"), &h.token(t, assigner)).await;
    assert_eq!(status, 404);
    let other_tenant = h.token(Uuid::now_v7(), me);
    let (status, inbox) = h.call("GET", "/api/v1/notifications", &other_tenant).await;
    assert_eq!((status, inbox["items"].as_array().unwrap().len()), (200, 0));

    let (status, _) = h.call("POST", &format!("/api/v1/notifications/{id}/read"), &mine).await;
    assert_eq!(status, 204);
    let (_, count) = h.call("GET", "/api/v1/notifications/unread-count", &mine).await;
    assert_eq!(count["count"], 0);
    let (_, unread) = h.call("GET", "/api/v1/notifications?unread=true", &mine).await;
    assert!(unread["items"].as_array().unwrap().is_empty());

    // Losing access to the project hides its notifications.
    h.world.roles.lock().unwrap().remove(&(t, me));
    h.notifier.policy.forget(t, me);
    let (_, inbox) = h.call("GET", "/api/v1/notifications", &mine).await;
    assert!(inbox["items"].as_array().unwrap().is_empty());

    // The database itself refuses rows of another tenant to the runtime role.
    let visible: i64 = sqlx::query_scalar("SELECT count(*) FROM notifications").fetch_one(&h.db.app).await.unwrap();
    assert_eq!(visible, 0, "no tenant set, no rows");
    h.db.cleanup().await;
}
