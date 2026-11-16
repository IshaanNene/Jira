//! Verifying an address and resetting a password, against a real PostgreSQL, with the mail
//! server replaced by a recorder.

use std::{
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::Duration,
};

use async_trait::async_trait;
use auth_middleware::RedisHandle;
use axum::{
    body::Body,
    extract::connect_info::MockConnectInfo,
    http::{header, Method, Request, StatusCode},
    Router,
};
use base64::Engine;
use error_handling::ApiResult;
use identity_service::{
    account_email::AccountMail,
    crypto::KeySealer,
    http,
    membership::{MembershipChecker, MembershipInfo},
    AppState, Settings, MIGRATOR,
};
use serde_json::{json, Value};
use smtp_mailer::{Mailer, Outgoing, SendError};
use test_utilities::TestDb;
use tower::ServiceExt;
use uuid::Uuid;

const ORIGIN: &str = "http://localhost:5173";
const PASSWORD: &str = "a-long-enough-password";

struct NoMembership;

#[async_trait]
impl MembershipChecker for NoMembership {
    async fn check(&self, _: Uuid, _: Uuid) -> ApiResult<Option<MembershipInfo>> {
        Ok(None)
    }
    async fn active_members(&self, _: Uuid, _: &[Uuid]) -> ApiResult<Vec<Uuid>> {
        Ok(Vec::new())
    }
}

/// (to, subject, text) of every email sent.
#[derive(Default)]
struct Outbox(Mutex<Vec<(String, String, String)>>);

#[async_trait]
impl Mailer for Outbox {
    async fn send(&self, email: &Outgoing<'_>) -> Result<(), SendError> {
        self.0.lock().unwrap().push((email.to.to_owned(), email.subject.to_owned(), email.text.to_owned()));
        Ok(())
    }
}

struct Harness {
    db: TestDb,
    app: Router,
    outbox: Arc<Outbox>,
}

async fn harness(with_mail: bool) -> Harness {
    let db = TestDb::create(&MIGRATOR).await.expect("test database");
    let sealer = KeySealer::from_base64(&base64::engine::general_purpose::STANDARD.encode([1_u8; 32])).unwrap();
    let settings = Settings {
        issuer: "http://identity.test".into(),
        allowed_origins: vec![ORIGIN.into()],
        cookie_secure: false,
        session_ttl_days: 30,
        trust_forwarded_for: false,
    };
    let mut state =
        AppState::build(db.app.clone(), settings, sealer, Arc::new(NoMembership), RedisHandle::default()).await.unwrap();
    let outbox = Arc::new(Outbox::default());
    if with_mail {
        state.mail = Some(Arc::new(AccountMail { mailer: outbox.clone(), app_url: "https://app.example.com".into() }));
    }
    let app = http::router(state).layer(MockConnectInfo(SocketAddr::from(([127, 0, 0, 1], 4000))));
    Harness { db, app, outbox }
}

struct Reply {
    status: StatusCode,
    body: Value,
    cookie: Option<String>,
}

impl Harness {
    async fn call(&self, method: Method, path: &str, body: Option<Value>, bearer: Option<&str>, cookie: Option<&str>) -> Reply {
        let mut req = Request::builder()
            .method(method)
            .uri(path)
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::ORIGIN, ORIGIN);
        if let Some(token) = bearer {
            req = req.header(header::AUTHORIZATION, format!("Bearer {token}"));
        }
        if let Some(value) = cookie {
            req = req.header(header::COOKIE, format!("refresh_token={value}"));
        }
        let response = self
            .app
            .clone()
            .oneshot(req.body(body.map_or_else(Body::empty, |b| Body::from(b.to_string()))).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let cookie = response
            .headers()
            .get(header::SET_COOKIE)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(';').next())
            .and_then(|kv| kv.strip_prefix("refresh_token="))
            .map(str::to_owned);
        let bytes = axum::body::to_bytes(response.into_body(), 1 << 20).await.unwrap();
        Reply { status, body: serde_json::from_slice(&bytes).unwrap_or(Value::Null), cookie }
    }

    async fn post(&self, path: &str, body: Value) -> Reply {
        self.call(Method::POST, path, Some(body), None, None).await
    }

    async fn login(&self, email: &str, password: &str) -> Reply {
        self.post("/api/v1/auth/login", json!({ "email": email, "password": password })).await
    }

    /// The emails sent to `to` so far, after giving background sends a moment.
    async fn mails_to(&self, to: &str, expected: usize) -> Vec<(String, String)> {
        for _ in 0..50 {
            let found: Vec<_> = self
                .outbox
                .0
                .lock()
                .unwrap()
                .iter()
                .filter(|(t, _, _)| t == to)
                .map(|(_, s, b)| (s.clone(), b.clone()))
                .collect();
            if found.len() >= expected {
                return found;
            }
            tokio::time::sleep(Duration::from_millis(40)).await;
        }
        self.outbox.0.lock().unwrap().iter().filter(|(t, _, _)| t == to).map(|(_, s, b)| (s.clone(), b.clone())).collect()
    }

    /// Let the clock pass the per-account interval between links.
    async fn age_links(&self) {
        sqlx::query("UPDATE email_tokens SET created_at = created_at - interval '2 minutes'")
            .execute(&self.db.owner)
            .await
            .unwrap();
    }
}

