//! Outgoing webhooks end to end: REST → consumer → queue → a real HTTP receiver.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use auth_middleware::{PolicyClient, PolicySnapshot, PolicySource, RedisHandle};
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
    http, webhook_sender,
    webhooks::{signature, Dispatcher, Reach, SecretBox, Settings},
    AppState, MIGRATOR,
};
use serde_json::{json, Value};
use sqlx::PgPool;
use tenant_context::DbRouter;
use test_utilities::{TestAuth, TestDb};
use tower::ServiceExt;
use uuid::Uuid;

struct NoRoles;

#[async_trait::async_trait]
impl PolicySource for NoRoles {
    async fn fetch(&self, _tenant_id: Uuid, _user_id: Uuid) -> ApiResult<PolicySnapshot> {
        Ok(PolicySnapshot { version: 0, roles: HashMap::new(), default_roles: HashMap::new() })
    }
}

#[derive(Default)]
struct Receiver {
    /// Status to answer with.
    status: Mutex<u16>,
    got: Mutex<Vec<(HeaderMap, Bytes)>>,
}

async fn receive(State(r): State<Arc<Receiver>>, headers: HeaderMap, body: Bytes) -> StatusCode {
    r.got.lock().unwrap().push((headers, body));
    StatusCode::from_u16(*r.status.lock().unwrap()).unwrap()
}

const SECRET: &str = "test-secret-for-webhooks";

struct Harness {
    db: TestDb,
    app: Router,
    strict: Router,
    dispatcher: Dispatcher,
    auth: TestAuth,
    receiver: Arc<Receiver>,
    endpoint: String,
}

fn state(db: &PgPool, auth: &TestAuth, reach: Reach) -> AppState {
    AppState {
        db: Arc::new(DbRouter::single(db.clone())),
        verifier: Arc::clone(&auth.verifier),
        policy: Arc::new(PolicyClient::new(Arc::new(NoRoles), RedisHandle::default())),
        allowed_origins: Arc::new(vec![]),
        unsubscribe: Arc::new(notification_service::unsubscribe::UnsubscribeKey::derive(SECRET)),
        webhooks: Arc::new(Settings { secrets: SecretBox::derive(SECRET), reach }),
    }
}

async fn harness() -> Harness {
    let db = TestDb::create(&MIGRATOR).await.expect("test database");
    let auth = TestAuth::new().unwrap();
    let dev = Reach { allow_http: true, allow_private: true };
    let receiver = Arc::new(Receiver { status: Mutex::new(200), ..Default::default() });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/hook", listener.local_addr().unwrap());
    let server = Router::new().route("/hook", post(receive)).with_state(Arc::clone(&receiver));
    tokio::spawn(async move { axum::serve(listener, server).await.unwrap() });
    Harness {
        app: http::router(state(&db.app, &auth, dev)),
        strict: http::router(state(&db.app, &auth, Reach::default())),
        dispatcher: Dispatcher { db: Arc::new(DbRouter::single(db.app.clone())) },
        db,
        auth,
        receiver,
        endpoint,
    }
}

impl Harness {
    fn token(&self, tenant: Uuid, role: &str) -> String {
        self.auth.org_token(Uuid::now_v7(), "x@example.com", tenant, Uuid::now_v7(), role).unwrap()
    }

