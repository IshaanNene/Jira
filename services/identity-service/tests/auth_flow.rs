//! End-to-end tests of the identity HTTP API against a real PostgreSQL.

use std::{collections::HashMap, net::SocketAddr, sync::Arc};

use async_trait::async_trait;
use auth_middleware::{OrgRole, RedisHandle};
use axum::{
    body::Body,
    extract::connect_info::MockConnectInfo,
    http::{header, Method, Request, StatusCode},
    Router,
};
use base64::Engine;
use error_handling::ApiResult;
use identity_service::{
    crypto::KeySealer,
    http,
    membership::{MembershipChecker, MembershipInfo},
    AppState, Settings, MIGRATOR,
};
use serde_json::{json, Value};
use test_utilities::TestDb;
use tower::ServiceExt;
use uuid::Uuid;

const ORIGIN: &str = "http://localhost:5173";

/// Stands in for the user service: a fixed (user, org) -> membership table.
#[derive(Default)]
struct FakeMembership(std::sync::Mutex<HashMap<(Uuid, Uuid), MembershipInfo>>);

#[async_trait]
impl MembershipChecker for FakeMembership {
    async fn check(&self, user_id: Uuid, organization_id: Uuid) -> ApiResult<Option<MembershipInfo>> {
        Ok(self.0.lock().unwrap().get(&(user_id, organization_id)).cloned())
    }
}

struct Harness {
    db: TestDb,
    app: Router,
    membership: Arc<FakeMembership>,
}

async fn harness() -> Harness {
    let db = TestDb::create(&MIGRATOR).await.expect("test database");
    let sealer = KeySealer::from_base64(&base64::engine::general_purpose::STANDARD.encode([1_u8; 32])).unwrap();
    let membership = Arc::new(FakeMembership::default());
    let settings = Settings {
        issuer: "http://identity.test".into(),
        allowed_origins: vec![ORIGIN.into()],
        cookie_secure: false,
        session_ttl_days: 30,
        trust_forwarded_for: false,
    };
    let state = AppState::build(db.app.clone(), settings, &sealer, membership.clone(), RedisHandle::default()).await.unwrap();
    let app = http::router(state).layer(MockConnectInfo(SocketAddr::from(([127, 0, 0, 1], 4000))));
    Harness { db, app, membership }
}

struct Reply {
    status: StatusCode,
    body: Value,
    /// Value of the refresh cookie if the response set one ("" when it was cleared).
    cookie: Option<String>,
    raw_set_cookie: Option<String>,
}

async fn call(
    app: &Router,
    method: Method,
    path: &str,
    body: Option<Value>,
    bearer: Option<&str>,
    cookie: Option<&str>,
) -> Reply {
    call_from(app, method, path, body, bearer, cookie, Some(ORIGIN)).await
}

async fn call_from(
    app: &Router,
    method: Method,
    path: &str,
    body: Option<Value>,
    bearer: Option<&str>,
    cookie: Option<&str>,
    origin: Option<&str>,
) -> Reply {
    let mut req = Request::builder().method(method).uri(path).header(header::CONTENT_TYPE, "application/json");
    if let Some(token) = bearer {
        req = req.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    if let Some(value) = cookie {
        req = req.header(header::COOKIE, format!("refresh_token={value}"));
    }
    if let Some(origin) = origin {
        req = req.header(header::ORIGIN, origin);
    }
    let body = body.map_or_else(Body::empty, |b| Body::from(b.to_string()));
    let response = app.clone().oneshot(req.body(body).unwrap()).await.unwrap();
    let status = response.status();
    let raw_set_cookie = response.headers().get(header::SET_COOKIE).map(|v| v.to_str().unwrap().to_owned());
    let cookie = raw_set_cookie
        .as_ref()
        .and_then(|v| v.split(';').next())
        .and_then(|kv| kv.strip_prefix("refresh_token="))
        .map(str::to_owned);
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20).await.unwrap();
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    Reply { status, body, cookie, raw_set_cookie }
}

