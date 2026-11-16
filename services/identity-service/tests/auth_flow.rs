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
    async fn active_members(&self, tenant_id: Uuid, user_ids: &[Uuid]) -> ApiResult<Vec<Uuid>> {
        let table = self.0.lock().unwrap();
        Ok(user_ids.iter().copied().filter(|u| table.iter().any(|((who, _), m)| who == u && m.tenant_id == tenant_id)).collect())
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
    let state = AppState::build(db.app.clone(), settings, sealer, membership.clone(), RedisHandle::default()).await.unwrap();
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
    h.membership
        .0
        .lock()
        .unwrap()
        .insert((user_id, org), MembershipInfo { tenant_id: tenant, role: OrgRole::Admin, require_two_step: false });

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

fn base32_decode(input: &str) -> Vec<u8> {
    let (mut buffer, mut bits, mut out) = (0_u32, 0_u32, Vec::new());
    for c in input.bytes() {
        let value = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567".iter().position(|&a| a == c).unwrap() as u32;
        buffer = (buffer << 5) | value;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
        }
    }
    out
}

fn step_now() -> i64 {
    identity_service::mfa::step_at(time::OffsetDateTime::now_utc().unix_timestamp())
}

/// A six-digit code that is wrong at every step the server could accept right now.
fn wrong_code(secret: &[u8]) -> String {
    let now = step_now();
    let valid: Vec<String> = (now - 2..=now + 2).map(|s| identity_service::mfa::code_at(secret, s)).collect();
    (0..).map(|n| format!("{n:06}")).find(|c| !valid.contains(c)).unwrap()
}

