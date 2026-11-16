//! Integrations end to end: Slack channels (outgoing) and GitHub repositories (incoming).

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use auth_middleware::{PolicyClient, PolicySnapshot, PolicySource, ProjectRole, RedisHandle};
use axum::{
    body::{Body, Bytes},
    extract::State,
    http::{HeaderMap, Request, StatusCode},
    routing::post,
    Router,
};
use error_handling::ApiResult;
use event_bus::{Event, Handler};
use event_contracts::topics;
use notification_service::{
    http,
    projects::Projects,
    source, webhook_sender,
    webhooks::{Dispatcher, Reach, SecretBox, Settings},
    AppState, MIGRATOR,
};
use serde_json::{json, Value};
use tenant_context::DbRouter;
use test_utilities::{TestAuth, TestDb};
use tower::ServiceExt;
use uuid::Uuid;

const SECRET: &str = "test-secret-for-integrations";
const DEV: Reach = Reach { allow_http: true, allow_private: true };

#[derive(Default)]
struct World {
    projects: Mutex<HashMap<(Uuid, String), Uuid>>,
    roles: Mutex<HashMap<(Uuid, Uuid), HashMap<Uuid, ProjectRole>>>,
}

#[async_trait::async_trait]
impl Projects for World {
    async fn id_of(&self, tenant_id: Uuid, key: &str) -> ApiResult<Option<Uuid>> {
        Ok(self.projects.lock().unwrap().get(&(tenant_id, key.to_owned())).copied())
    }
}

#[async_trait::async_trait]
impl PolicySource for World {
    async fn fetch(&self, tenant_id: Uuid, user_id: Uuid) -> ApiResult<PolicySnapshot> {
        Ok(PolicySnapshot {
            version: 0,
            roles: self.roles.lock().unwrap().get(&(tenant_id, user_id)).cloned().unwrap_or_default(),
            default_roles: HashMap::new(),
        })
    }
}

#[derive(Default)]
struct Receiver {
    status: Mutex<u16>,
    got: Mutex<Vec<(HeaderMap, Bytes)>>,
}

async fn receive(State(r): State<Arc<Receiver>>, headers: HeaderMap, body: Bytes) -> StatusCode {
    r.got.lock().unwrap().push((headers, body));
    StatusCode::from_u16(*r.status.lock().unwrap()).unwrap()
}

struct Harness {
    db: TestDb,
    app: Router,
    strict: Router,
    dispatcher: Dispatcher,
    auth: TestAuth,
    world: Arc<World>,
    receiver: Arc<Receiver>,
    endpoint: String,
}

async fn harness() -> Harness {
    let db = TestDb::create(&MIGRATOR).await.expect("test database");
    let auth = TestAuth::new().unwrap();
    let world = Arc::new(World::default());
    let state = |reach| AppState {
        db: Arc::new(DbRouter::single(db.app.clone())),
        verifier: Arc::clone(&auth.verifier),
        policy: Arc::new(PolicyClient::new(world.clone(), RedisHandle::default())),
        allowed_origins: Arc::new(vec![]),
        unsubscribe: Arc::new(notification_service::unsubscribe::UnsubscribeKey::derive(SECRET)),
        webhooks: Arc::new(Settings { secrets: SecretBox::derive(SECRET), reach }),
        projects: world.clone(),
    };
    let receiver = Arc::new(Receiver { status: Mutex::new(200), ..Default::default() });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/slack", listener.local_addr().unwrap());
    let server = Router::new().route("/slack", post(receive)).with_state(Arc::clone(&receiver));
    tokio::spawn(async move { axum::serve(listener, server).await.unwrap() });
    Harness {
        app: http::router(state(DEV)),
        strict: http::router(state(Reach::default())),
        dispatcher: Dispatcher {
            db: Arc::new(DbRouter::single(db.app.clone())),
            app_url: Some("https://meridian.example".into()),
        },
        db,
        auth,
        world,
        receiver,
        endpoint,
    }
}