    async fn request(&self, app: &Router, method: &str, path: &str, token: &str, body: Option<Value>) -> (u16, Value) {
        let mut req = Request::builder().method(method).uri(path).header("authorization", format!("Bearer {token}"));
        if body.is_some() {
            req = req.header("content-type", "application/json");
        }
        let req = req.body(body.map_or_else(Body::empty, |b| Body::from(b.to_string()))).unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        let status = res.status().as_u16();
        let bytes = axum::body::to_bytes(res.into_body(), 1 << 20).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    async fn call(&self, method: &str, path: &str, token: &str, body: Option<Value>) -> (u16, Value) {
        self.request(&self.app, method, path, token, body).await
    }

    async fn event(&self, tenant: Uuid, event_type: &str, payload: Value) -> Uuid {
        let event_id = Uuid::now_v7();
        let raw = json!({
            "event_id": event_id, "event_type": event_type, "schema_version": 1, "tenant_id": tenant,
            "aggregate_type": "issue", "aggregate_id": payload["issue_id"], "aggregate_version": 2,
            "occurred_at": time::OffsetDateTime::now_utc().format(&time::format_description::well_known::Rfc3339).unwrap(),
            "correlation_id": null, "causation_id": null, "actor": {"type": "user", "id": Uuid::now_v7()}, "payload": payload
        });
        let event = Event::parse(topics::ISSUE, &serde_json::to_vec(&raw).unwrap()).unwrap();
        self.dispatcher.handle(&event).await.unwrap();
        // Redelivery queues nothing more.
        self.dispatcher.handle(&event).await.unwrap();
        event_id
    }

    async fn send(&self, reach: Reach) -> usize {
        webhook_sender::run_once(&self.db.app, &SecretBox::derive(SECRET), reach).await.unwrap()
    }

    /// Make every pending retry due now.
    async fn hurry(&self) {
        sqlx::query("UPDATE webhook_deliveries SET send_after = now() WHERE status = 'pending'")
            .execute(&self.db.owner)
            .await
            .unwrap();
    }

    fn received(&self) -> Vec<(HeaderMap, Value)> {
        self.receiver.got.lock().unwrap().iter().map(|(h, b)| (h.clone(), serde_json::from_slice(b).unwrap())).collect()
    }
}

fn verify(secret: &str, headers: &HeaderMap, body: &Bytes) -> bool {
    let header = headers["x-meridian-signature"].to_str().unwrap();
    let t: i64 = header.split(',').next().unwrap().trim_start_matches("t=").parse().unwrap();
    header == signature(secret, t, body)
}

#[tokio::test]
async fn webhooks_deliver_signed_events_to_subscribed_endpoints() {
    let h = harness().await;
    let tenant = Uuid::now_v7();
    let (admin, member) = (h.token(tenant, "admin"), h.token(tenant, "member"));
    let (pay, ops) = (Uuid::now_v7(), Uuid::now_v7());

    // Administrators only, HTTPS to public addresses only (in production).
    assert_eq!(h.call("GET", "/api/v1/webhooks", &member, None).await.0, 403);
    for bad in [
        "https://169.254.169.254/latest/meta-data",
        "http://hooks.example.com/",
        "https://127.0.0.1/x",
        "https://u:p@hooks.example.com/",
    ] {
        let (status, _) = h
            .request(
                &h.strict,
                "POST",
                "/api/v1/webhooks",
                &admin,
                Some(json!({ "name": "x", "url": bad, "events": ["issue.created"] })),
            )
            .await;
        assert_eq!(status, 422, "{bad}");
    }
    let (status, _) = h
        .call("POST", "/api/v1/webhooks", &admin, Some(json!({ "name": "x", "url": h.endpoint, "events": ["issue.exploded"] })))
        .await;
    assert_eq!(status, 422);

    let (status, created) = h
        .call(
            "POST",
            "/api/v1/webhooks",
            &admin,
            Some(
                json!({ "name": "CI", "url": h.endpoint, "events": ["issue.created", "comment.created"], "project_ids": [pay] }),
            ),
        )
        .await;
    assert_eq!(status, 201, "{created}");
    let secret = created["secret"].as_str().unwrap().to_owned();
    let id = created["webhook"]["id"].as_str().unwrap().to_owned();
    assert!(secret.starts_with("whsec_"));
    let (listed,) = (h.call("GET", "/api/v1/webhooks", &admin, None).await.1,);
    assert!(listed.to_string().find("whsec_").is_none(), "the secret is shown once");
    let stored: Vec<u8> = sqlx::query_scalar("SELECT secret_sealed FROM webhooks").fetch_one(&h.db.owner).await.unwrap();
    assert!(!stored.windows(secret.len()).any(|w| w == secret.as_bytes()), "stored encrypted");

    // Subscribed event in the chosen project: delivered once, signed.
    let issue = Uuid::now_v7();
    let event_id = h.event(tenant, "issue.created", json!({ "issue_id": issue, "project_id": pay, "key": "PAY-1" })).await;
    // Not subscribed, other project, other tenant: nothing.
    h.event(tenant, "issue.updated", json!({ "issue_id": issue, "project_id": pay })).await;
    h.event(tenant, "issue.created", json!({ "issue_id": Uuid::now_v7(), "project_id": ops })).await;
    h.event(Uuid::now_v7(), "issue.created", json!({ "issue_id": Uuid::now_v7(), "project_id": pay })).await;
    assert_eq!(h.send(Reach { allow_http: true, allow_private: true }).await, 1);
    let got = h.receiver.got.lock().unwrap().clone();
    assert_eq!(got.len(), 1);
    let (headers, raw) = &got[0];
    assert!(verify(&secret, headers, raw), "signature checks out");
    assert_eq!(headers["x-meridian-event"], "issue.created");
    let body: Value = serde_json::from_slice(raw).unwrap();
    assert_eq!(
        (body["id"].as_str(), body["type"].as_str(), body["data"]["key"].as_str()),
        (Some(event_id.to_string().as_str()), Some("issue.created"), Some("PAY-1"))
    );
    assert!(body.get("tenant_id").is_none());
    let (_, log) = h.call("GET", &format!("/api/v1/webhooks/{id}/deliveries"), &admin, None).await;
    assert_eq!((log["items"][0]["status"].as_str(), log["items"][0]["response_status"].as_i64()), (Some("delivered"), Some(200)));

    // Another workspace's administrator sees none of it.
    let stranger = h.token(Uuid::now_v7(), "owner");
    assert_eq!(h.call("GET", &format!("/api/v1/webhooks/{id}/deliveries"), &stranger, None).await.0, 404);
    assert_eq!(h.call("PATCH", &format!("/api/v1/webhooks/{id}"), &stranger, Some(json!({ "enabled": false }))).await.0, 404);

    // A private address is refused at delivery time too, and nothing is sent.
    h.event(tenant, "comment.created", json!({ "issue_id": issue, "project_id": pay, "comment_id": Uuid::now_v7() })).await;
    assert_eq!(h.send(Reach::default()).await, 1);
    assert_eq!(h.received().len(), 1);
    let (_, log) = h.call("GET", &format!("/api/v1/webhooks/{id}/deliveries"), &admin, None).await;
    assert_eq!(log["items"][0]["status"], "pending");
    assert!(log["items"][0]["last_error"].as_str().unwrap().contains("private"));

    // Failures are retried with backoff, then given up on.
    *h.receiver.status.lock().unwrap() = 500;
    for _ in 0..webhook_sender::MAX_ATTEMPTS {
        h.hurry().await;
        h.send(Reach { allow_http: true, allow_private: true }).await;
    }
    let (_, log) = h.call("GET", &format!("/api/v1/webhooks/{id}/deliveries"), &admin, None).await;
    assert_eq!((log["items"][0]["status"].as_str(), log["items"][0]["response_status"].as_i64()), (Some("failed"), Some(500)));
    assert_eq!(h.call("GET", "/api/v1/webhooks", &admin, None).await.1["items"][0]["consecutive_failures"], 1);

    // 410 Gone turns the webhook off; later events are not queued.
    *h.receiver.status.lock().unwrap() = 410;
    h.event(tenant, "issue.created", json!({ "issue_id": Uuid::now_v7(), "project_id": pay })).await;
    h.send(Reach { allow_http: true, allow_private: true }).await;
    let (_, hooks) = h.call("GET", "/api/v1/webhooks", &admin, None).await;
    assert_eq!(hooks["items"][0]["enabled"], false);
    assert!(hooks["items"][0]["disabled_reason"].as_str().unwrap().contains("410"));
    h.event(tenant, "issue.created", json!({ "issue_id": Uuid::now_v7(), "project_id": pay })).await;
    let queued: i64 = sqlx::query_scalar("SELECT count(*) FROM webhook_deliveries WHERE status = 'pending'")
        .fetch_one(&h.db.owner)
        .await
        .unwrap();
    assert_eq!(queued, 0);
    assert_eq!(h.call("POST", &format!("/api/v1/webhooks/{id}/test"), &admin, None).await.0, 409);

    // Turned back on: failures cleared; a test ping uses a fresh secret after rotation.
    *h.receiver.status.lock().unwrap() = 204;
    let (_, on) = h.call("PATCH", &format!("/api/v1/webhooks/{id}"), &admin, Some(json!({ "enabled": true }))).await;
    assert_eq!(
        (on["enabled"].as_bool(), on["disabled_reason"].clone(), on["consecutive_failures"].as_i64()),
        (Some(true), Value::Null, Some(0))
    );
    let (_, rotated) = h.call("POST", &format!("/api/v1/webhooks/{id}/secret"), &admin, None).await;
    let fresh = rotated["secret"].as_str().unwrap().to_owned();
    assert_ne!(fresh, secret);
    assert_eq!(h.call("POST", &format!("/api/v1/webhooks/{id}/test"), &admin, None).await.0, 202);
    h.send(Reach { allow_http: true, allow_private: true }).await;
    let got = h.receiver.got.lock().unwrap().clone();
    let (headers, raw) = got.last().unwrap();
    assert_eq!(headers["x-meridian-event"], "webhook.ping");
    assert!(verify(&fresh, headers, raw) && !verify(&secret, headers, raw));

    // Deleting removes the webhook and its log.
    assert_eq!(h.call("DELETE", &format!("/api/v1/webhooks/{id}"), &admin, None).await.0, 204);
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM webhook_deliveries").fetch_one(&h.db.owner).await.unwrap();
    assert_eq!(left, 0);
    h.db.cleanup().await;
}