async fn register(app: &Router, email: &str) -> Reply {
    let reply = call(
        app,
        Method::POST,
        "/api/v1/auth/register",
        Some(json!({ "email": email, "password": "a-long-enough-password", "display_name": "Test User" })),
        None,
        None,
    )
    .await;
    assert_eq!(reply.status, StatusCode::CREATED, "{}", reply.body);
    reply
}

fn token(reply: &Reply) -> String {
    reply.body["access_token"].as_str().unwrap().to_owned()
}

fn jwt_payload(token: &str) -> Value {
    let payload = token.split('.').nth(1).unwrap();
    serde_json::from_slice(&base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload).unwrap()).unwrap()
}

#[tokio::test]
async fn register_login_and_me() {
    let h = harness().await;
    let registered = register(&h.app, "Ada@Example.com").await;
    assert_eq!(registered.body["account"]["email"], "ada@example.com", "email is normalised");
    let cookie = registered.raw_set_cookie.clone().unwrap();
    assert!(cookie.contains("HttpOnly") && cookie.contains("SameSite=Strict") && cookie.contains("Path=/api/v1/auth"));
    assert_eq!(jwt_payload(&token(&registered))["typ"], "account");
    assert!(jwt_payload(&token(&registered)).get("tid").is_none(), "no tenant before an organization is selected");

    let login = call(
        &h.app,
        Method::POST,
        "/api/v1/auth/login",
        Some(json!({ "email": "ada@example.com", "password": "a-long-enough-password" })),
        None,
        None,
    )
    .await;
    assert_eq!(login.status, StatusCode::OK);

    let me = call(&h.app, Method::GET, "/api/v1/auth/me", None, Some(&token(&login)), None).await;
    assert_eq!(me.status, StatusCode::OK);
    assert_eq!(me.body["display_name"], "Test User");

    let anonymous = call(&h.app, Method::GET, "/api/v1/auth/me", None, None, None).await;
    assert_eq!(anonymous.status, StatusCode::UNAUTHORIZED);
    assert_eq!(anonymous.body["code"], "unauthenticated");

    // The password is stored as an Argon2id hash, never in clear text.
    let (hash,): (String,) = sqlx::query_as("SELECT password_hash FROM accounts").fetch_one(&h.db.owner).await.unwrap();
    assert!(hash.starts_with("$argon2id$"));
    let (events,): (i64,) = sqlx::query_as("SELECT count(*) FROM outbox WHERE event_type = 'identity.user_registered'")
        .fetch_one(&h.db.owner)
        .await
        .unwrap();
    assert_eq!(events, 1);
    h.db.cleanup().await;
}

#[tokio::test]
async fn registration_validates_input_and_rejects_duplicates() {
    let h = harness().await;
    let weak = call(
        &h.app,
        Method::POST,
        "/api/v1/auth/register",
        Some(json!({ "email": "not-an-email", "password": "short", "display_name": "" })),
        None,
        None,
    )
    .await;
    assert_eq!(weak.status, StatusCode::UNPROCESSABLE_ENTITY);
    let fields: Vec<&str> = weak.body["errors"].as_array().unwrap().iter().map(|e| e["field"].as_str().unwrap()).collect();
    assert_eq!(fields, ["display_name", "email", "password"]);

    let unknown_field = call(
        &h.app,
        Method::POST,
        "/api/v1/auth/register",
        Some(json!({ "email": "a@example.com", "password": "a-long-enough-password", "display_name": "A", "status": "admin" })),
        None,
        None,
    )
    .await;
    assert!(unknown_field.status.is_client_error(), "unknown fields are refused, not ignored");

    register(&h.app, "dup@example.com").await;
    let dup = call(
        &h.app,
        Method::POST,
        "/api/v1/auth/register",
        Some(json!({ "email": "DUP@example.com", "password": "a-long-enough-password", "display_name": "Dup" })),
        None,
        None,
    )
    .await;
    assert_eq!(dup.status, StatusCode::CONFLICT);
    h.db.cleanup().await;
}

