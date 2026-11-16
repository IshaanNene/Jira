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
    notifier::{EmailSettings, Notifier},
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
    let unsubscribe = Arc::new(notification_service::unsubscribe::UnsubscribeKey::derive("test-secret-for-unsubscribe-links"));
    let state = AppState {
        db: Arc::clone(&router),
        verifier: Arc::clone(&auth.verifier),
        policy: Arc::clone(&policy),
        allowed_origins: Arc::new(vec![]),
        unsubscribe: Arc::clone(&unsubscribe),
        webhooks: Arc::new(notification_service::webhooks::Settings {
            secrets: notification_service::webhooks::SecretBox::derive("test-secret-for-webhooks"),
            reach: notification_service::webhooks::Reach { allow_http: true, allow_private: true },
        }),
        projects: Arc::new(NoProjects),
    };
    let notifier = Notifier {
        db: router,
        directory: world.clone(),
        policy,
        email: Some(EmailSettings { app_url: "https://app.example.com".into(), delay: std::time::Duration::ZERO, unsubscribe }),
    };
    Harness { db, app: http::router(state), notifier, auth, world, tenant: Uuid::now_v7() }
}

impl Harness {
    /// A workspace member; `project` access as a viewer when given.
    fn person(&self, tenant: Uuid, name: &str, project: Option<Uuid>, active: bool) -> Uuid {
        let id = Uuid::now_v7();
        self.world.people.lock().unwrap().insert(
            (tenant, id),
            Person {
                user_id: id,
                role: OrgRole::Member,
                active,
                display_name: name.into(),
                email: format!("{}-{id}@example.test", name.split(' ').next().unwrap().to_lowercase()),
            },
        );
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

    /// A request without a session, as a mail client or a signed-out browser sends it.
    async fn anonymous(&self, method: &str, path: &str, content_type: &str, body: &str) -> (u16, Value) {
        let req = Request::builder()
            .method(method)
            .uri(path)
            .header("content-type", content_type)
            .body(Body::from(body.to_owned()))
            .unwrap();
        let res = self.app.clone().oneshot(req).await.unwrap();
        let status = res.status().as_u16();
        let bytes = axum::body::to_bytes(res.into_body(), 1 << 20).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    async fn put_json(&self, path: &str, token: &str, body: Value) -> (u16, Value) {
        let req = Request::builder()
            .method("PUT")
            .uri(path)
            .header("authorization", format!("Bearer {token}"))
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        let res = self.app.clone().oneshot(req).await.unwrap();
        let status = res.status().as_u16();
        let bytes = axum::body::to_bytes(res.into_body(), 1 << 20).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    /// The email queue as (recipient, status) pairs.
    async fn deliveries(&self) -> Vec<(Uuid, String)> {
        sqlx::query_as("SELECT recipient_id, status FROM email_deliveries ORDER BY created_at")
            .fetch_all(&self.db.owner)
            .await
            .unwrap()
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

    // A description edit that adds mentions tells only the people it mentions (not followers),
    // and still nobody without access.
    h.deliver(
        t,
        "issue.mentioned",
        watcher,
        json!({ "issue_id": issue, "key": "PAY-7", "project_id": project, "mentioned_user_ids": [reporter, outsider] }),
    )
    .await;
    let (_, inbox) = h.call("GET", "/api/v1/notifications", &h.token(t, reporter)).await;
    assert_eq!(inbox["items"][0]["kind"], "mentioned");
    assert_eq!(inbox["items"][0]["detail"], json!({ "in": "description" }));
    let (_, inbox) = h.call("GET", "/api/v1/notifications", &h.token(t, outsider)).await;
    assert!(inbox["items"].as_array().unwrap().is_empty());
    let (_, inbox) = h.call("GET", "/api/v1/notifications", &h.token(t, author)).await;
    assert_eq!(inbox["items"].as_array().unwrap().len(), 1, "the assignee is not told about mentions of others");

    // Each notification announced itself for the live channel.
    let announced: i64 = sqlx::query_scalar("SELECT count(*) FROM outbox WHERE event_type = 'notification.created'")
        .fetch_one(&h.db.owner)
        .await
        .unwrap();
    assert_eq!(announced, 5);
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

fn smtp_url() -> String {
    std::env::var("TEST_SMTP_URL").unwrap_or_else(|_| "smtp://127.0.0.1:51025".to_owned())
}

fn mailpit_api() -> String {
    std::env::var("TEST_MAILPIT_URL").unwrap_or_else(|_| "http://127.0.0.1:58025".to_owned())
}

/// Messages Mailpit received for `address`.
async fn inbox_of(address: &str) -> Vec<Value> {
    let url = format!("{}/api/v1/search?query={}", mailpit_api(), urlencode(&format!("to:{address}")));
    let body: Value = reqwest::get(url).await.unwrap().json().await.unwrap();
    body["messages"].as_array().cloned().unwrap_or_default()
}

fn urlencode(value: &str) -> String {
    value
        .bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect()
}

#[tokio::test]
async fn notifications_are_emailed_unless_read_first_or_turned_off() {
    use notification_service::{mailer::SmtpMailer, sender};

    let h = harness().await;
    let t = h.tenant;
    let project = Uuid::now_v7();
    let maya = h.person(t, "Maya <b>Member</b>", Some(project), true);
    let olivia = h.person(t, "Olivia Owner", Some(project), true);
    let rae = h.person(t, "Rae Reporter", Some(project), true);
    let issue = Uuid::now_v7();
    h.world.audiences.lock().unwrap().insert(
        issue,
        Audience {
            key: "PAY-7".into(),
            summary: "Refunds <script>fail</script>".into(),
            project_id: project,
            reporter_id: rae,
            assignee_id: None,
            watcher_ids: vec![],
        },
    );
    let address = |id: Uuid| h.world.people.lock().unwrap()[&(t, id)].email.clone();

    // Rae turns off status-change email; the API reports it back.
    let (status, prefs) = h.call("GET", "/api/v1/notifications/preferences", &h.token(t, rae)).await;
    assert_eq!(
        (status, prefs["email_enabled"].as_bool(), prefs["email_status_changed"].as_bool()),
        (200, Some(true), Some(true))
    );
    let off = json!({ "email_enabled": true, "email_assigned": true, "email_mentioned": true, "email_commented": true, "email_status_changed": false, "email_filter_digest": true });
    assert_eq!(h.put_json("/api/v1/notifications/preferences", &h.token(t, rae), off.clone()).await, (200, off));

    h.deliver(
        t,
        "issue.assigned",
        maya,
        json!({ "issue_id": issue, "key": "PAY-7", "project_id": project, "assignee_id": olivia, "previous_assignee_id": null }),
    )
    .await;
    h.deliver(
        t,
        "issue.transitioned",
        maya,
        json!({ "issue_id": issue, "key": "PAY-7", "project_id": project, "from_status": "todo", "to_status": "in_review" }),
    )
    .await;
    let queued = h.deliveries().await;
    assert!(queued.contains(&(olivia, "pending".into())), "{queued:?}");
    assert!(!queued.iter().any(|(who, _)| *who == rae), "muted kind is not queued");

    let mailer = SmtpMailer::new(&smtp_url(), "Meridian <notifications@meridian.localhost>").unwrap();
    sender::run_once(&h.db.app, &mailer).await.unwrap();
    assert!(h.deliveries().await.iter().filter(|(who, _)| *who == olivia).all(|(_, s)| s == "sent"));

    let mails = inbox_of(&address(olivia)).await;
    let subjects: Vec<&str> = mails.iter().map(|m| m["Subject"].as_str().unwrap()).collect();
    assert!(subjects.contains(&"[PAY-7] Refunds <script>fail</script>"), "{subjects:?}");
    let id = mails[0]["ID"].as_str().unwrap();
    let full: Value = reqwest::get(format!("{}/api/v1/message/{id}", mailpit_api())).await.unwrap().json().await.unwrap();
    let html = full["HTML"].as_str().unwrap();
    assert!(html.contains("&lt;script&gt;") && !html.contains("<script>"), "escaped");
    assert!(full["Text"].as_str().unwrap().contains("https://app.example.com/projects/PAY/issues/PAY-7"));

    // Read in the app before the email went out: it is skipped.
    let issue2 = Uuid::now_v7();
    h.world.audiences.lock().unwrap().insert(
        issue2,
        Audience {
            key: "PAY-8".into(),
            summary: "Second".into(),
            project_id: project,
            reporter_id: rae,
            assignee_id: None,
            watcher_ids: vec![],
        },
    );
    h.deliver(
        t,
        "issue.assigned",
        maya,
        json!({ "issue_id": issue2, "key": "PAY-8", "project_id": project, "assignee_id": olivia, "previous_assignee_id": null }),
    )
    .await;
    let (_, inbox) = h.call("GET", "/api/v1/notifications?unread=true", &h.token(t, olivia)).await;
    let nid = inbox["items"][0]["id"].as_str().unwrap().to_owned();
    assert_eq!(h.call("POST", &format!("/api/v1/notifications/{nid}/read"), &h.token(t, olivia)).await.0, 204);
    sender::run_once(&h.db.app, &mailer).await.unwrap();
    assert_eq!(h.deliveries().await.last().unwrap().1, "skipped");
    assert_eq!(inbox_of(&address(olivia)).await.len(), 1);

    // Email switched off entirely: nothing is queued.
    let none = json!({ "email_enabled": false, "email_assigned": true, "email_mentioned": true, "email_commented": true, "email_status_changed": true });
    h.put_json("/api/v1/notifications/preferences", &h.token(t, olivia), none).await;
    let before = h.deliveries().await.len();
    let issue3 = Uuid::now_v7();
    h.world.audiences.lock().unwrap().insert(
        issue3,
        Audience {
            key: "PAY-9".into(),
            summary: "Third".into(),
            project_id: project,
            reporter_id: rae,
            assignee_id: None,
            watcher_ids: vec![],
        },
    );
    h.deliver(
        t,
        "issue.assigned",
        maya,
        json!({ "issue_id": issue3, "key": "PAY-9", "project_id": project, "assignee_id": olivia, "previous_assignee_id": null }),
    )
    .await;
    assert_eq!(h.deliveries().await.len(), before);
    h.db.cleanup().await;
}

#[tokio::test]
async fn an_unreachable_mail_server_is_retried_later() {
    use notification_service::{mailer::SmtpMailer, sender};

    let h = harness().await;
    let t = h.tenant;
    let project = Uuid::now_v7();
    let maya = h.person(t, "Maya", Some(project), true);
    let olivia = h.person(t, "Olivia", Some(project), true);
    let issue = Uuid::now_v7();
    h.world.audiences.lock().unwrap().insert(
        issue,
        Audience {
            key: "OPS-1".into(),
            summary: "S".into(),
            project_id: project,
            reporter_id: maya,
            assignee_id: None,
            watcher_ids: vec![],
        },
    );
    h.deliver(
        t,
        "issue.assigned",
        maya,
        json!({ "issue_id": issue, "key": "OPS-1", "project_id": project, "assignee_id": olivia, "previous_assignee_id": null }),
    )
    .await;

    // Nothing listens on port 9.
    let down = SmtpMailer::new("smtp://127.0.0.1:9", "Meridian <notifications@meridian.localhost>").unwrap();
    assert_eq!(sender::run_once(&h.db.app, &down).await.unwrap(), 1);
    let (status, attempts, later): (String, i32, bool) =
        sqlx::query_as("SELECT status, attempts, send_after > now() + interval '50 seconds' FROM email_deliveries")
            .fetch_one(&h.db.owner)
            .await
            .unwrap();
    assert_eq!((status.as_str(), attempts, later), ("pending", 1, true));
    assert_eq!(sender::run_once(&h.db.app, &down).await.unwrap(), 0, "not due yet");
    h.db.cleanup().await;
}

#[tokio::test]
async fn emails_carry_one_click_unsubscribe_links_that_only_turn_email_off() {
    use notification_service::{mailer::SmtpMailer, sender};

    let h = harness().await;
    let t = h.tenant;
    let project = Uuid::now_v7();
    let maya = h.person(t, "Maya", Some(project), true);
    let olivia = h.person(t, "Olivia", Some(project), true);
    let issue = Uuid::now_v7();
    h.world.audiences.lock().unwrap().insert(
        issue,
        Audience {
            key: "WEB-3".into(),
            summary: "Footer".into(),
            project_id: project,
            reporter_id: olivia,
            assignee_id: None,
            watcher_ids: vec![],
        },
    );
    let comment = json!({ "comment_id": Uuid::now_v7(), "issue_id": issue, "key": "WEB-3", "project_id": project, "author_id": maya, "mentioned_user_ids": [] });
    h.deliver(t, "comment.created", maya, comment).await;
    let mailer = SmtpMailer::new(&smtp_url(), "Meridian <notifications@meridian.localhost>").unwrap();
    sender::run_once(&h.db.app, &mailer).await.unwrap();

    // The delivered message has the RFC 8058 headers and a confirmation link in its body.
    let address = h.world.people.lock().unwrap()[&(t, olivia)].email.clone();
    let id = inbox_of(&address).await[0]["ID"].as_str().unwrap().to_owned();
    let headers: Value =
        reqwest::get(format!("{}/api/v1/message/{id}/headers", mailpit_api())).await.unwrap().json().await.unwrap();
    assert_eq!(headers["List-Unsubscribe-Post"][0], "List-Unsubscribe=One-Click");
    let one_click = headers["List-Unsubscribe"][0].as_str().unwrap().trim_matches(|c| c == '<' || c == '>').to_owned();
    assert!(one_click.starts_with("https://app.example.com/api/v1/notifications/unsubscribe?token="), "{one_click}");
    let full: Value = reqwest::get(format!("{}/api/v1/message/{id}", mailpit_api())).await.unwrap().json().await.unwrap();
    assert!(full["Text"].as_str().unwrap().contains("Stop emails about comments: https://app.example.com/unsubscribe?token="));
    let path = one_click.trim_start_matches("https://app.example.com");

    // Looking at the link changes nothing; it describes only the kind.
    let (status, link) = h.anonymous("GET", path, "text/plain", "").await;
    assert_eq!((status, link), (200, json!({ "kind": "commented", "kind_label": "comments" })));
    let (_, prefs) = h.call("GET", "/api/v1/notifications/preferences", &h.token(t, olivia)).await;
    assert_eq!(prefs["email_commented"], true);

    // The mail client's one-click POST turns off that kind only, without a session.
    let (status, _) = h.anonymous("POST", path, "application/x-www-form-urlencoded", "List-Unsubscribe=One-Click").await;
    assert_eq!(status, 204);
    let (_, prefs) = h.call("GET", "/api/v1/notifications/preferences", &h.token(t, olivia)).await;
    assert_eq!(
        (prefs["email_enabled"].as_bool(), prefs["email_commented"].as_bool(), prefs["email_assigned"].as_bool()),
        (Some(true), Some(false), Some(true))
    );
    // Repeating it is harmless; "all" switches every email off.
    assert_eq!(h.anonymous("POST", path, "application/json", r#"{"all": true}"#).await.0, 204);
    let (_, prefs) = h.call("GET", "/api/v1/notifications/preferences", &h.token(t, olivia)).await;
    assert_eq!(prefs["email_enabled"], false);
    assert_eq!(
        h.call("GET", "/api/v1/notifications/preferences", &h.token(t, maya)).await.1["email_enabled"],
        true,
        "nobody else"
    );

    // Tampered, foreign or missing tokens.
    let token = path.split("token=").nth(1).unwrap();
    let tampered = format!("/api/v1/notifications/unsubscribe?token={}x", token);
    assert_eq!(h.anonymous("POST", &tampered, "text/plain", "").await.0, 404);
    let foreign = notification_service::unsubscribe::UnsubscribeKey::derive("some-other-deployments-secret!!").issue(
        &notification_service::unsubscribe::Claims {
            tenant_id: t,
            user_id: maya,
            kind: "commented".into(),
            issued_at: time::OffsetDateTime::now_utc().unix_timestamp(),
        },
    );
    assert_eq!(h.anonymous("POST", &format!("/api/v1/notifications/unsubscribe?token={foreign}"), "text/plain", "").await.0, 404);
    assert_eq!(h.anonymous("POST", "/api/v1/notifications/unsubscribe", "text/plain", "").await.0, 400);
    h.db.cleanup().await;
}

/// A `filter.digest` event from search-service, as the relay publishes it.
async fn deliver_digest(h: &Harness, tenant: Uuid, recipient: Uuid, total: u64, age: time::Duration) -> Uuid {
    let event_id = Uuid::now_v7();
    let subscription = Uuid::now_v7();
    let raw = json!({
        "event_id": event_id, "event_type": "filter.digest", "schema_version": 1, "tenant_id": tenant,
        "aggregate_type": "filter_subscription", "aggregate_id": subscription, "aggregate_version": 1,
        "occurred_at": (time::OffsetDateTime::now_utc() - age).format(&time::format_description::well_known::Rfc3339).unwrap(),
        "correlation_id": null, "causation_id": null, "actor": {"type": "system", "id": "search-service"},
        "payload": {
            "subscription_id": subscription, "filter_id": Uuid::nil(), "recipient_id": recipient,
            "filter_name": "My <i>bugs</i>", "query": "assignee = currentUser()", "total": total, "total_is_lower_bound": false,
            "issues": [
                { "issue_id": Uuid::now_v7(), "key": "PAY-1", "summary": "Refunds <script>x</script>", "status": "in_review" },
                { "issue_id": Uuid::now_v7(), "key": "PAY-2", "summary": "Payouts", "status": "todo" }
            ]
        }
    });
    let event = Event::parse(topics::SEARCH, &serde_json::to_vec(&raw).unwrap()).unwrap();
    h.notifier.handle(&event).await.unwrap();
    h.notifier.handle(&event).await.unwrap();
    event_id
}

#[tokio::test]
async fn filter_digests_are_emailed_to_active_subscribers_who_want_them() {
    use notification_service::{mailer::SmtpMailer, sender};

    let h = harness().await;
    let t = h.tenant;
    let ada = h.person(t, "Ada Admin", None, true);
    let bob = h.person(t, "Bob Builder", None, true);
    let carol = h.person(t, "Carol Gone", None, false);
    let address = |id: Uuid| h.world.people.lock().unwrap()[&(t, id)].email.clone();

    // Bob turns digests off; an older client that leaves the field out keeps them on.
    let (_, prefs) = h.call("GET", "/api/v1/notifications/preferences", &h.token(t, bob)).await;
    assert_eq!(prefs["email_filter_digest"], true);
    let off = json!({ "email_enabled": true, "email_assigned": true, "email_mentioned": true, "email_commented": true, "email_status_changed": true, "email_filter_digest": false });
    assert_eq!(h.put_json("/api/v1/notifications/preferences", &h.token(t, bob), off.clone()).await, (200, off));
    let legacy = json!({ "email_enabled": true, "email_assigned": true, "email_mentioned": true, "email_commented": true, "email_status_changed": true });
    assert_eq!(h.put_json("/api/v1/notifications/preferences", &h.token(t, ada), legacy).await.1["email_filter_digest"], true);

    deliver_digest(&h, t, ada, 3, time::Duration::ZERO).await;
    deliver_digest(&h, t, bob, 3, time::Duration::ZERO).await;
    deliver_digest(&h, t, carol, 3, time::Duration::ZERO).await;
    deliver_digest(&h, t, ada, 3, time::Duration::days(2)).await;
    assert_eq!(h.deliveries().await, vec![(ada, "pending".to_owned())], "one email, to Ada only, not the stale one");

    // No in-app notification exists for a digest, and it is sent all the same.
    let (_, inbox) = h.call("GET", "/api/v1/notifications", &h.token(t, ada)).await;
    assert_eq!(inbox["items"].as_array().unwrap().len(), 0);
    let mailer = SmtpMailer::new(&smtp_url(), "Meridian <notifications@meridian.localhost>").unwrap();
    sender::run_once(&h.db.app, &mailer).await.unwrap();
    assert_eq!(h.deliveries().await, vec![(ada, "sent".to_owned())]);

    let mails = inbox_of(&address(ada)).await;
    assert_eq!(mails.len(), 1);
    assert_eq!(mails[0]["Subject"], "[My <i>bugs</i>] 3 issues");
    let id = mails[0]["ID"].as_str().unwrap();
    let full: Value = reqwest::get(format!("{}/api/v1/message/{id}", mailpit_api())).await.unwrap().json().await.unwrap();
    let html = full["HTML"].as_str().unwrap();
    assert!(html.contains("Refunds &lt;script&gt;") && !html.contains("<script>") && !html.contains("<i>bugs"), "escaped");
    let text = full["Text"].as_str().unwrap();
    assert!(text.contains("PAY-1  Refunds <script>x</script>  [In review]") && text.contains("…and 1 more"), "{text}");
    assert!(text.contains("https://app.example.com/search?mode=mql&filter="));

    // Its unsubscribe link names digests, and turns only them off.
    let one_click: String = sqlx::query_scalar("SELECT unsubscribe_url FROM email_deliveries WHERE recipient_id = $1")
        .bind(ada)
        .fetch_one(&h.db.owner)
        .await
        .unwrap();
    let path = one_click.trim_start_matches("https://app.example.com");
    let (status, link) = h.anonymous("GET", path, "text/plain", "").await;
    assert_eq!((status, link), (200, json!({ "kind": "filter_digest", "kind_label": "saved filter subscriptions" })));
    assert_eq!(h.anonymous("POST", path, "text/plain", "").await.0, 204);
    let (_, prefs) = h.call("GET", "/api/v1/notifications/preferences", &h.token(t, ada)).await;
    assert_eq!((prefs["email_filter_digest"].as_bool(), prefs["email_assigned"].as_bool()), (Some(false), Some(true)));
    deliver_digest(&h, t, ada, 3, time::Duration::ZERO).await;
    assert_eq!(h.deliveries().await.len(), 1);
    h.db.cleanup().await;
}

struct NoProjects;

#[async_trait::async_trait]
impl notification_service::projects::Projects for NoProjects {
    async fn id_of(&self, _tenant_id: Uuid, _key: &str) -> error_handling::ApiResult<Option<Uuid>> {
        Ok(None)
    }
}
