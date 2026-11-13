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
use realtime_service::{hub::Hub, Announcer, AppState};
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