#[tokio::test]
async fn two_step_verification_guards_sign_in() {
    use identity_service::mfa::code_at;
    let h = harness().await;
    let password = "a-long-enough-password";
    let registered = register(&h.app, "mfa@example.com").await;
    let me = token(&registered);
    let login = |body: Value| {
        let app = h.app.clone();
        async move { call(&app, Method::POST, "/api/v1/auth/login", Some(body), None, None).await }
    };
    let second_step = |mfa_token: &str, code: &str| {
        let (app, body) = (h.app.clone(), json!({ "mfa_token": mfa_token, "code": code }));
        async move { call(&app, Method::POST, "/api/v1/auth/login/mfa", Some(body), None, None).await }
    };
    let credentials = json!({ "email": "mfa@example.com", "password": password });
    // A second session, which turning two-step verification on will end.
    assert_eq!(login(credentials.clone()).await.status, StatusCode::OK);

    let status = call(&h.app, Method::GET, "/api/v1/auth/mfa", None, Some(&me), None).await;
    assert_eq!(status.body, json!({ "totp_enabled": false, "enabled_at": null, "recovery_codes_remaining": 0 }));

    // Set-up needs the password again.
    let bad =
        call(&h.app, Method::POST, "/api/v1/auth/mfa/totp/setup", Some(json!({ "password": "nope" })), Some(&me), None).await;
    assert_eq!(bad.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(bad.body["errors"][0]["field"], "password");
    sqlx::query("UPDATE accounts SET failed_login_count = 0").execute(&h.db.owner).await.unwrap();

    let setup =
        call(&h.app, Method::POST, "/api/v1/auth/mfa/totp/setup", Some(json!({ "password": password })), Some(&me), None).await;
    assert_eq!(setup.status, StatusCode::OK, "{}", setup.body);
    let encoded = setup.body["secret"].as_str().unwrap().to_owned();
    let secret = base32_decode(&encoded);
    assert_eq!(secret.len(), 20);
    assert!(setup.body["otpauth_uri"].as_str().unwrap().contains(&format!("secret={encoded}")));
    assert!(setup.body["qr_svg"].as_str().unwrap().contains("<svg"));
    // Not on until a code proves the app has the secret.
    assert_eq!(login(credentials.clone()).await.status, StatusCode::OK);
    // The secret is stored sealed, not in clear.
    let (stored,): (Vec<u8>,) = sqlx::query_as("SELECT secret_enc FROM totp_factors").fetch_one(&h.db.owner).await.unwrap();
    assert!(!stored.windows(secret.len()).any(|w| w == secret.as_slice()));

    let confirm = |code: String| {
        let app = h.app.clone();
        let me = me.clone();
        async move { call(&app, Method::POST, "/api/v1/auth/mfa/totp/confirm", Some(json!({ "code": code })), Some(&me), None).await }
    };
    let wrong = confirm(wrong_code(&secret)).await;
    assert_eq!(wrong.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(wrong.body["errors"][0]["field"], "code");
    let now = step_now();
    let enabled = confirm(code_at(&secret, now)).await;
    assert_eq!(enabled.status, StatusCode::OK, "{}", enabled.body);
    let recovery: Vec<String> = serde_json::from_value(enabled.body["codes"].clone()).unwrap();
    assert_eq!(recovery.len(), 10);

    // Every other session ended; this one carries on.
    let sessions = call(&h.app, Method::GET, "/api/v1/auth/sessions", None, Some(&me), None).await;
    assert_eq!(sessions.body["items"].as_array().unwrap().len(), 1);
    let status = call(&h.app, Method::GET, "/api/v1/auth/mfa", None, Some(&me), None).await;
    assert_eq!((status.body["totp_enabled"].as_bool(), status.body["recovery_codes_remaining"].as_i64()), (Some(true), Some(10)));
    let again =
        call(&h.app, Method::POST, "/api/v1/auth/mfa/totp/setup", Some(json!({ "password": password })), Some(&me), None).await;
    assert_eq!(again.status, StatusCode::CONFLICT);

    // The password alone now opens nothing.
    let challenged = login(credentials.clone()).await;
    assert_eq!(challenged.status, StatusCode::ACCEPTED);
    assert!(challenged.raw_set_cookie.is_none() && challenged.body.get("access_token").is_none());
    let mfa_token = challenged.body["mfa_token"].as_str().unwrap().to_owned();
    assert_eq!(second_step("forged", &code_at(&secret, now + 1)).await.status, StatusCode::UNAUTHORIZED);
    assert_eq!(second_step(&mfa_token, &wrong_code(&secret)).await.status, StatusCode::UNPROCESSABLE_ENTITY);
    let (failures,): (i32,) = sqlx::query_as("SELECT failed_login_count FROM accounts").fetch_one(&h.db.owner).await.unwrap();
    assert_eq!(failures, 1, "wrong codes count towards the lockout");

    let signed_in = second_step(&mfa_token, &code_at(&secret, now + 1)).await;
    assert_eq!(signed_in.status, StatusCode::OK, "{}", signed_in.body);
    assert!(signed_in.cookie.as_deref().is_some_and(|c| !c.is_empty()));
    assert_eq!(jwt_payload(&token(&signed_in))["email"], "mfa@example.com");
    // The challenge is single use, and so is the code.
    assert_eq!(second_step(&mfa_token, &code_at(&secret, now + 1)).await.status, StatusCode::UNAUTHORIZED);
    let next = login(credentials.clone()).await.body["mfa_token"].as_str().unwrap().to_owned();
    assert_eq!(second_step(&next, &code_at(&secret, now + 1)).await.status, StatusCode::UNPROCESSABLE_ENTITY, "no replay");

    // A recovery code works once, however it is typed.
    let typed = recovery[0].to_lowercase().replace('-', " ");
    assert_eq!(second_step(&next, &typed).await.status, StatusCode::OK);
    let next = login(credentials.clone()).await.body["mfa_token"].as_str().unwrap().to_owned();
    assert_eq!(second_step(&next, &recovery[0]).await.status, StatusCode::UNPROCESSABLE_ENTITY);

    // Five wrong codes (the reused one above was the first) and the attempt is over, even
    // for a right code.
    for attempt in 2..=4 {
        assert_eq!(second_step(&next, &wrong_code(&secret)).await.status, StatusCode::UNPROCESSABLE_ENTITY, "attempt {attempt}");
    }
    assert_eq!(second_step(&next, &wrong_code(&secret)).await.status, StatusCode::UNAUTHORIZED, "attempt 5");
    assert_eq!(second_step(&next, &recovery[1]).await.status, StatusCode::UNAUTHORIZED);
    // ...and those failures locked the account, so a fresh sign-in is refused for a while.
    assert_eq!(login(credentials.clone()).await.status, StatusCode::UNAUTHORIZED);
    sqlx::query("UPDATE accounts SET failed_login_count = 0, locked_until = NULL").execute(&h.db.owner).await.unwrap();

    // New recovery codes replace the old ones.
    let renewed = call(
        &h.app,
        Method::POST,
        "/api/v1/auth/mfa/recovery-codes",
        Some(json!({ "password": password, "code": recovery[1] })),
        Some(&me),
        None,
    )
    .await;
    assert_eq!(renewed.status, StatusCode::OK, "{}", renewed.body);
    let renewed: Vec<String> = serde_json::from_value(renewed.body["codes"].clone()).unwrap();
    let next = login(credentials.clone()).await.body["mfa_token"].as_str().unwrap().to_owned();
    assert_eq!(second_step(&next, &recovery[2]).await.status, StatusCode::UNPROCESSABLE_ENTITY, "old codes are gone");

    // Turning it off needs the password and a code.
    let off = |password: &str, code: &str| {
        let (app, me, body) = (h.app.clone(), me.clone(), json!({ "password": password, "code": code }));
        async move { call(&app, Method::POST, "/api/v1/auth/mfa/totp/disable", Some(body), Some(&me), None).await }
    };
    assert_eq!(off(password, &wrong_code(&secret)).await.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(off(password, &renewed[0]).await.status, StatusCode::NO_CONTENT);
    let status = call(&h.app, Method::GET, "/api/v1/auth/mfa", None, Some(&me), None).await;
    assert_eq!(status.body["totp_enabled"], false);
    assert_eq!(login(credentials).await.status, StatusCode::OK);

    let outcomes: Vec<(String,)> =
        sqlx::query_as("SELECT DISTINCT outcome FROM login_events ORDER BY outcome").fetch_all(&h.db.owner).await.unwrap();
    let outcomes: Vec<&str> = outcomes.iter().map(|(o,)| o.as_str()).collect();
    assert_eq!(outcomes, ["locked", "mfa_failed", "mfa_required", "success"]);
    h.db.cleanup().await;
}

#[tokio::test]
async fn api_tokens_are_exchanged_for_org_tokens_and_cannot_manage_credentials() {
    let h = harness().await;
    let registered = register(&h.app, "script@example.com").await;
    let user_id = Uuid::parse_str(registered.body["account"]["id"].as_str().unwrap()).unwrap();
    let (org, tenant) = (Uuid::now_v7(), Uuid::now_v7());
    h.membership
        .0
        .lock()
        .unwrap()
        .insert((user_id, org), MembershipInfo { tenant_id: tenant, role: OrgRole::Member, require_two_step: false });
    let session = token(
        &call(
            &h.app,
            Method::POST,
            "/api/v1/auth/token/exchange",
            Some(json!({ "organization_id": org })),
            Some(&token(&registered)),
            None,
        )
        .await,
    );

    // Tokens belong to an organization: an account-level session cannot make one.
    let no_org = call(
        &h.app,
        Method::POST,
        "/api/v1/auth/api-tokens",
        Some(json!({ "name": "ci", "expires_in_days": 30 })),
        Some(&token(&registered)),
        None,
    )
    .await;
    assert_eq!(no_org.status, StatusCode::UNAUTHORIZED);
    for bad in [
        json!({ "name": " ", "expires_in_days": 30 }),
        json!({ "name": "ci", "expires_in_days": 0 }),
        json!({ "name": "ci", "expires_in_days": 366 }),
    ] {
        let r = call(&h.app, Method::POST, "/api/v1/auth/api-tokens", Some(bad), Some(&session), None).await;
        assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY, "{}", r.body);
    }

    let created = call(
        &h.app,
        Method::POST,
        "/api/v1/auth/api-tokens",
        Some(json!({ "name": "ci", "expires_in_days": 30 })),
        Some(&session),
        None,
    )
    .await;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.body);
    let secret = created.body["secret"].as_str().unwrap().to_owned();
    let id = created.body["token"]["id"].as_str().unwrap().to_owned();
    assert!(secret.starts_with("mrd_"));
    assert!(secret.starts_with(created.body["token"]["hint"].as_str().unwrap()));
    // Only the hash is stored.
    let (stored,): (i64,) =
        sqlx::query_as("SELECT count(*) FROM api_tokens WHERE hint = $1 AND token_hash <> convert_to($2, 'UTF8')")
            .bind(created.body["token"]["hint"].as_str().unwrap())
            .bind(&secret)
            .fetch_one(&h.db.app)
            .await
            .unwrap();
    assert_eq!(stored, 1);

    let list = call(&h.app, Method::GET, "/api/v1/auth/api-tokens", None, Some(&session), None).await;
    assert_eq!(list.body["items"].as_array().unwrap().len(), 1);
    assert!(list.body["items"][0].get("secret").is_none());

    let exchanged = call(&h.app, Method::POST, "/api/v1/auth/token/api", Some(json!({ "token": secret })), None, None).await;
    assert_eq!(exchanged.status, StatusCode::OK, "{}", exchanged.body);
    let api = token(&exchanged);
    let claims = jwt_payload(&api);
    assert_eq!(claims["typ"], "org");
    assert_eq!(claims["tid"], tenant.to_string());
    assert_eq!(claims["roles"], json!(["member"]));
    assert_eq!(claims["api"], true);
    assert_eq!(claims["sid"], id);
    assert!(jwt_payload(&session).get("api").is_none());
    let used = call(&h.app, Method::GET, "/api/v1/auth/api-tokens", None, Some(&session), None).await;
    assert!(used.body["items"][0]["last_used_at"].is_string());

    // An API-derived token reaches the services but never credential management.
    for (method, path, body) in [
        (Method::GET, "/api/v1/auth/api-tokens".to_owned(), None),
        (Method::POST, "/api/v1/auth/api-tokens".to_owned(), Some(json!({ "name": "more", "expires_in_days": 1 }))),
        (Method::DELETE, format!("/api/v1/auth/api-tokens/{id}"), None),
        (Method::GET, "/api/v1/auth/sessions".to_owned(), None),
        (Method::GET, "/api/v1/auth/mfa".to_owned(), None),
        (Method::POST, "/api/v1/auth/token/exchange".to_owned(), Some(json!({ "organization_id": org }))),
    ] {
        let r = call(&h.app, method.clone(), &path, body, Some(&api), None).await;
        assert_eq!(r.status, StatusCode::FORBIDDEN, "{method} {path}: {}", r.body);
    }

    // Unknown, malformed and expired secrets look the same.
    for bad in ["mrd_nope", "not-a-token", &"mrd_x".repeat(30)] {
        let r = call(&h.app, Method::POST, "/api/v1/auth/token/api", Some(json!({ "token": bad })), None, None).await;
        assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    }
    let expiring = call(
        &h.app,
        Method::POST,
        "/api/v1/auth/api-tokens",
        Some(json!({ "name": "short", "expires_in_days": 1 })),
        Some(&session),
        None,
    )
    .await;
    sqlx::query("UPDATE api_tokens SET created_at = now() - interval '2 days', expires_at = now() - interval '1 day' WHERE name = 'short'")
        .execute(&h.db.app)
        .await
        .unwrap();
    let r =
        call(&h.app, Method::POST, "/api/v1/auth/token/api", Some(json!({ "token": expiring.body["secret"] })), None, None).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);

    // Removed from the organization: the token stops working at the next exchange.
    h.membership.0.lock().unwrap().clear();
    let r = call(&h.app, Method::POST, "/api/v1/auth/token/api", Some(json!({ "token": secret })), None, None).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    h.membership
        .0
        .lock()
        .unwrap()
        .insert((user_id, org), MembershipInfo { tenant_id: tenant, role: OrgRole::Member, require_two_step: false });

    // Someone else cannot revoke it; its owner can, and then it is dead.
    let mallory = register(&h.app, "mallory@example.com").await;
    let r = call(&h.app, Method::DELETE, &format!("/api/v1/auth/api-tokens/{id}"), None, Some(&token(&mallory)), None).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    let r = call(&h.app, Method::DELETE, &format!("/api/v1/auth/api-tokens/{id}"), None, Some(&session), None).await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    let r = call(&h.app, Method::POST, "/api/v1/auth/token/api", Some(json!({ "token": secret })), None, None).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    let list = call(&h.app, Method::GET, "/api/v1/auth/api-tokens", None, Some(&session), None).await;
    assert_eq!(list.body["items"].as_array().unwrap().len(), 0);

    // At most 25 active per person per organization.
    for i in 0..25 {
        let r = call(
            &h.app,
            Method::POST,
            "/api/v1/auth/api-tokens",
            Some(json!({ "name": format!("t{i}"), "expires_in_days": 7 })),
            Some(&session),
            None,
        )
        .await;
        assert_eq!(r.status, StatusCode::CREATED, "{i}: {}", r.body);
    }
    let r = call(
        &h.app,
        Method::POST,
        "/api/v1/auth/api-tokens",
        Some(json!({ "name": "one too many", "expires_in_days": 7 })),
        Some(&session),
        None,
    )
    .await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    h.db.cleanup().await;
}