#[tokio::test]
async fn wrong_password_and_unknown_email_are_indistinguishable_and_lock_after_five() {
    let h = harness().await;
    register(&h.app, "lock@example.com").await;
    let attempt = |email: &'static str, password: &'static str| {
        let app = h.app.clone();
        async move {
            call(&app, Method::POST, "/api/v1/auth/login", Some(json!({ "email": email, "password": password })), None, None)
                .await
        }
    };

    let unknown = attempt("nobody@example.com", "whatever-password").await;
    let wrong = attempt("lock@example.com", "wrong-password-1").await;
    assert_eq!(unknown.status, StatusCode::UNAUTHORIZED);
    assert_eq!(unknown.body, wrong.body, "same body for unknown account and wrong password");

    for _ in 0..4 {
        assert_eq!(attempt("lock@example.com", "wrong-password-1").await.status, StatusCode::UNAUTHORIZED);
    }
    // Five failures: the correct password is now refused too, until the lock expires.
    assert_eq!(attempt("lock@example.com", "a-long-enough-password").await.status, StatusCode::UNAUTHORIZED);

    sqlx::query("UPDATE accounts SET locked_until = now() - interval '1 second'").execute(&h.db.owner).await.unwrap();
    assert_eq!(attempt("lock@example.com", "a-long-enough-password").await.status, StatusCode::OK);
    let (count,): (i32,) = sqlx::query_as("SELECT failed_login_count FROM accounts").fetch_one(&h.db.owner).await.unwrap();
    assert_eq!(count, 0, "a successful login resets the counter");

    let outcomes: Vec<(String, i64)> =
        sqlx::query_as("SELECT outcome, count(*) FROM login_events GROUP BY outcome ORDER BY outcome")
            .fetch_all(&h.db.owner)
            .await
            .unwrap();
    assert_eq!(outcomes, vec![("bad_credentials".into(), 6), ("locked".into(), 1), ("success".into(), 1)]);
    h.db.cleanup().await;
}

#[tokio::test]
async fn refresh_rotates_and_reuse_revokes_the_session() {
    let h = harness().await;
    let registered = register(&h.app, "rot@example.com").await;
    let first = registered.cookie.clone().unwrap();

    let refreshed = call(&h.app, Method::POST, "/api/v1/auth/token/refresh", Some(json!({})), None, Some(&first)).await;
    assert_eq!(refreshed.status, StatusCode::OK, "{}", refreshed.body);
    let second = refreshed.cookie.clone().unwrap();
    assert_ne!(first, second, "a new refresh token is issued on every use");

    // Immediate replay of the old token: treated as a tab race, session survives.
    let race = call(&h.app, Method::POST, "/api/v1/auth/token/refresh", Some(json!({})), None, Some(&first)).await;
    assert_eq!(race.status, StatusCode::CONFLICT);
    let still_ok = call(&h.app, Method::POST, "/api/v1/auth/token/refresh", Some(json!({})), None, Some(&second)).await;
    assert_eq!(still_ok.status, StatusCode::OK);
    let third = still_ok.cookie.unwrap();

    // Replay after the grace window: theft. The whole session dies, including the newest token.
    sqlx::query("UPDATE refresh_tokens SET used_at = used_at - interval '1 minute' WHERE used_at IS NOT NULL")
        .execute(&h.db.owner)
        .await
        .unwrap();
    let theft = call(&h.app, Method::POST, "/api/v1/auth/token/refresh", Some(json!({})), None, Some(&first)).await;
    assert_eq!(theft.status, StatusCode::UNAUTHORIZED);
    assert_eq!(theft.cookie.as_deref(), Some(""), "cookie is cleared");
    let after = call(&h.app, Method::POST, "/api/v1/auth/token/refresh", Some(json!({})), None, Some(&third)).await;
    assert_eq!(after.status, StatusCode::UNAUTHORIZED, "the legitimate token is revoked with the session");

    let (reason,): (String,) = sqlx::query_as("SELECT revoked_reason FROM sessions").fetch_one(&h.db.owner).await.unwrap();
    assert_eq!(reason, "refresh_token_reuse");
    // Only hashes are stored.
    let (stored,): (Vec<u8>,) =
        sqlx::query_as("SELECT token_hash FROM refresh_tokens LIMIT 1").fetch_one(&h.db.owner).await.unwrap();
    assert_eq!(stored.len(), 32);
    h.db.cleanup().await;
}