fn link_token(text: &str) -> String {
    text.split("token=").nth(1).unwrap().split_whitespace().next().unwrap().to_owned()
}

fn verified_claim(access_token: &str) -> Value {
    let payload = access_token.split('.').nth(1).unwrap();
    let claims: Value =
        serde_json::from_slice(&base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload).unwrap()).unwrap();
    claims["email_verified"].clone()
}

#[tokio::test]
async fn registering_sends_a_single_use_verification_link() {
    let h = harness(true).await;
    let email = "ada@example.com";
    let reply = h
        .post("/api/v1/auth/register", json!({ "email": email, "password": PASSWORD, "display_name": "Ada <b>Lovelace</b>" }))
        .await;
    assert_eq!(reply.status, StatusCode::CREATED);
    let access = reply.body["access_token"].as_str().unwrap().to_owned();
    assert_eq!(verified_claim(&access), false);
    assert_eq!(reply.body["account"]["email_verified"], false);

    let mails = h.mails_to(email, 1).await;
    assert_eq!(mails.len(), 1);
    assert_eq!(mails[0].0, "Verify your email address");
    assert!(mails[0].1.contains("https://app.example.com/verify-email?token="));
    let first = link_token(&mails[0].1);

    // Asking again straight away is refused; later it sends a new link.
    let again = h.call(Method::POST, "/api/v1/auth/email/verification", None, Some(&access), None).await;
    assert_eq!(again.status, StatusCode::TOO_MANY_REQUESTS);
    h.age_links().await;
    assert_eq!(
        h.call(Method::POST, "/api/v1/auth/email/verification", None, Some(&access), None).await.status,
        StatusCode::ACCEPTED
    );
    let mails = h.mails_to(email, 2).await;
    let second = link_token(&mails[1].1);
    assert_ne!(first, second);

    // Wrong tokens change nothing; the first good one verifies and spends the others.
    assert_eq!(
        h.post("/api/v1/auth/email/verify", json!({ "token": "nonsense" })).await.status,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        h.post("/api/v1/auth/password/reset", json!({ "token": second, "password": "another-good-password" })).await.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "a verification link is not a reset link"
    );
    assert_eq!(h.post("/api/v1/auth/email/verify", json!({ "token": second })).await.status, StatusCode::NO_CONTENT);
    assert_eq!(h.post("/api/v1/auth/email/verify", json!({ "token": second })).await.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(h.post("/api/v1/auth/email/verify", json!({ "token": first })).await.status, StatusCode::UNPROCESSABLE_ENTITY);

    let login = h.login(email, PASSWORD).await;
    assert_eq!(verified_claim(login.body["access_token"].as_str().unwrap()), true);
    assert_eq!(login.body["account"]["email_verified"], true);
    let access = login.body["access_token"].as_str().unwrap();
    assert_eq!(
        h.call(Method::POST, "/api/v1/auth/email/verification", None, Some(access), None).await.status,
        StatusCode::NO_CONTENT
    );

    // A link is for the address it was sent to.
    let bob = "bob@example.com";
    h.post("/api/v1/auth/register", json!({ "email": bob, "password": PASSWORD, "display_name": "Bob" })).await;
    let token = link_token(&h.mails_to(bob, 1).await[0].1);
    sqlx::query("UPDATE accounts SET email = 'bob2@example.com' WHERE email = $1").bind(bob).execute(&h.db.owner).await.unwrap();
    assert_eq!(h.post("/api/v1/auth/email/verify", json!({ "token": token })).await.status, StatusCode::UNPROCESSABLE_ENTITY);
    let stored: i64 = sqlx::query_scalar("SELECT count(*) FROM email_tokens WHERE token_hash = sha256($1::bytea)")
        .bind(token.as_bytes())
        .fetch_one(&h.db.owner)
        .await
        .unwrap();
    assert_eq!(stored, 1, "only the hash is stored");
    h.db.cleanup().await;
}

#[tokio::test]
async fn a_reset_link_sets_a_new_password_and_ends_every_session() {
    let h = harness(true).await;
    let email = "maya@example.com";
    let registered =
        h.post("/api/v1/auth/register", json!({ "email": email, "password": PASSWORD, "display_name": "Maya" })).await;
    let old_cookie = registered.cookie.clone().unwrap();
    // Locked out by wrong passwords.
    for _ in 0..5 {
        h.login(email, "wrong-password-here").await;
    }
    assert_eq!(h.login(email, PASSWORD).await.status, StatusCode::UNAUTHORIZED);

    // Unknown addresses get the same answer and no email.
    assert_eq!(
        h.post("/api/v1/auth/password/forgot", json!({ "email": "nobody@example.com" })).await.status,
        StatusCode::ACCEPTED
    );
    assert_eq!(
        h.post("/api/v1/auth/password/forgot", json!({ "email": " MAYA@example.com " })).await.status,
        StatusCode::ACCEPTED
    );
    let mails = h.mails_to(email, 2).await;
    let reset: Vec<_> = mails.iter().filter(|(s, _)| s == "Reset your password").collect();
    assert_eq!(reset.len(), 1);
    assert!(reset[0].1.contains("https://app.example.com/reset-password?token="));
    let token = link_token(&reset[0].1);
    // A second request straight away sends nothing more.
    assert_eq!(h.post("/api/v1/auth/password/forgot", json!({ "email": email })).await.status, StatusCode::ACCEPTED);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(h.mails_to(email, 3).await.iter().filter(|(s, _)| s == "Reset your password").count(), 1);
    assert!(h.outbox.0.lock().unwrap().iter().all(|(to, _, _)| to != "nobody@example.com"));

    let short = h.post("/api/v1/auth/password/reset", json!({ "token": token, "password": "short" })).await;
    assert_eq!(short.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(short.body["errors"][0]["field"], "password");
    let new_password = "a-brand-new-password";
    assert_eq!(
        h.post("/api/v1/auth/password/reset", json!({ "token": token, "password": new_password })).await.status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        h.post("/api/v1/auth/password/reset", json!({ "token": token, "password": new_password })).await.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "single use"
    );

    // The old session is over, the lockout is gone, the old password is not accepted, and the
    // address counts as verified.
    let refreshed = h.call(Method::POST, "/api/v1/auth/token/refresh", Some(json!({})), None, Some(&old_cookie)).await;
    assert_eq!(refreshed.status, StatusCode::UNAUTHORIZED);
    assert_eq!(h.login(email, PASSWORD).await.status, StatusCode::UNAUTHORIZED);
    let login = h.login(email, new_password).await;
    assert_eq!(login.status, StatusCode::OK);
    assert_eq!(login.body["account"]["email_verified"], true);
    h.db.cleanup().await;
}

#[tokio::test]
async fn without_a_mail_server_links_cannot_be_requested() {
    let h = harness(false).await;
    let reply =
        h.post("/api/v1/auth/register", json!({ "email": "x@example.com", "password": PASSWORD, "display_name": "X" })).await;
    assert_eq!(reply.status, StatusCode::CREATED);
    let access = reply.body["access_token"].as_str().unwrap();
    assert_eq!(
        h.call(Method::POST, "/api/v1/auth/email/verification", None, Some(access), None).await.status,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(
        h.post("/api/v1/auth/password/forgot", json!({ "email": "x@example.com" })).await.status,
        StatusCode::SERVICE_UNAVAILABLE
    );
    h.db.cleanup().await;
}

#[tokio::test]
async fn changing_the_address_needs_the_password_and_a_link_sent_to_the_new_one() {
    let h = harness(true).await;
    let (old, new) = ("ada@example.com", "ada@new.example");
    let reply = h.post("/api/v1/auth/register", json!({ "email": old, "password": PASSWORD, "display_name": "Ada" })).await;
    let access = reply.body["access_token"].as_str().unwrap().to_owned();
    let verify_link = link_token(&h.mails_to(old, 1).await[0].1);
    h.post("/api/v1/auth/register", json!({ "email": "taken@example.com", "password": PASSWORD, "display_name": "T" })).await;
    let change = |body: Value| async { h.call(Method::POST, "/api/v1/auth/email/change", Some(body), Some(&access), None).await };

    // The password, a real and different address, and one that is free.
    assert_eq!(change(json!({ "new_email": new, "password": "wrong-password" })).await.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        change(json!({ "new_email": "not an address", "password": PASSWORD })).await.status,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        change(json!({ "new_email": " ADA@example.com", "password": PASSWORD })).await.status,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(change(json!({ "new_email": "taken@example.com", "password": PASSWORD })).await.status, StatusCode::CONFLICT);
    assert_eq!(
        h.call(Method::POST, "/api/v1/auth/email/change", Some(json!({ "new_email": new, "password": PASSWORD })), None, None)
            .await
            .status,
        StatusCode::UNAUTHORIZED
    );

    // A first request, then one for another address: only the latest link works.
    assert_eq!(change(json!({ "new_email": "first@new.example", "password": PASSWORD })).await.status, StatusCode::ACCEPTED);
    assert_eq!(change(json!({ "new_email": new, "password": PASSWORD })).await.status, StatusCode::TOO_MANY_REQUESTS);
    h.age_links().await;
    assert_eq!(change(json!({ "new_email": new, "password": PASSWORD })).await.status, StatusCode::ACCEPTED);
    let stale = link_token(&h.mails_to("first@new.example", 1).await[0].1);
    let mails = h.mails_to(new, 1).await;
    assert_eq!(mails[0].0, "Confirm your new email address");
    assert!(mails[0].1.contains("https://app.example.com/confirm-email?token="));
    let link = link_token(&mails[0].1);
    assert_eq!(
        h.post("/api/v1/auth/email/change/confirm", json!({ "token": stale })).await.status,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert!(h.mails_to(old, 2).await.len() == 1, "nothing has changed yet");

    // Confirming moves the account; the old address is told, without the whole new one.
    assert_eq!(h.post("/api/v1/auth/email/change/confirm", json!({ "token": link })).await.status, StatusCode::NO_CONTENT);
    assert_eq!(
        h.post("/api/v1/auth/email/change/confirm", json!({ "token": link })).await.status,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let notice = h.mails_to(old, 2).await;
    assert_eq!(notice[1].0, "Your email address was changed");
    assert!(notice[1].1.contains("a•••@new.example") && !notice[1].1.contains(new));
    assert_eq!(h.login(old, PASSWORD).await.status, StatusCode::UNAUTHORIZED);
    let login = h.login(new, PASSWORD).await;
    assert_eq!(login.status, StatusCode::OK);
    assert_eq!(login.body["account"]["email"], new);
    assert_eq!(verified_claim(login.body["access_token"].as_str().unwrap()), true, "the link proved the new address");
    // Links sent to the old address died with it.
    assert_eq!(
        h.post("/api/v1/auth/email/verify", json!({ "token": verify_link })).await.status,
        StatusCode::UNPROCESSABLE_ENTITY
    );

    // Other services learn the new address, and the one it replaced.
    let (payload,): (Value,) =
        sqlx::query_as("SELECT payload->'payload' FROM outbox WHERE event_type = 'identity.email_changed'")
            .fetch_one(&h.db.owner)
            .await
            .unwrap();
    assert_eq!((payload["email"].as_str(), payload["previous_email"].as_str()), (Some(new), Some(old)));
    h.db.cleanup().await;
}
