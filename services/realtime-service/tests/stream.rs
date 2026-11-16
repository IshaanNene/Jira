//! Live stream over real HTTP and the local Redis (`make infra`).

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};

use async_trait::async_trait;
use auth_middleware::{PolicyClient, PolicySnapshot, PolicySource, ProjectRole, RedisHandle};
use error_handling::ApiResult;
use event_bus::{Event, Handler};
use event_contracts::topics;
use realtime_service::{hub::Hub, quota::Quotas, Announcer, AppState};
use test_utilities::TestAuth;
use tokio::sync::Semaphore;
use uuid::Uuid;

fn redis_url() -> String {
    std::env::var("TEST_REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:63790".to_owned())
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
    base: String,
    auth: TestAuth,
    roles: Arc<Roles>,
    policy: Arc<PolicyClient>,
    announcer: Announcer,
    http: reqwest::Client,
}

async fn start() -> Harness {
    start_with(1000, 100).await
}

async fn start_with(per_workspace: usize, per_person: usize) -> Harness {
    let redis = RedisHandle::connect_in_background(&redis_url()).unwrap();
    for _ in 0..50 {
        if redis.is_connected() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let hub = Hub::start(&redis_url(), redis.clone()).unwrap();
    let auth = TestAuth::new().unwrap();
    let roles = Arc::new(Roles::default());
    // No Redis on the policy client: snapshots are not cached across a revocation in tests.
    let policy = Arc::new(PolicyClient::new(roles.clone(), RedisHandle::default()));
    let state = AppState {
        verifier: Arc::clone(&auth.verifier),
        policy: Arc::clone(&policy),
        hub: Arc::clone(&hub),
        redis: redis.clone(),
        quotas: Arc::new(Quotas::new(redis.clone(), per_workspace, per_person, Duration::from_secs(3))),
        slots: Arc::new(Semaphore::new(100)),
        recheck: Duration::from_millis(300),
        allowed_origins: Arc::new(vec![]),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, realtime_service::http::router(state)).await });
    // Let the fanout subscription come up.
    tokio::time::sleep(Duration::from_millis(300)).await;
    Harness { base, auth, roles, policy, announcer: Announcer { hub }, http: reqwest::Client::new() }
}

struct Member {
    user: Uuid,
    tenant: Uuid,
    token: String,
}

impl Harness {
    fn member(&self, tenant: Uuid) -> Member {
        let user = Uuid::now_v7();
        let token = self.auth.org_token(user, "ada@example.com", tenant, Uuid::now_v7(), "member").unwrap();
        Member { user, tenant, token }
    }

    fn grant(&self, m: &Member, project: Uuid) {
        self.roles.0.lock().unwrap().entry((m.tenant, m.user)).or_default().insert(project, ProjectRole::Viewer);
        self.policy.forget(m.tenant, m.user);
    }

    fn revoke(&self, m: &Member, project: Uuid) {
        self.roles.0.lock().unwrap().entry((m.tenant, m.user)).or_default().remove(&project);
        self.policy.forget(m.tenant, m.user);
    }

    async fn open(&self, token: &str, channel: &str, cursor: Option<&str>) -> reqwest::Response {
        let mut request =
            self.http.get(format!("{}/api/v1/rt/stream", self.base)).query(&[("channel", channel)]).bearer_auth(token);
        if let Some(cursor) = cursor {
            request = request.header("last-event-id", cursor);
        }
        request.send().await.unwrap()
    }

    async fn announce(&self, tenant: Uuid, project: Uuid, key: &str) {
        let raw = serde_json::json!({
            "event_id": Uuid::now_v7(), "event_type": "issue.updated", "schema_version": 1, "tenant_id": tenant,
            "aggregate_type": "issue", "aggregate_id": Uuid::now_v7(), "aggregate_version": 2,
            "occurred_at": time::OffsetDateTime::now_utc().format(&time::format_description::well_known::Rfc3339).unwrap(),
            "correlation_id": null, "causation_id": null, "actor": {"type": "system", "id": "test"},
            "payload": { "project_id": project, "key": key, "changed_fields": ["summary"] }
        });
        let event = Event::parse(topics::ISSUE, &serde_json::to_vec(&raw).unwrap()).unwrap();
        self.announcer.handle(&event).await.unwrap();
    }
}

#[derive(Debug)]
struct SseEvent {
    name: String,
    id: Option<String>,
    data: String,
}

/// Reads one SSE stream event by event.
struct Reader {
    response: reqwest::Response,
    buffer: String,
}

impl Reader {
    fn new(response: reqwest::Response) -> Self {
        assert_eq!(response.status(), 200);
        Self { response, buffer: String::new() }
    }