#[tokio::test]
async fn refresh_requires_cookie_and_trusted_origin() {
    let h = harness().await;
    let registered = register(&h.app, "origin@example.com").await;
    let cookie = registered.cookie.unwrap();

    let none = call(&h.app, Method::POST, "/api/v1/auth/token/refresh", Some(json!({})), None, None).await;
    assert_eq!(none.status, StatusCode::UNAUTHORIZED);
    let garbage = call(&h.app, Method::POST, "/api/v1/auth/token/refresh", Some(json!({})), None, Some("made-up")).await;
    assert_eq!(garbage.status, StatusCode::UNAUTHORIZED);

    let cross_site = call_from(
        &h.app,
        Method::POST,
        "/api/v1/auth/token/refresh",
        Some(json!({})),
        None,
        Some(&cookie),
        Some("https://evil.example"),
    )
    .await;
    assert_eq!(cross_site.status, StatusCode::FORBIDDEN);
    // The forged request must not have consumed the token.
    let genuine = call(&h.app, Method::POST, "/api/v1/auth/token/refresh", Some(json!({})), None, Some(&cookie)).await;
    assert_eq!(genuine.status, StatusCode::OK);
    h.db.cleanup().await;
}

#[tokio::test]
async fn exchange_issues_org_token_only_for_members() {
    let h = harness().await;
    let registered = register(&h.app, "member@example.com").await;
    let user_id = Uuid::parse_str(registered.body["account"]["id"].as_str().unwrap()).unwrap();
    let (org, tenant, other_org) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    h.membership.0.lock().unwrap().insert((user_id, org), MembershipInfo { tenant_id: tenant, role: OrgRole::Admin });

    let ok = call(
        &h.app,
        Method::POST,
        "/api/v1/auth/token/exchange",
        Some(json!({ "organization_id": org })),
        Some(&token(&registered)),
        None,
    )
    .await;
    assert_eq!(ok.status, StatusCode::OK, "{}", ok.body);
    let claims = jwt_payload(&token(&ok));
    assert_eq!(claims["typ"], "org");
    assert_eq!(claims["tid"], tenant.to_string());
    assert_eq!(claims["org"], org.to_string());
    assert_eq!(claims["roles"], json!(["admin"]));
    assert_eq!(ok.body["organization"]["role"], "admin");

    let denied = call(
        &h.app,
        Method::POST,
        "/api/v1/auth/token/exchange",
        Some(json!({ "organization_id": other_org })),
        Some(&token(&registered)),
        None,
    )
    .await;
    assert_eq!(denied.status, StatusCode::FORBIDDEN);

    // A client cannot name its own tenant or role.
    let smuggled = call(
        &h.app,
        Method::POST,
        "/api/v1/auth/token/exchange",
        Some(json!({ "organization_id": other_org, "tenant_id": tenant, "role": "owner" })),
        Some(&token(&registered)),
        None,
    )
    .await;
    assert!(smuggled.status.is_client_error());

    // An org token can be exchanged again, and membership is re-checked: once removed, it fails.
    h.membership.0.lock().unwrap().clear();
    let stale = call(
        &h.app,
        Method::POST,
        "/api/v1/auth/token/exchange",
        Some(json!({ "organization_id": org })),
        Some(&token(&ok)),
        None,
    )
    .await;
    assert_eq!(stale.status, StatusCode::FORBIDDEN);

    // Refresh with an organization scope the user no longer has falls back to an account token.
    let refreshed = call(
        &h.app,
        Method::POST,
        "/api/v1/auth/token/refresh",
        Some(json!({ "organization_id": org })),
        None,
        Some(&registered.cookie.clone().unwrap()),
    )
    .await;
    assert_eq!(refreshed.status, StatusCode::OK);
    assert!(refreshed.body["organization"].is_null());
    assert_eq!(jwt_payload(&token(&refreshed))["typ"], "account");
    h.db.cleanup().await;
}