#[tokio::test]
async fn a_workspace_can_require_two_step_verification() {
    let h = harness().await;
    let password = "a-long-enough-password";
    let registered = register(&h.app, "careful@example.com").await;
    let me = token(&registered);
    let cookie = registered.cookie.clone().unwrap();
    let user_id = Uuid::parse_str(registered.body["account"]["id"].as_str().unwrap()).unwrap();
    let (org, tenant) = (Uuid::now_v7(), Uuid::now_v7());
    let set_required = |required: bool| {
        h.membership
            .0
            .lock()
            .unwrap()
            .insert((user_id, org), MembershipInfo { tenant_id: tenant, role: OrgRole::Member, require_two_step: required });
    };
    let exchange = |bearer: String| {
        let app = h.app.clone();
        async move {
            call(&app, Method::POST, "/api/v1/auth/token/exchange", Some(json!({ "organization_id": org })), Some(&bearer), None)
                .await
        }
    };

    // Before the requirement: a workspace token and an API token.
    set_required(false);
    let session = exchange(me.clone()).await;
    assert_eq!(session.status, StatusCode::OK);
    let created = call(
        &h.app,
        Method::POST,
        "/api/v1/auth/api-tokens",
        Some(json!({ "name": "ci", "expires_in_days": 30 })),
        Some(&token(&session)),
        None,
    )
    .await;
    assert_eq!(created.status, StatusCode::CREATED);
    let secret = created.body["secret"].clone();

    // The workspace starts requiring it: no new workspace token of any kind until it is on.
    set_required(true);
    let refused = exchange(me.clone()).await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN);
    assert_eq!(refused.body["code"], "two_step_required", "{}", refused.body);
    let api = call(&h.app, Method::POST, "/api/v1/auth/token/api", Some(json!({ "token": secret })), None, None).await;
    assert_eq!(api.body["code"], "two_step_required");
    let more = call(
        &h.app,
        Method::POST,
        "/api/v1/auth/api-tokens",
        Some(json!({ "name": "more", "expires_in_days": 30 })),
        Some(&token(&session)),
        None,
    )
    .await;
    assert_eq!(more.body["code"], "two_step_required");
    // Refresh still signs the person in, to their account, so they can turn it on.
    let refreshed =
        call(&h.app, Method::POST, "/api/v1/auth/token/refresh", Some(json!({ "organization_id": org })), None, Some(&cookie))
            .await;
    assert_eq!(refreshed.status, StatusCode::OK, "{}", refreshed.body);
    assert!(refreshed.body["organization"].is_null());
    let cookie = refreshed.cookie.clone().unwrap();

    let setup =
        call(&h.app, Method::POST, "/api/v1/auth/mfa/totp/setup", Some(json!({ "password": password })), Some(&me), None).await;
    let secret_bytes = base32_decode(setup.body["secret"].as_str().unwrap());
    let code = identity_service::mfa::code_at(&secret_bytes, step_now());
    let on = call(&h.app, Method::POST, "/api/v1/auth/mfa/totp/confirm", Some(json!({ "code": code })), Some(&me), None).await;
    assert_eq!(on.status, StatusCode::OK, "{}", on.body);

    // Now all three work again.
    assert_eq!(exchange(me.clone()).await.status, StatusCode::OK);
    let api = call(&h.app, Method::POST, "/api/v1/auth/token/api", Some(json!({ "token": secret })), None, None).await;
    assert_eq!(api.status, StatusCode::OK, "{}", api.body);
    let refreshed =
        call(&h.app, Method::POST, "/api/v1/auth/token/refresh", Some(json!({ "organization_id": org })), None, Some(&cookie))
            .await;
    assert_eq!(refreshed.body["organization"]["id"], org.to_string(), "{}", refreshed.body);
    h.db.cleanup().await;
}