    async fn next(&mut self) -> Option<SseEvent> {
        loop {
            if let Some(end) = self.buffer.find("\n\n") {
                let block: String = self.buffer.drain(..end + 2).collect();
                let mut event = SseEvent { name: "message".into(), id: None, data: String::new() };
                let mut comment_only = true;
                for line in block.lines() {
                    if let Some(v) = line.strip_prefix("event:") {
                        event.name = v.trim().to_owned();
                        comment_only = false;
                    } else if let Some(v) = line.strip_prefix("id:") {
                        event.id = Some(v.trim().to_owned());
                    } else if let Some(v) = line.strip_prefix("data:") {
                        event.data.push_str(v.trim());
                        comment_only = false;
                    }
                }
                if comment_only {
                    continue; // keep-alive
                }
                return Some(event);
            }
            let chunk =
                tokio::time::timeout(Duration::from_secs(10), self.response.chunk()).await.expect("stream stalled").ok()??;
            self.buffer.push_str(&String::from_utf8_lossy(&chunk));
        }
    }

    async fn expect(&mut self, name: &str) -> SseEvent {
        let event = self.next().await.unwrap_or_else(|| panic!("stream ended, expected {name}"));
        assert_eq!(event.name, name, "{event:?}");
        event
    }
}

#[tokio::test]
async fn members_receive_hints_for_projects_they_can_browse_and_nothing_else() {
    let h = start().await;
    let tenant = Uuid::now_v7();
    let (project, other_project) = (Uuid::now_v7(), Uuid::now_v7());
    let ada = h.member(tenant);
    h.grant(&ada, project);

    let mut stream = Reader::new(h.open(&ada.token, &format!("project:{project}"), None).await);
    stream.expect("ready").await;

    // Another project in the same workspace, and the same project id in another tenant.
    h.announce(tenant, other_project, "OTHER-1").await;
    h.announce(Uuid::now_v7(), project, "FOREIGN-1").await;
    h.announce(tenant, project, "PAY-1").await;
    let change = stream.expect("change").await;
    let hint: serde_json::Value = serde_json::from_str(&change.data).unwrap();
    assert_eq!(hint["key"], "PAY-1");
    assert_eq!(hint["type"], "issue.updated");
    assert!(hint.get("changed_fields").is_none() && hint.get("payload").is_none(), "hints carry no payload");
}

#[tokio::test]
async fn subscriptions_are_authorized() {
    let h = start().await;
    let tenant = Uuid::now_v7();
    let ada = h.member(tenant);
    let project = Uuid::now_v7();

    assert_eq!(h.open(&ada.token, &format!("project:{project}"), None).await.status(), 404, "no access reads as not found");
    assert_eq!(h.open(&ada.token, &format!("user:{}", Uuid::now_v7()), None).await.status(), 404, "someone else's notifications");
    assert_eq!(h.open(&ada.token, &format!("user:{}", ada.user), None).await.status(), 200);
    assert_eq!(h.open(&ada.token, "org", None).await.status(), 200);
    assert_eq!(h.open(&ada.token, "tenant:everything", None).await.status(), 422);
    assert_eq!(h.open("not-a-token", "org", None).await.status(), 401);
    let account = h.auth.account_token(ada.user, "ada@example.com").unwrap();
    assert_eq!(h.open(&account, "org", None).await.status(), 401, "a workspace token is required");
}