impl Harness {
    /// A member of `tenant` with `role`, holding `projects` roles. Returns (id, token).
    fn person(&self, tenant: Uuid, role: &str, projects: &[(Uuid, ProjectRole)]) -> (Uuid, String) {
        let id = Uuid::now_v7();
        self.world.roles.lock().unwrap().insert((tenant, id), projects.iter().copied().collect());
        (id, self.auth.org_token(id, "x@example.com", tenant, Uuid::now_v7(), role).unwrap())
    }

    async fn send(&self, app: &Router, req: Request<Body>) -> (u16, Value) {
        let res = app.clone().oneshot(req).await.unwrap();
        let status = res.status().as_u16();
        let bytes = axum::body::to_bytes(res.into_body(), 1 << 20).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    async fn on(&self, app: &Router, method: &str, path: &str, token: &str, body: Option<Value>) -> (u16, Value) {
        let mut req = Request::builder().method(method).uri(path).header("authorization", format!("Bearer {token}"));
        if body.is_some() {
            req = req.header("content-type", "application/json");
        }
        self.send(app, req.body(body.map_or_else(Body::empty, |b| Body::from(b.to_string()))).unwrap()).await
    }

    async fn call(&self, method: &str, path: &str, token: &str, body: Option<Value>) -> (u16, Value) {
        self.on(&self.app, method, path, token, body).await
    }

    /// A delivery as GitHub sends it, signed with `secret` (or unsigned).
    async fn github(&self, id: &str, event: &str, body: &Value, secret: Option<&str>) -> (u16, Value) {
        let bytes = serde_json::to_vec(body).unwrap();
        let mut req = Request::builder()
            .method("POST")
            .uri(format!("/hooks/github/{id}"))
            .header("content-type", "application/json")
            .header("x-github-event", event);
        if let Some(secret) = secret {
            req = req.header("x-hub-signature-256", source::sign(secret, &bytes));
        }
        self.send(&self.app, req.body(Body::from(bytes)).unwrap()).await
    }

    async fn event(&self, tenant: Uuid, event_type: &str, payload: Value) {
        let raw = json!({
            "event_id": Uuid::now_v7(), "event_type": event_type, "schema_version": 1, "tenant_id": tenant,
            "aggregate_type": "issue", "aggregate_id": payload["issue_id"], "aggregate_version": 2,
            "occurred_at": time::OffsetDateTime::now_utc().format(&time::format_description::well_known::Rfc3339).unwrap(),
            "correlation_id": null, "causation_id": null, "actor": {"type": "user", "id": Uuid::now_v7()}, "payload": payload
        });
        let event = Event::parse(topics::ISSUE, &serde_json::to_vec(&raw).unwrap()).unwrap();
        self.dispatcher.handle(&event).await.unwrap();
    }
}

#[tokio::test]
async fn slack_channels_get_short_escaped_unsigned_messages() {
    let h = harness().await;
    let tenant = Uuid::now_v7();
    let (_, admin) = h.person(tenant, "admin", &[]);
    let slack = |url: &str| json!({ "kind": "slack", "name": "#payments", "url": url, "events": ["issue.created", "issue.transitioned"] });

    // In production only Slack's own address is accepted, and the URL — the credential — is
    // never shown back or stored in the clear.
    for bad in ["https://example.com/services/T/B/X", "https://hooks.slack.com/api/x", "http://hooks.slack.com/services/T/B/X"] {
        assert_eq!(h.on(&h.strict, "POST", "/api/v1/webhooks", &admin, Some(slack(bad))).await.0, 422, "{bad}");
    }
    let (status, real) = h
        .on(&h.strict, "POST", "/api/v1/webhooks", &admin, Some(slack("https://hooks.slack.com/services/T1/B1/hushhushWXYZ")))
        .await;
    assert_eq!(status, 201, "{real}");
    assert_eq!(
        (real["secret"].clone(), real["webhook"]["url"].as_str(), real["webhook"]["kind"].as_str()),
        (Value::Null, Some("https://hooks.slack.com/…WXYZ"), Some("slack"))
    );
    let stored: (String, Vec<u8>) =
        sqlx::query_as("SELECT url, secret_sealed FROM webhooks").fetch_one(&h.db.owner).await.unwrap();
    assert!(!stored.0.contains("hushhush") && !stored.1.windows(8).any(|w| w == b"hushhush"));
    let real_id = real["webhook"]["id"].as_str().unwrap();
    assert_eq!(h.call("POST", &format!("/api/v1/webhooks/{real_id}/secret"), &admin, None).await.0, 409, "nothing to rotate");
    assert_eq!(h.call("DELETE", &format!("/api/v1/webhooks/{real_id}"), &admin, None).await.0, 204);

    // A channel (here, a local stand-in) gets one line per event, with the key linked and the
    // summary escaped so it cannot ping the channel or smuggle a link.
    let (status, created) = h.call("POST", "/api/v1/webhooks", &admin, Some(slack(&h.endpoint))).await;
    assert_eq!(status, 201, "{created}");
    let id = created["webhook"]["id"].as_str().unwrap().to_owned();
    let (pay, issue) = (Uuid::now_v7(), Uuid::now_v7());
    h.event(
        tenant,
        "issue.created",
        json!({ "issue_id": issue, "key": "PAY-1", "project_id": pay, "issue_type": "bug",
        "summary": "<!channel> refunds & <https://evil.example|click>", "reporter_id": Uuid::now_v7(), "assignee_id": null }),
    )
    .await;
    h.event(
        tenant,
        "comment.created",
        json!({ "issue_id": issue, "key": "PAY-1", "project_id": pay, "comment_id": Uuid::now_v7(),
        "author_id": Uuid::now_v7(), "mentioned_user_ids": [] }),
    )
    .await;
    assert_eq!(webhook_sender::run_once(&h.db.app, &SecretBox::derive(SECRET), DEV).await.unwrap(), 1, "only subscribed events");
    let (headers, body) = h.receiver.got.lock().unwrap()[0].clone();
    assert!(headers.get("x-meridian-signature").is_none(), "Slack messages are not signed");
    assert_eq!(
        serde_json::from_slice::<Value>(&body).unwrap(),
        json!({ "text": "<https://meridian.example/issues/PAY-1|PAY-1> was created: &lt;!channel&gt; refunds &amp; &lt;https://evil.example|click&gt;" })
    );

    // A removed Slack webhook answers 404: the channel is turned off.
    *h.receiver.status.lock().unwrap() = 404;
    assert_eq!(h.call("POST", &format!("/api/v1/webhooks/{id}/test"), &admin, None).await.0, 202);
    webhook_sender::run_once(&h.db.app, &SecretBox::derive(SECRET), DEV).await.unwrap();
    let (_, listed) = h.call("GET", "/api/v1/webhooks", &admin, None).await;
    assert_eq!(
        (listed["items"][0]["enabled"].as_bool(), listed["items"][0]["disabled_reason"].as_str()),
        (Some(false), Some("The endpoint answered 410 Gone."))
    );
    h.db.cleanup().await;
}

fn push(sha: &str, message: &str, branch: &str) -> Value {
    json!({
        "ref": format!("refs/heads/{branch}"),
        "repository": { "full_name": "acme/payments", "html_url": "https://github.com/acme/payments" },
        "commits": [{ "id": sha, "message": message, "timestamp": "2026-11-16T10:00:00Z",
                      "url": format!("https://github.com/acme/payments/commit/{sha}"), "author": { "name": "Ada" } }]
    })
}

fn pull_request(action: &str, state: &str, merged: bool) -> Value {
    json!({
        "action": action,
        "repository": { "full_name": "acme/payments", "html_url": "https://github.com/acme/payments" },
        "pull_request": { "number": 7, "title": "Retry refunds", "html_url": "https://github.com/acme/payments/pull/7",
            "state": state, "merged": merged, "merged_at": null, "updated_at": "2026-11-16T12:00:00Z",
            "head": { "ref": "pay-9-retries" }, "user": { "login": "ada" } }
    })
}

#[tokio::test]
async fn github_links_commits_branches_and_pull_requests_to_issues() {
    let h = harness().await;
    let (tenant, other) = (Uuid::now_v7(), Uuid::now_v7());
    let (pay, ops) = (Uuid::now_v7(), Uuid::now_v7());
    h.world.projects.lock().unwrap().extend([((tenant, "PAY".to_owned()), pay), ((tenant, "OPS".to_owned()), ops)]);
    let (_, admin) = h.person(tenant, "admin", &[]);
    let (_, member) = h.person(tenant, "member", &[(pay, ProjectRole::Viewer)]);
    let (_, outsider) = h.person(tenant, "member", &[(ops, ProjectRole::Member)]);
    let (_, stranger_admin) = h.person(other, "admin", &[]);

    // Administrators connect a repository for chosen projects; ids come from project-service.
    let connect = |provider: &str, keys: Value| json!({ "provider": provider, "name": "Payments repo", "project_keys": keys });
    assert_eq!(h.call("POST", "/api/v1/source-connections", &member, Some(connect("github", json!(["PAY"])))).await.0, 403);
    assert_eq!(h.call("POST", "/api/v1/source-connections", &admin, Some(connect("gitlab", json!(["PAY"])))).await.0, 422);
    assert_eq!(h.call("POST", "/api/v1/source-connections", &admin, Some(connect("github", json!(["NOPE"])))).await.0, 422);
    let (status, made) = h.call("POST", "/api/v1/source-connections", &admin, Some(connect("github", json!(["pay"])))).await;
    assert_eq!(status, 201, "{made}");
    let id = made["connection"]["id"].as_str().unwrap().to_owned();
    let secret = made["secret"].as_str().unwrap().to_owned();
    assert!(secret.starts_with("ghsec_"));
    assert_eq!(
        (made["connection"]["project_keys"].clone(), made["connection"]["payload_path"].as_str()),
        (json!(["PAY"]), Some(format!("/hooks/github/{id}").as_str()))
    );
    let (_, listed) = h.call("GET", "/api/v1/source-connections", &admin, None).await;
    assert!(!listed.to_string().contains("ghsec_"), "the secret is shown once");

    // Deliveries count only with the connection's signature.
    let sha = "a".repeat(40);
    let body = push(&sha, "PAY-9: retry refunds (see OPS-1)", "feature/pay-9-retries");
    assert_eq!(h.github(&id, "push", &body, None).await.0, 401);
    assert_eq!(h.github(&id, "push", &body, Some("ghsec_wrong")).await.0, 401);
    assert_eq!(h.github(&Uuid::now_v7().to_string(), "push", &body, Some(&secret)).await.0, 404);
    assert_eq!(h.github(&id, "ping", &json!({ "zen": "Design for failure." }), Some(&secret)).await.1["result"], "pong");
    let (status, receipt) = h.github(&id, "push", &body, Some(&secret)).await;
    assert_eq!((status, receipt["links"].as_u64()), (200, Some(2)), "PAY-9's commit and branch; OPS is not connected");
    assert_eq!(h.github(&id, "issues", &json!({}), Some(&secret)).await.1["result"], "ignored");
    assert_eq!(h.github(&id, "push", &json!({ "ref": 1 }), Some(&secret)).await.0, 422, "signed but malformed");

    // A pull request is opened, then merged: one link whose state follows it.
    h.github(&id, "pull_request", &pull_request("opened", "open", false), Some(&secret)).await;
    h.github(&id, "pull_request", &pull_request("closed", "closed", true), Some(&secret)).await;

    // Anyone who can see the project reads them; nobody else learns anything.
    let (status, dev) = h.call("GET", "/api/v1/development?issue=pay-9", &member, None).await;
    assert_eq!(status, 200, "{dev}");
    let items = dev["items"].as_array().unwrap();
    let kinds: Vec<(&str, Option<&str>)> = items.iter().map(|i| (i["kind"].as_str().unwrap(), i["state"].as_str())).collect();
    // Newest first: the branch was seen just now, the commit and pull request carry GitHub's times.
    assert_eq!(kinds, [("branch", None), ("pull_request", Some("merged")), ("commit", None)]);
    assert_eq!(
        (items[2]["ref"].as_str(), items[2]["author"].as_str(), items[2]["url"].as_str()),
        (Some(sha.as_str()), Some("Ada"), Some(format!("https://github.com/acme/payments/commit/{sha}").as_str()))
    );
    assert_eq!(h.call("GET", "/api/v1/development?issue=PAY-9", &outsider, None).await.0, 404);
    let (_, ops_dev) = h.call("GET", "/api/v1/development?issue=OPS-1", &outsider, None).await;
    assert_eq!((ops_dev["connected"].as_bool(), ops_dev["items"].as_array().unwrap().len()), (Some(false), 0));
    let (_, theirs) = h.call("GET", "/api/v1/development?issue=PAY-9", &stranger_admin, None).await;
    assert_eq!(theirs["connected"], false, "another workspace sees nothing");
    assert_eq!(h.call("GET", "/api/v1/source-connections", &stranger_admin, None).await.1["items"], json!([]));
    assert_eq!(h.call("DELETE", &format!("/api/v1/source-connections/{id}"), &stranger_admin, None).await.0, 404);

    // Deleting the branch removes its link.
    let gone = json!({ "ref": "refs/heads/feature/pay-9-retries", "deleted": true,
        "repository": { "full_name": "acme/payments", "html_url": "https://github.com/acme/payments" } });
    assert_eq!(h.github(&id, "push", &gone, Some(&secret)).await.1["links"], 1);
    assert_eq!(h.call("GET", "/api/v1/development?issue=PAY-9", &member, None).await.1["items"].as_array().unwrap().len(), 2);

    // A new secret takes over at once; removing the connection removes its links and its address.
    let (_, rotated) = h.call("POST", &format!("/api/v1/source-connections/{id}/secret"), &admin, None).await;
    let fresh = rotated["secret"].as_str().unwrap();
    assert_eq!(h.github(&id, "ping", &json!({}), Some(&secret)).await.0, 401);
    assert_eq!(h.github(&id, "ping", &json!({}), Some(fresh)).await.0, 200);
    assert_eq!(h.call("DELETE", &format!("/api/v1/source-connections/{id}"), &admin, None).await.0, 204);
    assert_eq!(h.github(&id, "ping", &json!({}), Some(fresh)).await.0, 404);
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM dev_links").fetch_one(&h.db.owner).await.unwrap();
    assert_eq!(left, 0);

    // Every table with a tenant column is under forced row-level security, except the system
    // queues drained across tenants, which hold ids, times and outgoing payloads only.
    let unprotected: Vec<String> = sqlx::query_scalar(
        "SELECT c.relname::text FROM pg_class c JOIN pg_attribute a ON a.attrelid = c.oid AND a.attname = 'tenant_id' \
         WHERE c.relkind = 'r' AND c.relnamespace = 'public'::regnamespace \
           AND NOT (c.relrowsecurity AND c.relforcerowsecurity) ORDER BY 1",
    )
    .fetch_all(&h.db.owner)
    .await
    .unwrap();
    assert_eq!(unprotected, ["email_deliveries", "outbox", "source_hook_routes", "webhook_deliveries"]);
    h.db.cleanup().await;
}