/// Register, make a member of `org` with `role`, and return (account id, workspace session token).
async fn member(h: &Harness, email: &str, org: Uuid, tenant: Uuid, role: OrgRole) -> (Uuid, String) {
    let registered = register(&h.app, email).await;
    let id = Uuid::parse_str(registered.body["account"]["id"].as_str().unwrap()).unwrap();
    h.membership.0.lock().unwrap().insert((id, org), MembershipInfo { tenant_id: tenant, role, require_two_step: false });
    let exchanged = call(
        &h.app,
        Method::POST,
        "/api/v1/auth/token/exchange",
        Some(json!({ "organization_id": org })),
        Some(&token(&registered)),
        None,
    )
    .await;
    (id, token(&exchanged))
}

#[tokio::test]
async fn admins_see_members_two_step_and_manage_workspace_api_tokens() {
    let h = harness().await;
    let (org, tenant) = (Uuid::now_v7(), Uuid::now_v7());
    let (_, admin) = member(&h, "admin@example.com", org, tenant, OrgRole::Admin).await;
    let (alice, alice_session) = member(&h, "alice@example.com", org, tenant, OrgRole::Member).await;
    let (bob, _) = member(&h, "bob@example.com", org, tenant, OrgRole::Member).await;
    // Someone in another workspace only.
    let (other_org, other_tenant) = (Uuid::now_v7(), Uuid::now_v7());
    let (outsider, outsider_session) = member(&h, "out@example.com", other_org, other_tenant, OrgRole::Owner).await;
    for who in [alice, outsider] {
        sqlx::query(
            "INSERT INTO totp_factors (account_id, secret_enc, nonce, confirmed_at) VALUES ($1, '\\x00', '\\x00', now())",
        )
        .bind(who)
        .execute(&h.db.owner)
        .await
        .unwrap();
    }

    // Two-step status: admins only, and only for the workspace's own members.
    let ask = json!({ "user_ids": [alice, bob, outsider] });
    let refused = call(&h.app, Method::POST, "/api/v1/auth/admin/two-step", Some(ask.clone()), Some(&alice_session), None).await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN);
    let status = call(&h.app, Method::POST, "/api/v1/auth/admin/two-step", Some(ask), Some(&admin), None).await;
    assert_eq!(status.status, StatusCode::OK, "{}", status.body);
    let mut got: Vec<(String, bool)> = status.body["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| (i["user_id"].as_str().unwrap().to_owned(), i["enabled"].as_bool().unwrap()))
        .collect();
    got.sort();
    let mut want = vec![(alice.to_string(), true), (bob.to_string(), false)];
    want.sort();
    assert_eq!(got, want, "the outsider is left out");
    let too_many = json!({ "user_ids": vec![Uuid::nil(); 501] });
    assert_eq!(
        call(&h.app, Method::POST, "/api/v1/auth/admin/two-step", Some(too_many), Some(&admin), None).await.status,
        StatusCode::UNPROCESSABLE_ENTITY
    );

    // API tokens: the workspace's own, listed and revocable by its admins, and audited.
    let mine = call(
        &h.app,
        Method::POST,
        "/api/v1/auth/api-tokens",
        Some(json!({ "name": "ci", "expires_in_days": 30 })),
        Some(&alice_session),
        None,
    )
    .await;
    let theirs = call(
        &h.app,
        Method::POST,
        "/api/v1/auth/api-tokens",
        Some(json!({ "name": "other", "expires_in_days": 30 })),
        Some(&outsider_session),
        None,
    )
    .await;
    let (mine_id, theirs_id) =
        (mine.body["token"]["id"].as_str().unwrap().to_owned(), theirs.body["token"]["id"].as_str().unwrap().to_owned());
    assert_eq!(
        call(&h.app, Method::GET, "/api/v1/auth/admin/api-tokens", None, Some(&alice_session), None).await.status,
        StatusCode::FORBIDDEN
    );
    let listed = call(&h.app, Method::GET, "/api/v1/auth/admin/api-tokens", None, Some(&admin), None).await;
    let items = listed.body["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(
        (items[0]["id"].as_str(), items[0]["owner_id"].as_str()),
        (Some(mine_id.as_str()), Some(alice.to_string().as_str()))
    );
    assert!(items[0].get("secret").is_none() && items[0].get("token_hash").is_none());

    let other =
        call(&h.app, Method::DELETE, &format!("/api/v1/auth/admin/api-tokens/{theirs_id}"), None, Some(&admin), None).await;
    assert_eq!(other.status, StatusCode::NOT_FOUND, "another workspace's token is out of reach");
    let revoked =
        call(&h.app, Method::DELETE, &format!("/api/v1/auth/admin/api-tokens/{mine_id}"), None, Some(&admin), None).await;
    assert_eq!(revoked.status, StatusCode::NO_CONTENT);
    let secret = mine.body["secret"].as_str().unwrap();
    let exchange = call(&h.app, Method::POST, "/api/v1/auth/token/api", Some(json!({ "token": secret })), None, None).await;
    assert_eq!(exchange.status, StatusCode::UNAUTHORIZED, "a revoked token no longer works");
    let audit: (Option<Uuid>, Value) = sqlx::query_as(
        "SELECT tenant_id, payload->'payload' FROM outbox WHERE event_type = 'audit.recorded' ORDER BY created_at DESC LIMIT 1",
    )
    .fetch_one(&h.db.owner)
    .await
    .unwrap();
    assert_eq!(audit.0, Some(tenant));
    assert_eq!((audit.1["action"].as_str(), audit.1["resource_label"].as_str()), (Some("api_token.revoked"), Some("ci")));
    assert_eq!(audit.1["details"]["owner_id"], alice.to_string());
    h.db.cleanup().await;
}