#[tokio::test]
async fn a_reconnecting_client_catches_up_from_its_cursor() {
    let h = start().await;
    let tenant = Uuid::now_v7();
    let project = Uuid::now_v7();
    let ada = h.member(tenant);
    h.grant(&ada, project);
    let channel = format!("project:{project}");

    let mut first = Reader::new(h.open(&ada.token, &channel, None).await);
    h.announce(tenant, project, "PAY-1").await;
    let cursor = first.expect("ready").await.id.unwrap_or_default();
    first.expect("change").await;
    drop(first);

    // Missed while disconnected.
    h.announce(tenant, project, "PAY-2").await;
    h.announce(tenant, project, "PAY-3").await;

    let mut again = Reader::new(h.open(&ada.token, &channel, Some(&cursor)).await);
    let keys: Vec<String> = [again.expect("change").await, again.expect("change").await, again.expect("change").await]
        .iter()
        .map(|e| serde_json::from_str::<serde_json::Value>(&e.data).unwrap()["key"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(keys, ["PAY-1", "PAY-2", "PAY-3"], "everything after the cursor, in order");
    again.expect("ready").await;

    // A cursor into a buffer that no longer exists (expired, or Redis lost) means: refetch.
    let quiet = Uuid::now_v7();
    h.grant(&ada, quiet);
    let mut stale = Reader::new(h.open(&ada.token, &format!("project:{quiet}"), Some("1-0")).await);
    stale.expect("resync").await;
}

#[tokio::test]
async fn streams_end_when_access_is_withdrawn_or_the_token_expires() {
    let h = start().await;
    let tenant = Uuid::now_v7();
    let project = Uuid::now_v7();
    let ada = h.member(tenant);
    h.grant(&ada, project);

    let mut stream = Reader::new(h.open(&ada.token, &format!("project:{project}"), None).await);
    stream.expect("ready").await;
    h.revoke(&ada, project);
    stream.expect("denied").await;
    assert!(stream.next().await.is_none(), "the stream closes");

    let mut claims = TestAuth::claims(ada.user, "ada@example.com");
    claims.typ = auth_middleware::TokenKind::Org;
    claims.tid = Some(tenant);
    claims.org = Some(Uuid::now_v7());
    claims.roles = vec!["member".into()];
    claims.exp = time::OffsetDateTime::now_utc().unix_timestamp() + 2;
    let short = h.auth.sign(&claims).unwrap();
    let mut stream = Reader::new(h.open(&short, "org", None).await);
    stream.expect("ready").await;
    stream.expect("expired").await;
}

#[tokio::test]
async fn live_connections_are_capped_per_person_and_per_workspace() {
    let h = start_with(3, 2).await;
    let tenant = Uuid::now_v7();
    let (ada, bob, cy) = (h.member(tenant), h.member(tenant), h.member(tenant));

    let mut first = Reader::new(h.open(&ada.token, "org", None).await);
    first.expect("ready").await;
    let mut second = Reader::new(h.open(&ada.token, "org", None).await);
    second.expect("ready").await;
    let refused = h.open(&ada.token, "org", None).await;
    assert_eq!(refused.status(), 429, "a third tab of the same person");
    assert_eq!(refused.headers().get("retry-after").and_then(|v| v.to_str().ok()), Some("30"));

    let mut bobs = Reader::new(h.open(&bob.token, "org", None).await);
    bobs.expect("ready").await;
    assert_eq!(h.open(&cy.token, "org", None).await.status(), 429, "the workspace is full");
    let elsewhere = h.member(Uuid::now_v7());
    assert_eq!(h.open(&elsewhere.token, "org", None).await.status(), 200, "other workspaces are unaffected");

    // Closing a stream gives its place back.
    drop(second);
    let mut reopened = None;
    for _ in 0..50 {
        let response = h.open(&ada.token, "org", None).await;
        if response.status() == 200 {
            reopened = Some(response);
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let mut third = Reader::new(reopened.expect("a place came free"));
    third.expect("ready").await;

    // Open streams keep their places past the lease (3 s here) by renewing them.
    tokio::time::sleep(Duration::from_secs(4)).await;
    assert_eq!(h.open(&cy.token, "org", None).await.status(), 429, "renewed leases still count");
    drop((first, bobs, third));
}

#[tokio::test]
async fn people_see_who_else_is_looking_at_an_issue() {
    let h = start().await;
    let tenant = Uuid::now_v7();
    let (project, issue) = (Uuid::now_v7(), Uuid::now_v7());
    let (ada, bob, outsider) = (h.member(tenant), h.member(tenant), h.member(tenant));
    h.grant(&ada, project);
    h.grant(&bob, project);
    let here = |m: &Member, method: reqwest::Method, project: Uuid| {
        let request = h
            .http
            .request(method, format!("{}/api/v1/rt/presence", h.base))
            .bearer_auth(&m.token)
            .json(&serde_json::json!({ "project_id": project, "issue_id": issue }));
        async move {
            let response = request.send().await.unwrap();
            let status = response.status().as_u16();
            (status, response.json::<serde_json::Value>().await.unwrap_or_default())
        }
    };

    let mut bobs = Reader::new(h.open(&bob.token, &format!("project:{project}"), None).await);
    bobs.expect("ready").await;
    assert_eq!(here(&ada, reqwest::Method::PUT, project).await, (200, serde_json::json!({ "viewers": [] })));
    let arrived: serde_json::Value = serde_json::from_str(&bobs.expect("change").await.data).unwrap();
    assert_eq!((arrived["type"].as_str(), arrived["id"].as_str()), (Some("presence.changed"), Some(issue.to_string().as_str())));
    assert_eq!(arrived["actor"], ada.user.to_string());

    assert_eq!(here(&bob, reqwest::Method::PUT, project).await.1["viewers"], serde_json::json!([ada.user]));
    assert_eq!(here(&ada, reqwest::Method::PUT, project).await.1["viewers"], serde_json::json!([bob.user]), "never yourself");

    // Leaving is announced too.
    assert_eq!(here(&ada, reqwest::Method::DELETE, project).await.0, 204);
    let mut left = bobs.expect("change").await;
    let mut hint: serde_json::Value = serde_json::from_str(&left.data).unwrap();
    if hint["actor"] == bob.user.to_string() {
        left = bobs.expect("change").await; // bob's own arrival came first
        hint = serde_json::from_str(&left.data).unwrap();
    }
    assert_eq!((hint["type"].as_str(), hint["actor"].as_str()), (Some("presence.changed"), Some(ada.user.to_string().as_str())));
    assert_eq!(here(&bob, reqwest::Method::PUT, project).await.1["viewers"], serde_json::json!([]));

    // Without Browse: not found. Another workspace with the same ids sees nobody.
    assert_eq!(here(&outsider, reqwest::Method::PUT, project).await.0, 404);
    let stranger = h.member(Uuid::now_v7());
    h.grant(&stranger, project);
    assert_eq!(here(&stranger, reqwest::Method::PUT, project).await.1["viewers"], serde_json::json!([]));
}