#[tokio::test]
async fn logout_ends_the_session() {
    let h = harness().await;
    let registered = register(&h.app, "bye@example.com").await;
    let cookie = registered.cookie.clone().unwrap();

    let out = call(&h.app, Method::POST, "/api/v1/auth/logout", None, None, Some(&cookie)).await;
    assert_eq!(out.status, StatusCode::NO_CONTENT);
    assert_eq!(out.cookie.as_deref(), Some(""));

    let refresh = call(&h.app, Method::POST, "/api/v1/auth/token/refresh", Some(json!({})), None, Some(&cookie)).await;
    assert_eq!(refresh.status, StatusCode::UNAUTHORIZED);
    // The access token cannot be upgraded after logout even though its signature is still valid.
    let exchange = call(
        &h.app,
        Method::POST,
        "/api/v1/auth/token/exchange",
        Some(json!({ "organization_id": Uuid::now_v7() })),
        Some(&token(&registered)),
        None,
    )
    .await;
    assert_eq!(exchange.status, StatusCode::UNAUTHORIZED);
    h.db.cleanup().await;
}

#[tokio::test]
async fn sessions_are_listed_and_revocable_only_by_their_owner() {
    let h = harness().await;
    let alice = register(&h.app, "alice@example.com").await;
    let alice_second = call(
        &h.app,
        Method::POST,
        "/api/v1/auth/login",
        Some(json!({ "email": "alice@example.com", "password": "a-long-enough-password" })),
        None,
        None,
    )
    .await;
    let mallory = register(&h.app, "mallory@example.com").await;

    let list = call(&h.app, Method::GET, "/api/v1/auth/sessions", None, Some(&token(&alice)), None).await;
    let items = list.body["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items.iter().filter(|s| s["current"] == true).count(), 1);
    let other = items.iter().find(|s| s["current"] == false).unwrap()["id"].as_str().unwrap().to_owned();

    // Broken object-level authorization check: Mallory knows the id but does not own it.
    let stolen =
        call(&h.app, Method::DELETE, &format!("/api/v1/auth/sessions/{other}"), None, Some(&token(&mallory)), None).await;
    assert_eq!(stolen.status, StatusCode::NOT_FOUND);

    let revoked = call(&h.app, Method::DELETE, &format!("/api/v1/auth/sessions/{other}"), None, Some(&token(&alice)), None).await;
    assert_eq!(revoked.status, StatusCode::NO_CONTENT);
    let dead =
        call(&h.app, Method::POST, "/api/v1/auth/token/refresh", Some(json!({})), None, Some(&alice_second.cookie.unwrap()))
            .await;
    assert_eq!(dead.status, StatusCode::UNAUTHORIZED);
    let list = call(&h.app, Method::GET, "/api/v1/auth/sessions", None, Some(&token(&alice)), None).await;
    assert_eq!(list.body["items"].as_array().unwrap().len(), 1);
    h.db.cleanup().await;
}

#[tokio::test]
async fn jwks_exposes_public_key_only() {
    let h = harness().await;
    let jwks = call(&h.app, Method::GET, "/api/v1/auth/.well-known/jwks.json", None, None, None).await;
    let key = &jwks.body["keys"][0];
    assert_eq!((key["kty"].as_str(), key["crv"].as_str(), key["alg"].as_str()), (Some("OKP"), Some("Ed25519"), Some("EdDSA")));
    assert!(key.get("d").is_none(), "private component must never be published");
    // The private key at rest is sealed, not the raw PKCS#8 document.
    let (sealed,): (Vec<u8>,) = sqlx::query_as("SELECT private_key_enc FROM signing_keys").fetch_one(&h.db.owner).await.unwrap();
    assert!(!sealed.starts_with(&[0x30, 0x51]) && !sealed.starts_with(&[0x30, 0x53]));
    h.db.cleanup().await;
}
