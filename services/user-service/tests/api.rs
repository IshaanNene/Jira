//! HTTP + database tests for the user/organization service against a real PostgreSQL,
//! connected as an unprivileged role so row-level security is genuinely enforced.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use api_contracts::user::v1::{membership_server::Membership, CheckMembershipRequest, FindMembersRequest, GetMembersRequest};
use async_trait::async_trait;
use axum::{
    body::Body,
    http::{header, Method, Request, StatusCode},
    Router,
};
use error_handling::ApiResult;
use serde_json::{json, Value};
use tenant_context::DbRouter;
use test_utilities::{TestAuth, TestDb};
use tower::ServiceExt;
use user_service::{grpc::MembershipService, http, tenants::TenantProvisioner, AppState, Settings, MIGRATOR};
use uuid::Uuid;

#[derive(Default)]
struct FakeProvisioner {
    abandoned: Mutex<Vec<Uuid>>,
}

#[async_trait]
impl TenantProvisioner for FakeProvisioner {
    async fn provision(&self, _organization_id: Uuid) -> ApiResult<Uuid> {
        Ok(Uuid::now_v7())
    }
    async fn abandon(&self, tenant_id: Uuid) {
        self.abandoned.lock().unwrap().push(tenant_id);
    }
}

struct Harness {
    db: TestDb,
    app: Router,
    auth: TestAuth,
    router: Arc<DbRouter>,
    provisioner: Arc<FakeProvisioner>,
    outbox: Arc<MailRecorder>,
}

/// (to, subject, text) of every email sent.
#[derive(Default)]
struct MailRecorder(Mutex<Vec<(String, String, String)>>);

#[async_trait]
impl smtp_mailer::Mailer for MailRecorder {
    async fn send(&self, email: &smtp_mailer::Outgoing<'_>) -> Result<(), smtp_mailer::SendError> {
        self.0.lock().unwrap().push((email.to.to_owned(), email.subject.to_owned(), email.text.to_owned()));
        Ok(())
    }
}

struct User {
    id: Uuid,
    email: String,
}

impl User {
    fn new(name: &str) -> Self {
        Self { id: Uuid::now_v7(), email: format!("{name}@example.com") }
    }
}

struct Reply {
    status: StatusCode,
    body: Value,
    etag: Option<String>,
}

async fn harness() -> Harness {
    let db = TestDb::create(&MIGRATOR).await.expect("test database");
    let auth = TestAuth::new().unwrap();
    let router = Arc::new(DbRouter::single(db.app.clone()));
    let provisioner = Arc::new(FakeProvisioner::default());
    let outbox = Arc::new(MailRecorder::default());
    let state = AppState {
        db: Arc::clone(&router),
        verifier: Arc::clone(&auth.verifier),
        tenants: provisioner.clone(),
        settings: Arc::new(Settings {
            allowed_origins: vec![],
            web_base_url: "http://web.test".into(),
            scim_base_url: "http://api.test/scim/v2".into(),
        }),
        mail: Some(Arc::new(user_service::invite_mail::InviteMail { mailer: outbox.clone() })),
    };
    Harness { db, app: http::router(state), auth, router, provisioner, outbox }
}

impl Harness {
    async fn call(&self, method: Method, path: &str, token: Option<&str>, body: Option<Value>, if_match: Option<&str>) -> Reply {
        let mut req = Request::builder().method(method).uri(path).header(header::CONTENT_TYPE, "application/json");
        if let Some(t) = token {
            req = req.header(header::AUTHORIZATION, format!("Bearer {t}"));
        }
        if let Some(v) = if_match {
            req = req.header(header::IF_MATCH, v);
        }
        let body = body.map_or_else(Body::empty, |b| Body::from(b.to_string()));
        let response = self.app.clone().oneshot(req.body(body).unwrap()).await.unwrap();
        let status = response.status();
        let etag = response.headers().get(header::ETAG).map(|v| v.to_str().unwrap().to_owned());
        let bytes = axum::body::to_bytes(response.into_body(), 1 << 20).await.unwrap();
        Reply { status, body: serde_json::from_slice(&bytes).unwrap_or(Value::Null), etag }
    }

    fn account(&self, user: &User) -> String {
        self.auth.account_token(user.id, &user.email).unwrap()
    }

    /// Create an organization as `owner`; returns (organization_id, tenant_id).
    async fn create_org(&self, owner: &User, name: &str) -> (Uuid, Uuid) {
        let reply = self
            .call(Method::POST, "/api/v1/organizations", Some(&self.account(owner)), Some(json!({ "name": name })), None)
            .await;
        assert_eq!(reply.status, StatusCode::CREATED, "{}", reply.body);
        let org_id = Uuid::parse_str(reply.body["id"].as_str().unwrap()).unwrap();
        let (tenant_id,): (Uuid,) = sqlx::query_as("SELECT tenant_id FROM organizations WHERE id = $1")
            .bind(org_id)
            .fetch_one(&self.db.owner)
            .await
            .unwrap();
        (org_id, tenant_id)
    }

    fn org_token(&self, user: &User, tenant: Uuid, org: Uuid, role: &str) -> String {
        self.auth.org_token(user.id, &user.email, tenant, org, role).unwrap()
    }

    /// Invite `invitee` and accept as them; returns nothing, asserts success.
    async fn add_member(&self, admin_token: &str, invitee: &User, role: &str) {
        let invited = self
            .call(
                Method::POST,
                "/api/v1/organizations/current/invitations",
                Some(admin_token),
                Some(json!({ "email": invitee.email, "org_role": role })),
                None,
            )
            .await;
        assert_eq!(invited.status, StatusCode::CREATED, "{}", invited.body);
        let token = invited.body["accept_url"].as_str().unwrap().rsplit('/').next().unwrap().to_owned();
        let accepted = self
            .call(Method::POST, &format!("/api/v1/invitations/{token}/accept"), Some(&self.account(invitee)), None, None)
            .await;
        assert_eq!(accepted.status, StatusCode::OK, "{}", accepted.body);
    }
}

#[tokio::test]
async fn create_organization_makes_caller_owner_and_emits_events() {
    let h = harness().await;
    let ada = User::new("ada");
    let (org, tenant) = h.create_org(&ada, "Acme Rockets, Inc.").await;

    let mine = h.call(Method::GET, "/api/v1/organizations", Some(&h.account(&ada)), None, None).await;
    assert_eq!(mine.body["items"][0]["slug"], "acme-rockets-inc");
    assert_eq!(mine.body["items"][0]["role"], "owner");

    let current =
        h.call(Method::GET, "/api/v1/organizations/current", Some(&h.org_token(&ada, tenant, org, "owner")), None, None).await;
    assert_eq!(current.status, StatusCode::OK);
    assert_eq!(current.etag.as_deref(), Some("\"1\""));

    // An account token (no organization selected) cannot read tenant data.
    let no_org = h.call(Method::GET, "/api/v1/organizations/current", Some(&h.account(&ada)), None, None).await;
    assert_eq!(no_org.status, StatusCode::UNAUTHORIZED);

    let events: Vec<(String, Uuid)> = sqlx::query_as("SELECT event_type, tenant_id FROM outbox ORDER BY created_at, event_type")
        .fetch_all(&h.db.owner)
        .await
        .unwrap();
    assert_eq!(events, vec![("membership.changed".into(), tenant), ("organization.created".into(), tenant)]);
    h.db.cleanup().await;
}

#[tokio::test]
async fn slug_conflict_is_reported_and_the_tenant_is_abandoned() {
    let h = harness().await;
    let (ada, bob) = (User::new("ada"), User::new("bob"));
    h.create_org(&ada, "Acme").await;
    let dup = h.call(Method::POST, "/api/v1/organizations", Some(&h.account(&bob)), Some(json!({ "name": "ACME" })), None).await;
    assert_eq!(dup.status, StatusCode::CONFLICT);
    assert_eq!(h.provisioner.abandoned.lock().unwrap().len(), 1, "the provisioned tenant is compensated");
    let (orgs,): (i64,) = sqlx::query_as("SELECT count(*) FROM organizations").fetch_one(&h.db.owner).await.unwrap();
    assert_eq!(orgs, 1);
    let mine = h.call(Method::GET, "/api/v1/organizations", Some(&h.account(&bob)), None, None).await;
    assert_eq!(mine.body["items"], json!([]));

    for bad in [
        json!({ "name": "x" }),
        json!({ "name": "Fine Name", "slug": "admin" }),
        json!({ "name": "Fine Name", "slug": "Not Valid" }),
    ] {
        let reply = h.call(Method::POST, "/api/v1/organizations", Some(&h.account(&bob)), Some(bad), None).await;
        assert_eq!(reply.status, StatusCode::UNPROCESSABLE_ENTITY);
    }
    h.db.cleanup().await;
}

#[tokio::test]
async fn row_level_security_isolates_tenants_in_the_database() {
    let h = harness().await;
    let (ada, bob) = (User::new("ada"), User::new("bob"));
    let (_, tenant_a) = h.create_org(&ada, "Tenant A").await;
    let (org_b, tenant_b) = h.create_org(&bob, "Tenant B").await;

    // No tenant set: the runtime role sees nothing at all.
    let (visible,): (i64,) = sqlx::query_as("SELECT count(*) FROM organizations").fetch_one(&h.db.app).await.unwrap();
    assert_eq!(visible, 0);
    let (visible,): (i64,) = sqlx::query_as("SELECT count(*) FROM memberships").fetch_one(&h.db.app).await.unwrap();
    assert_eq!(visible, 0);

    // Inside tenant A's scope only tenant A exists, even for queries with no tenant predicate.
    let mut tx = h.router.begin(tenant_a).await.unwrap();
    let names: Vec<(String,)> = sqlx::query_as("SELECT name FROM organizations").fetch_all(tx.conn()).await.unwrap();
    assert_eq!(names, vec![("Tenant A".to_owned(),)]);

    // Writing another tenant's rows from tenant A's scope is impossible.
    let updated = sqlx::query("UPDATE organizations SET name = 'pwned' WHERE tenant_id = $1")
        .bind(tenant_b)
        .execute(tx.conn())
        .await
        .unwrap();
    assert_eq!(updated.rows_affected(), 0);
    let deleted = sqlx::query("DELETE FROM memberships").execute(tx.conn()).await.unwrap();
    assert_eq!(deleted.rows_affected(), 1, "only tenant A's single membership is reachable");
    tx.rollback().await.unwrap();

    let mut tx = h.router.begin(tenant_a).await.unwrap();
    let smuggled =
        sqlx::query("INSERT INTO memberships (tenant_id, user_id, organization_id, org_role) VALUES ($1, $2, $3, 'owner')")
            .bind(tenant_b)
            .bind(ada.id)
            .bind(org_b)
            .execute(tx.conn())
            .await;
    assert!(smuggled.is_err(), "WITH CHECK rejects rows for another tenant");
    drop(tx);

    // The setting is transaction-local: the pooled connection carries nothing over.
    let (visible,): (i64,) = sqlx::query_as("SELECT count(*) FROM organizations").fetch_one(&h.db.app).await.unwrap();
    assert_eq!(visible, 0);
    let (name,): (String,) = sqlx::query_as("SELECT name FROM organizations WHERE tenant_id = $1")
        .bind(tenant_b)
        .fetch_one(&h.db.owner)
        .await
        .unwrap();
    assert_eq!(name, "Tenant B");

    // Every table with a tenant_id column (except messaging infrastructure) forces RLS.
    let unprotected: Vec<(String,)> = sqlx::query_as(
        "SELECT c.relname::text FROM pg_class c JOIN pg_attribute a ON a.attrelid = c.oid AND a.attname = 'tenant_id' \
         WHERE c.relkind = 'r' AND c.relnamespace = 'public'::regnamespace \
           AND c.relname NOT IN ('outbox', 'user_org_directory', 'invitation_tokens', 'scim_token_index') \
           AND NOT (c.relrowsecurity AND c.relforcerowsecurity)",
    )
    .fetch_all(&h.db.owner)
    .await
    .unwrap();
    assert!(unprotected.is_empty(), "tables without forced RLS: {unprotected:?}");
    h.db.cleanup().await;
}

#[tokio::test]
async fn api_never_crosses_tenants() {
    let h = harness().await;
    let (ada, bob) = (User::new("ada"), User::new("bob"));
    let (org_a, tenant_a) = h.create_org(&ada, "Tenant A").await;
    let (org_b, tenant_b) = h.create_org(&bob, "Tenant B").await;
    let ada_token = h.org_token(&ada, tenant_a, org_a, "owner");

    let members = h.call(Method::GET, "/api/v1/organizations/current/members", Some(&ada_token), None, None).await;
    let emails: Vec<&str> = members.body["items"].as_array().unwrap().iter().map(|m| m["email"].as_str().unwrap()).collect();
    assert_eq!(emails, ["ada@example.com"]);

    // Ada (owner of A) targets Bob (member of B only) by id: he does not exist in her tenant.
    let cross = h
        .call(
            Method::PATCH,
            &format!("/api/v1/organizations/current/members/{}", bob.id),
            Some(&ada_token),
            Some(json!({ "org_role": "guest" })),
            None,
        )
        .await;
    assert_eq!(cross.status, StatusCode::NOT_FOUND);
    let cross =
        h.call(Method::DELETE, &format!("/api/v1/organizations/current/members/{}", bob.id), Some(&ada_token), None, None).await;
    assert_eq!(cross.status, StatusCode::NOT_FOUND);

    // An invitation id from tenant B is not revocable from tenant A.
    let bob_token = h.org_token(&bob, tenant_b, org_b, "owner");
    let invite = h
        .call(
            Method::POST,
            "/api/v1/organizations/current/invitations",
            Some(&bob_token),
            Some(json!({ "email": "x@example.com", "org_role": "member" })),
            None,
        )
        .await;
    let invite_id = invite.body["id"].as_str().unwrap();
    let cross = h
        .call(Method::DELETE, &format!("/api/v1/organizations/current/invitations/{invite_id}"), Some(&ada_token), None, None)
        .await;
    assert_eq!(cross.status, StatusCode::NOT_FOUND);
    let listed = h.call(Method::GET, "/api/v1/organizations/current/invitations", Some(&ada_token), None, None).await;
    assert_eq!(listed.body["items"], json!([]));

    let (role,): (String,) =
        sqlx::query_as("SELECT org_role FROM memberships WHERE user_id = $1").bind(bob.id).fetch_one(&h.db.owner).await.unwrap();
    assert_eq!(role, "owner", "tenant B is untouched");
    h.db.cleanup().await;
}

#[tokio::test]
async fn invitations_are_emailed_and_can_be_sent_again_with_a_new_link() {
    let h = harness().await;
    let (ada, grace, maya) = (User::new("ada"), User::new("grace"), User::new("maya"));
    let (org, tenant) = h.create_org(&ada, "Acme <Labs>").await;
    let owner = h.org_token(&ada, tenant, org, "owner");
    let mails_to = |to: String, expected: usize| {
        let outbox = Arc::clone(&h.outbox);
        async move {
            for _ in 0..50 {
                let found: Vec<(String, String)> = outbox
                    .0
                    .lock()
                    .unwrap()
                    .iter()
                    .filter(|(t, _, _)| *t == to)
                    .map(|(_, s, b)| (s.clone(), b.clone()))
                    .collect();
                if found.len() >= expected {
                    return found;
                }
                tokio::time::sleep(std::time::Duration::from_millis(40)).await;
            }
            panic!("expected {expected} emails to {to}");
        }
    };

    let created = h
        .call(
            Method::POST,
            "/api/v1/organizations/current/invitations",
            Some(&owner),
            Some(json!({ "email": grace.email, "org_role": "member" })),
            None,
        )
        .await;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.body);
    assert!(created.body["last_sent_at"].is_string());
    let first_url = created.body["accept_url"].as_str().unwrap().to_owned();
    let mails = mails_to(grace.email.clone(), 1).await;
    assert_eq!(mails[0].0, "ada invited you to Acme <Labs> on Meridian");
    assert!(mails[0].1.contains(&format!("Accept the invitation: {first_url}")) && mails[0].1.contains("as a member."));
    let id = created.body["id"].as_str().unwrap().to_owned();
    let resend = format!("/api/v1/organizations/current/invitations/{id}/resend");

    // Not again within a minute; members cannot send at all.
    assert_eq!(h.call(Method::POST, &resend, Some(&owner), None, None).await.status, StatusCode::TOO_MANY_REQUESTS);
    h.add_member(&owner, &maya, "member").await;
    let member = h.org_token(&maya, tenant, org, "member");
    assert_eq!(h.call(Method::POST, &resend, Some(&member), None, None).await.status, StatusCode::FORBIDDEN);

    // Later, an expired invitation gets a new link and a new week; the old link is dead.
    sqlx::query(
        "UPDATE invitations SET last_sent_at = now() - interval '2 minutes', expires_at = now() - interval '1 day' WHERE id = $1",
    )
    .bind(Uuid::parse_str(&id).unwrap())
    .execute(&h.db.owner)
    .await
    .unwrap();
    let again = h.call(Method::POST, &resend, Some(&owner), None, None).await;
    assert_eq!(again.status, StatusCode::OK, "{}", again.body);
    assert_eq!(again.body["state"], "pending");
    let second_url = again.body["accept_url"].as_str().unwrap().to_owned();
    assert_ne!(first_url, second_url);
    let mails = mails_to(grace.email.clone(), 2).await;
    assert!(mails[1].1.contains(&second_url));
    let token_of = |url: &str| url.rsplit('/').next().unwrap().to_owned();
    assert_eq!(
        h.call(Method::GET, &format!("/api/v1/invitations/{}", token_of(&first_url)), None, None, None).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        h.call(Method::GET, &format!("/api/v1/invitations/{}", token_of(&second_url)), None, None, None).await.status,
        StatusCode::OK
    );
    let listed = h.call(Method::GET, "/api/v1/organizations/current/invitations", Some(&owner), None, None).await;
    assert!(listed.body["items"][0].get("accept_url").is_none() && listed.body["items"][0]["last_sent_at"].is_string());

    // Once accepted there is nothing to resend.
    let accepted = h
        .call(
            Method::POST,
            &format!("/api/v1/invitations/{}/accept", token_of(&second_url)),
            Some(&h.account(&grace)),
            None,
            None,
        )
        .await;
    assert_eq!(accepted.status, StatusCode::OK);
    sqlx::query("UPDATE invitations SET last_sent_at = NULL").execute(&h.db.owner).await.unwrap();
    assert_eq!(h.call(Method::POST, &resend, Some(&owner), None, None).await.status, StatusCode::NOT_FOUND);
    h.db.cleanup().await;
}

#[tokio::test]
async fn invitation_lifecycle() {
    let h = harness().await;
    let (ada, grace, mallory) = (User::new("ada"), User::new("grace"), User::new("mallory"));
    let (org, tenant) = h.create_org(&ada, "Acme").await;
    let owner = h.org_token(&ada, tenant, org, "owner");
    let invite = |token: String, email: &'static str, role: &'static str| {
        let h = &h;
        async move {
            h.call(
                Method::POST,
                "/api/v1/organizations/current/invitations",
                Some(&token),
                Some(json!({ "email": email, "org_role": role })),
                None,
            )
            .await
        }
    };

    let created = invite(owner.clone(), "Grace@Example.com", "member").await;
    assert_eq!(created.status, StatusCode::CREATED);
    assert_eq!(created.body["email"], "grace@example.com");
    let url = created.body["accept_url"].as_str().unwrap();
    assert!(url.starts_with("http://web.test/invite/"));
    let token = url.rsplit('/').next().unwrap().to_owned();

    // Only the hash is stored, and listing never returns the link again.
    let (stored,): (Vec<u8>,) = sqlx::query_as("SELECT token_hash FROM invitation_tokens").fetch_one(&h.db.owner).await.unwrap();
    assert_ne!(stored, token.as_bytes());
    let listed = h.call(Method::GET, "/api/v1/organizations/current/invitations", Some(&owner), None, None).await;
    assert!(listed.body["items"][0].get("accept_url").is_none());

    assert_eq!(
        invite(owner.clone(), "grace@example.com", "member").await.status,
        StatusCode::CONFLICT,
        "duplicate pending invite"
    );
    assert_eq!(invite(owner.clone(), "ada@example.com", "member").await.status, StatusCode::CONFLICT, "already a member");
    assert_eq!(invite(owner.clone(), "o@example.com", "owner").await.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(invite(owner.clone(), "nope", "member").await.status, StatusCode::UNPROCESSABLE_ENTITY);

    // Public preview shows just enough; unknown tokens are indistinguishable from malformed ones.
    let preview = h.call(Method::GET, &format!("/api/v1/invitations/{token}"), None, None, None).await;
    assert_eq!(preview.status, StatusCode::OK);
    assert_eq!(preview.body["organization_name"], "Acme");
    assert_eq!(preview.body["state"], "pending");
    let unknown = h.call(Method::GET, &format!("/api/v1/invitations/{}", "A".repeat(43)), None, None, None).await;
    assert_eq!(unknown.status, StatusCode::NOT_FOUND);
    assert_eq!(h.call(Method::GET, "/api/v1/invitations/short", None, None, None).await.status, StatusCode::NOT_FOUND);

    // A different account holding the link cannot redeem it.
    let stolen =
        h.call(Method::POST, &format!("/api/v1/invitations/{token}/accept"), Some(&h.account(&mallory)), None, None).await;
    assert_eq!(stolen.status, StatusCode::CONFLICT);
    let anonymous = h.call(Method::POST, &format!("/api/v1/invitations/{token}/accept"), None, None, None).await;
    assert_eq!(anonymous.status, StatusCode::UNAUTHORIZED);

    // The invited address, but not yet proved to be the caller's: refused, and the link still works.
    let mut unverified = test_utilities::TestAuth::claims(grace.id, &grace.email);
    unverified.email_verified = false;
    let unverified = h.auth.sign(&unverified).unwrap();
    let refused = h.call(Method::POST, &format!("/api/v1/invitations/{token}/accept"), Some(&unverified), None, None).await;
    assert_eq!(
        (refused.status, refused.body["code"].as_str()),
        (StatusCode::FORBIDDEN, Some("email_unverified")),
        "{}",
        refused.body
    );

    let accepted =
        h.call(Method::POST, &format!("/api/v1/invitations/{token}/accept"), Some(&h.account(&grace)), None, None).await;
    assert_eq!(accepted.status, StatusCode::OK, "{}", accepted.body);
    assert_eq!(accepted.body["organization_id"], org.to_string());
    let again = h.call(Method::POST, &format!("/api/v1/invitations/{token}/accept"), Some(&h.account(&grace)), None, None).await;
    assert_eq!(again.status, StatusCode::NOT_FOUND, "single use");

    let mine = h.call(Method::GET, "/api/v1/organizations", Some(&h.account(&grace)), None, None).await;
    assert_eq!(mine.body["items"][0]["role"], "member");

    // Members cannot invite; admins cannot be out-ranked through invitations.
    let member = h.org_token(&grace, tenant, org, "member");
    assert_eq!(invite(member, "z@example.com", "guest").await.status, StatusCode::FORBIDDEN);

    // Revoked and expired invitations.
    let second = invite(owner.clone(), "late@example.com", "guest").await;
    let second_token = second.body["accept_url"].as_str().unwrap().rsplit('/').next().unwrap().to_owned();
    sqlx::query("UPDATE invitations SET expires_at = now() - interval '1 day' WHERE id = $1")
        .bind(Uuid::parse_str(second.body["id"].as_str().unwrap()).unwrap())
        .execute(&h.db.owner)
        .await
        .unwrap();
    let late = User::new("late");
    let expired =
        h.call(Method::POST, &format!("/api/v1/invitations/{second_token}/accept"), Some(&h.account(&late)), None, None).await;
    assert_eq!(expired.status, StatusCode::CONFLICT);
    // An expired invitation does not block re-inviting the same address.
    let reinvited = invite(owner.clone(), "late@example.com", "guest").await;
    assert_eq!(reinvited.status, StatusCode::CREATED, "{}", reinvited.body);
    let third_token = reinvited.body["accept_url"].as_str().unwrap().rsplit('/').next().unwrap().to_owned();
    let revoked = h
        .call(
            Method::DELETE,
            &format!("/api/v1/organizations/current/invitations/{}", reinvited.body["id"].as_str().unwrap()),
            Some(&owner),
            None,
            None,
        )
        .await;
    assert_eq!(revoked.status, StatusCode::NO_CONTENT);
    let dead =
        h.call(Method::POST, &format!("/api/v1/invitations/{third_token}/accept"), Some(&h.account(&late)), None, None).await;
    assert_eq!(dead.status, StatusCode::NOT_FOUND);
    h.db.cleanup().await;
}

#[tokio::test]
async fn member_management_enforces_role_rules() {
    let h = harness().await;
    let (ada, grace, linus) = (User::new("ada"), User::new("grace"), User::new("linus"));
    let (org, tenant) = h.create_org(&ada, "Acme").await;
    let owner = h.org_token(&ada, tenant, org, "owner");
    h.add_member(&owner, &grace, "admin").await;
    h.add_member(&owner, &linus, "member").await;
    let admin = h.org_token(&grace, tenant, org, "admin");
    let member = h.org_token(&linus, tenant, org, "member");
    let patch = |token: String, user: Uuid, body: Value| {
        let h = &h;
        async move {
            h.call(Method::PATCH, &format!("/api/v1/organizations/current/members/{user}"), Some(&token), Some(body), None).await
        }
    };

    // Privilege escalation attempts.
    assert_eq!(patch(member.clone(), linus.id, json!({ "org_role": "admin" })).await.status, StatusCode::FORBIDDEN);
    assert_eq!(patch(admin.clone(), grace.id, json!({ "org_role": "owner" })).await.status, StatusCode::FORBIDDEN);
    assert_eq!(patch(admin.clone(), ada.id, json!({ "org_role": "member" })).await.status, StatusCode::FORBIDDEN);
    assert_eq!(patch(admin.clone(), ada.id, json!({ "status": "suspended" })).await.status, StatusCode::FORBIDDEN);

    // Legitimate admin actions.
    let promoted = patch(admin.clone(), linus.id, json!({ "org_role": "admin" })).await;
    assert_eq!(promoted.status, StatusCode::OK, "{}", promoted.body);
    assert_eq!(promoted.body["org_role"], "admin");
    assert_eq!(patch(admin.clone(), linus.id, json!({ "status": "suspended" })).await.body["status"], "suspended");

    // The sole owner cannot step down, be suspended or leave.
    assert_eq!(patch(owner.clone(), ada.id, json!({ "org_role": "admin" })).await.status, StatusCode::CONFLICT);
    let leave =
        h.call(Method::DELETE, &format!("/api/v1/organizations/current/members/{}", ada.id), Some(&owner), None, None).await;
    assert_eq!(leave.status, StatusCode::CONFLICT);
    // After promoting a second owner, the first may step down.
    assert_eq!(patch(owner.clone(), grace.id, json!({ "org_role": "owner" })).await.status, StatusCode::OK);
    assert_eq!(patch(owner.clone(), ada.id, json!({ "org_role": "admin" })).await.status, StatusCode::OK);

    // Ada's token still says "owner", but the database says admin: the stale claim is not honoured.
    let stale = patch(owner.clone(), grace.id, json!({ "org_role": "member" })).await;
    assert_eq!(stale.status, StatusCode::FORBIDDEN);

    // Suspended members lose access on re-check even with a valid token.
    let suspended_admin = h.org_token(&linus, tenant, org, "admin");
    assert_eq!(patch(suspended_admin, ada.id, json!({ "org_role": "guest" })).await.status, StatusCode::FORBIDDEN);

    // Anyone can leave.
    let left =
        h.call(Method::DELETE, &format!("/api/v1/organizations/current/members/{}", ada.id), Some(&owner), None, None).await;
    assert_eq!(left.status, StatusCode::NO_CONTENT);
    let mine = h.call(Method::GET, "/api/v1/organizations", Some(&h.account(&ada)), None, None).await;
    assert_eq!(mine.body["items"], json!([]));

    assert_eq!(patch(admin.clone(), linus.id, json!({})).await.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(patch(admin, linus.id, json!({ "org_role": "root" })).await.status, StatusCode::UNPROCESSABLE_ENTITY);

    let changes: Vec<(String,)> = sqlx::query_as(
        "SELECT payload->'payload'->>'change' FROM outbox WHERE event_type = 'membership.changed' ORDER BY (payload->>'aggregate_version')::bigint",
    )
    .fetch_all(&h.db.owner)
    .await
    .unwrap();
    let changes: Vec<&str> = changes.iter().map(|(c,)| c.as_str()).collect();
    assert_eq!(changes, ["joined", "joined", "joined", "role_changed", "suspended", "role_changed", "role_changed", "removed"]);
    h.db.cleanup().await;
}

#[tokio::test]
async fn organization_rename_uses_optimistic_concurrency() {
    let h = harness().await;
    let ada = User::new("ada");
    let (org, tenant) = h.create_org(&ada, "Acme").await;
    let owner = h.org_token(&ada, tenant, org, "owner");
    let path = "/api/v1/organizations/current";

    let blind = h.call(Method::PATCH, path, Some(&owner), Some(json!({ "name": "Acme 2" })), None).await;
    assert_eq!(blind.status, StatusCode::PRECONDITION_REQUIRED);

    let ok = h.call(Method::PATCH, path, Some(&owner), Some(json!({ "name": "Acme 2" })), Some("\"1\"")).await;
    assert_eq!(ok.status, StatusCode::OK, "{}", ok.body);
    assert_eq!((ok.body["version"].as_i64(), ok.etag.as_deref()), (Some(2), Some("\"2\"")));

    // A second editor still holding version 1 is told, and shown the current state.
    let stale = h.call(Method::PATCH, path, Some(&owner), Some(json!({ "name": "Other" })), Some("\"1\"")).await;
    assert_eq!(stale.status, StatusCode::CONFLICT);
    assert_eq!(stale.body["code"], "version_conflict");
    assert_eq!(stale.body["current"]["name"], "Acme 2");

    let member = h.org_token(&ada, tenant, org, "member");
    let denied = h.call(Method::PATCH, path, Some(&member), Some(json!({ "name": "Nope" })), Some("\"2\"")).await;
    assert_eq!(denied.status, StatusCode::FORBIDDEN);
    h.db.cleanup().await;
}

#[tokio::test]
async fn a_workspace_can_require_two_step_verification() {
    let h = harness().await;
    let (ada, grace) = (User::new("ada"), User::new("grace"));
    let (org, tenant) = h.create_org(&ada, "Acme").await;
    let owner = h.org_token(&ada, tenant, org, "owner");
    h.add_member(&owner, &grace, "admin").await;
    let path = "/api/v1/organizations/current";
    let service = MembershipService { db: Arc::clone(&h.router) };
    let required = |user: Uuid| {
        let service = &service;
        async move {
            let request = CheckMembershipRequest { user_id: user.to_string(), organization_id: org.to_string() };
            service.check_membership(tonic::Request::new(request)).await.unwrap().into_inner().require_two_step
        }
    };
    assert!(!required(grace.id).await);

    let current = h.call(Method::GET, path, Some(&owner), None, None).await;
    assert_eq!(current.body["require_two_step"], false);
    let empty = h.call(Method::PATCH, path, Some(&owner), Some(json!({})), Some("\"1\"")).await;
    assert_eq!(empty.status, StatusCode::UNPROCESSABLE_ENTITY);
    let member = h.org_token(&grace, tenant, org, "member");
    let denied = h.call(Method::PATCH, path, Some(&member), Some(json!({ "require_two_step": true })), Some("\"1\"")).await;
    assert_eq!(denied.status, StatusCode::FORBIDDEN);

    let on = h.call(Method::PATCH, path, Some(&owner), Some(json!({ "require_two_step": true })), Some("\"1\"")).await;
    assert_eq!(on.status, StatusCode::OK, "{}", on.body);
    assert_eq!((on.body["require_two_step"].as_bool(), on.body["name"].as_str()), (Some(true), Some("Acme")));
    assert!(required(grace.id).await, "identity is told with every membership check");
    assert!(!required(User::new("stranger").id).await);
    // The setting goes through the same optimistic concurrency as a rename.
    let stale = h.call(Method::PATCH, path, Some(&owner), Some(json!({ "name": "Acme 2" })), Some("\"1\"")).await;
    assert_eq!(stale.status, StatusCode::CONFLICT);
    let off = h.call(Method::PATCH, path, Some(&owner), Some(json!({ "require_two_step": false })), Some("\"2\"")).await;
    assert_eq!(off.status, StatusCode::OK);

    // Both changes are audited; a no-op change emits nothing.
    let same = h.call(Method::PATCH, path, Some(&owner), Some(json!({ "require_two_step": false })), Some("\"3\"")).await;
    assert_eq!(same.status, StatusCode::OK);
    let recorder = user_service::audit::Recorder { db: Arc::clone(&h.router) };
    record_outbox(&h, &recorder).await;
    let page = h.call(Method::GET, "/api/v1/organizations/current/audit?category=organization", Some(&owner), None, None).await;
    let actions: Vec<&str> = page.body["items"].as_array().unwrap().iter().map(|i| i["action"].as_str().unwrap()).collect();
    assert_eq!(actions, ["organization.two_step_optional", "organization.two_step_required", "organization.created"]);
    h.db.cleanup().await;
}

#[tokio::test]
async fn member_listing_paginates_and_searches_safely() {
    let h = harness().await;
    let ada = User::new("ada");
    let (org, tenant) = h.create_org(&ada, "Acme").await;
    let owner = h.org_token(&ada, tenant, org, "owner");
    for name in ["bea", "cy", "di", "percent%name"] {
        h.add_member(&owner, &User::new(name), "member").await;
    }

    let mut seen = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let path = match &cursor {
            Some(c) => format!("/api/v1/organizations/current/members?limit=2&cursor={c}"),
            None => "/api/v1/organizations/current/members?limit=2".to_owned(),
        };
        let page = h.call(Method::GET, &path, Some(&owner), None, None).await;
        assert_eq!(page.status, StatusCode::OK);
        let items = page.body["items"].as_array().unwrap().clone();
        assert!(items.len() <= 2);
        seen.extend(items.iter().map(|m| m["email"].as_str().unwrap().to_owned()));
        match page.body["next_cursor"].as_str() {
            Some(next) => cursor = Some(next.to_owned()),
            None => break,
        }
    }
    assert_eq!(seen, ["ada@example.com", "bea@example.com", "cy@example.com", "di@example.com", "percent%name@example.com"]);

    let search = |q: &'static str| {
        let (h, owner) = (&h, owner.clone());
        async move {
            let reply =
                h.call(Method::GET, &format!("/api/v1/organizations/current/members?query={q}"), Some(&owner), None, None).await;
            assert_eq!(reply.status, StatusCode::OK, "{}", reply.body);
            reply.body["items"].as_array().unwrap().len()
        }
    };
    assert_eq!(search("BEA").await, 1);
    assert_eq!(search("%25").await, 1, "a literal % matches only the address containing one");
    assert_eq!(search("_").await, 0, "LIKE wildcards in input are not wildcards");
    assert_eq!(search("%27%3B%20DROP%20TABLE%20memberships%3B%20--").await, 0);
    let (still_there,): (i64,) = sqlx::query_as("SELECT count(*) FROM memberships").fetch_one(&h.db.owner).await.unwrap();
    assert_eq!(still_there, 5);

    let bad = h.call(Method::GET, "/api/v1/organizations/current/members?cursor=garbage", Some(&owner), None, None).await;
    assert_eq!(bad.status, StatusCode::UNPROCESSABLE_ENTITY);
    // Guests cannot browse the member directory.
    let guest = h.org_token(&ada, tenant, org, "guest");
    assert_eq!(
        h.call(Method::GET, "/api/v1/organizations/current/members", Some(&guest), None, None).await.status,
        StatusCode::FORBIDDEN
    );
    h.db.cleanup().await;
}

#[tokio::test]
async fn profile_is_created_from_token_and_editable() {
    let h = harness().await;
    let ada = User::new("ada");
    let token = h.account(&ada);
    let me = h.call(Method::GET, "/api/v1/users/me", Some(&token), None, None).await;
    assert_eq!((me.body["email"].as_str(), me.body["timezone"].as_str()), (Some("ada@example.com"), Some("UTC")));
    let updated = h
        .call(
            Method::PATCH,
            "/api/v1/users/me",
            Some(&token),
            Some(json!({ "display_name": " Ada L. ", "timezone": "Europe/London" })),
            None,
        )
        .await;
    assert_eq!(updated.body["display_name"], "Ada L.");
    // Edits made here are not overwritten by the name carried in later tokens.
    let again = h.call(Method::GET, "/api/v1/users/me", Some(&token), None, None).await;
    assert_eq!(again.body["display_name"], "Ada L.");
    let invalid = h.call(Method::PATCH, "/api/v1/users/me", Some(&token), Some(json!({ "display_name": "" })), None).await;
    assert_eq!(invalid.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(h.call(Method::GET, "/api/v1/users/me", None, None, None).await.status, StatusCode::UNAUTHORIZED);
    h.db.cleanup().await;
}

#[tokio::test]
async fn grpc_membership_check_reflects_role_and_status() {
    let h = harness().await;
    let (ada, grace, stranger) = (User::new("ada"), User::new("grace"), User::new("stranger"));
    let (org, tenant) = h.create_org(&ada, "Acme").await;
    let owner = h.org_token(&ada, tenant, org, "owner");
    h.add_member(&owner, &grace, "member").await;
    let service = MembershipService { db: Arc::clone(&h.router) };
    let check = |user: Uuid, org: Uuid| {
        let service = &service;
        async move {
            service
                .check_membership(tonic::Request::new(CheckMembershipRequest {
                    user_id: user.to_string(),
                    organization_id: org.to_string(),
                }))
                .await
                .unwrap()
                .into_inner()
        }
    };

    let reply = check(grace.id, org).await;
    assert!(reply.is_member);
    assert_eq!(
        (reply.tenant_id.as_str(), reply.org_role.as_str(), reply.status.as_str()),
        (tenant.to_string().as_str(), "member", "active")
    );
    assert!(!check(stranger.id, org).await.is_member);
    assert!(!check(grace.id, Uuid::now_v7()).await.is_member);

    h.call(
        Method::PATCH,
        &format!("/api/v1/organizations/current/members/{}", grace.id),
        Some(&owner),
        Some(json!({ "status": "suspended" })),
        None,
    )
    .await;
    assert_eq!(check(grace.id, org).await.status, "suspended", "identity refuses org tokens for suspended members");

    let bad = service
        .check_membership(tonic::Request::new(CheckMembershipRequest { user_id: "x".into(), organization_id: "y".into() }))
        .await;
    assert_eq!(bad.unwrap_err().code(), tonic::Code::InvalidArgument);

    // Bulk lookup for other services: members only, with role, status and name.
    let members = |tenant: Uuid, ids: Vec<String>| {
        let service = &service;
        async move {
            service.get_members(tonic::Request::new(GetMembersRequest { tenant_id: tenant.to_string(), user_ids: ids })).await
        }
    };
    let mut found = members(tenant, vec![ada.id.to_string(), grace.id.to_string(), stranger.id.to_string()])
        .await
        .unwrap()
        .into_inner()
        .members;
    found.sort_by_key(|m| m.org_role.clone());
    let summary: Vec<(String, &str, &str)> =
        found.iter().map(|m| (m.user_id.clone(), m.org_role.as_str(), m.status.as_str())).collect();
    assert_eq!(summary, [(grace.id.to_string(), "member", "suspended"), (ada.id.to_string(), "owner", "active")]);
    assert!(found.iter().all(|m| !m.display_name.is_empty() && m.email.contains('@')));
    assert!(members(Uuid::now_v7(), vec![ada.id.to_string()]).await.unwrap().into_inner().members.is_empty(), "other tenant");
    assert_eq!(members(tenant, vec!["x".into()]).await.unwrap_err().code(), tonic::Code::InvalidArgument);
    assert_eq!(members(tenant, vec![ada.id.to_string(); 501]).await.unwrap_err().code(), tonic::Code::InvalidArgument);

    // Finding people by exact email or name, ignoring case, only in that workspace.
    let find = |tenant: Uuid, terms: Vec<String>| {
        let service = &service;
        async move { service.find_members(tonic::Request::new(FindMembersRequest { tenant_id: tenant.to_string(), terms })).await }
    };
    let ada_row = found.iter().find(|m| m.user_id == ada.id.to_string()).unwrap().clone();
    let by_email = find(tenant, vec![ada_row.email.to_uppercase()]).await.unwrap().into_inner().members;
    assert_eq!(by_email.iter().map(|m| m.user_id.clone()).collect::<Vec<_>>(), [ada.id.to_string()]);
    let by_name = find(tenant, vec![format!("  {}  ", ada_row.display_name.to_lowercase())]).await.unwrap().into_inner().members;
    assert_eq!(by_name.len(), 1);
    assert!(
        find(tenant, vec![ada_row.display_name[..2].to_owned()]).await.unwrap().into_inner().members.is_empty(),
        "no partial matches"
    );
    assert!(find(Uuid::now_v7(), vec![ada_row.email.clone()]).await.unwrap().into_inner().members.is_empty(), "other tenant");
    assert_eq!(find(tenant, vec!["x".repeat(201)]).await.unwrap_err().code(), tonic::Code::InvalidArgument);
    h.db.cleanup().await;
}

/// Feed the outbox (real events from real requests) to the audit recorder.
async fn record_outbox(h: &Harness, recorder: &user_service::audit::Recorder) {
    use event_bus::Handler;
    let rows: Vec<(String, Value)> =
        sqlx::query_as("SELECT topic, payload FROM outbox ORDER BY created_at, id").fetch_all(&h.db.owner).await.unwrap();
    for (topic, envelope) in rows {
        let event = event_bus::Event::parse(&topic, &serde_json::to_vec(&envelope).unwrap()).unwrap();
        recorder.handle(&event).await.unwrap();
    }
}

#[tokio::test]
async fn admin_actions_land_in_a_tamper_evident_audit_log() {
    use event_bus::Handler;
    use event_contracts::{payloads::AuditRecordedV1, topics, Actor, EventEnvelope};

    let h = harness().await;
    let (ada, bob, carol) = (User::new("ada"), User::new("bob"), User::new("carol"));
    let (org, tenant) = h.create_org(&ada, "Acme").await;
    let owner = h.org_token(&ada, tenant, org, "owner");
    h.add_member(&owner, &bob, "member").await;
    let changed = h
        .call(
            Method::PATCH,
            &format!("/api/v1/organizations/current/members/{}", bob.id),
            Some(&owner),
            Some(json!({ "org_role": "admin" })),
            None,
        )
        .await;
    assert_eq!(changed.status, StatusCode::OK, "{}", changed.body);
    let (_, other_tenant) = h.create_org(&carol, "Other").await;

    let recorder = user_service::audit::Recorder { db: Arc::clone(&h.router) };
    record_outbox(&h, &recorder).await;
    record_outbox(&h, &recorder).await; // redelivery records nothing twice

    // An action reported by another service, and one that fails validation.
    let project = Uuid::now_v7();
    let granted = EventEnvelope::new(
        Some(tenant),
        project,
        0,
        Actor::User(ada.id),
        AuditRecordedV1 {
            action: "project.access_granted".into(),
            resource_type: "project".into(),
            resource_id: project.to_string(),
            resource_label: "PAY".into(),
            details: [("user_id".to_owned(), bob.id.to_string()), ("role".to_owned(), "member".to_owned())].into(),
        },
    );
    let parse =
        |e: &EventEnvelope<AuditRecordedV1>| event_bus::Event::parse(topics::AUDIT, &serde_json::to_vec(e).unwrap()).unwrap();
    recorder.handle(&parse(&granted)).await.unwrap();
    let mut forged = granted.clone();
    forged.event_id = Uuid::now_v7();
    forged.payload.action = "DROP TABLE".into();
    assert!(matches!(recorder.handle(&parse(&forged)).await, Err(event_bus::HandlerError::Permanent(_))));

    // Admins read it newest first, with names; members cannot.
    let page = h.call(Method::GET, "/api/v1/organizations/current/audit", Some(&owner), None, None).await;
    assert_eq!(page.status, StatusCode::OK, "{}", page.body);
    let actions: Vec<&str> = page.body["items"].as_array().unwrap().iter().map(|i| i["action"].as_str().unwrap()).collect();
    assert_eq!(
        actions,
        [
            "project.access_granted",
            "member.role_changed",
            "member.joined",
            "member.invited",
            "member.joined",
            "organization.created"
        ]
    );
    assert_eq!(page.body["items"][0]["details"]["user_id"], bob.id.to_string());
    assert_eq!(page.body["items"][0]["actor"], json!({ "type": "user", "id": ada.id.to_string() }));
    assert!(page.body["people"][bob.id.to_string()].is_string(), "{}", page.body["people"]);
    assert!(page.body["people"][ada.id.to_string()].is_string());

    let members = h.call(Method::GET, "/api/v1/organizations/current/audit?category=member", Some(&owner), None, None).await;
    assert_eq!(members.body["items"].as_array().unwrap().len(), 4);
    let by_bob =
        h.call(Method::GET, &format!("/api/v1/organizations/current/audit?actor={}", bob.id), Some(&owner), None, None).await;
    assert_eq!(by_bob.body["items"].as_array().unwrap().len(), 1, "bob accepted his invitation");
    let first = h.call(Method::GET, "/api/v1/organizations/current/audit?limit=4", Some(&owner), None, None).await;
    let cursor = first.body["next_cursor"].as_str().unwrap();
    let rest = h
        .call(Method::GET, &format!("/api/v1/organizations/current/audit?limit=4&cursor={cursor}"), Some(&owner), None, None)
        .await;
    assert_eq!(rest.body["items"].as_array().unwrap().len(), 2);
    assert!(rest.body["next_cursor"].is_null());
    assert_eq!(h.call(Method::GET, "/api/v1/organizations/current/audit?category=x", Some(&owner), None, None).await.status, 422);
    let carols = h.org_token(&carol, other_tenant, Uuid::now_v7(), "owner");
    let other = h.call(Method::GET, "/api/v1/organizations/current/audit", Some(&carols), None, None).await;
    assert_eq!(other.body["items"].as_array().unwrap().len(), 2, "another workspace sees only its own entries");
    let as_member = h.org_token(&bob, tenant, org, "member");
    assert_eq!(h.call(Method::GET, "/api/v1/organizations/current/audit", Some(&as_member), None, None).await.status, 403);

    // CSV export.
    let req = Request::builder()
        .uri("/api/v1/organizations/current/audit/export")
        .header(header::AUTHORIZATION, format!("Bearer {owner}"))
        .body(Body::empty())
        .unwrap();
    let res = h.app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.headers()[header::CONTENT_TYPE], "text/csv; charset=utf-8");
    assert!(res.headers()[header::CONTENT_DISPOSITION].to_str().unwrap().starts_with("attachment;"));
    let csv = String::from_utf8(axum::body::to_bytes(res.into_body(), 1 << 20).await.unwrap().to_vec()).unwrap();
    assert_eq!(csv.lines().count(), 7);
    assert!(csv.lines().nth(1).unwrap().contains("\"project.access_granted\""));

    // The chain verifies; the runtime role cannot change it; a change by the owner is caught.
    let verify = |token: String| {
        let h = &h;
        async move { h.call(Method::GET, "/api/v1/organizations/current/audit/verify", Some(&token), None, None).await.body }
    };
    assert_eq!(verify(owner.clone()).await, json!({ "entries": 6, "intact": true, "broken_at": null, "reason": null }));
    let mut tx = h.router.begin(tenant).await.unwrap();
    let refused = sqlx::query("UPDATE audit_log SET resource_label = 'x'").execute(tx.conn()).await;
    assert!(refused.is_err(), "the runtime role cannot rewrite history");
    drop(tx);
    let mut owner_tx = h.db.owner.begin().await.unwrap();
    for sql in [
        format!("SELECT set_config('app.tenant_id', '{tenant}', true)"),
        "ALTER TABLE audit_log DISABLE TRIGGER audit_log_append_only".to_owned(),
        "UPDATE audit_log SET details = '{\"role\": \"owner\"}' WHERE seq = 5".to_owned(),
        "ALTER TABLE audit_log ENABLE TRIGGER audit_log_append_only".to_owned(),
    ] {
        sqlx::query(&sql).execute(&mut *owner_tx).await.unwrap();
    }
    owner_tx.commit().await.unwrap();
    let broken = verify(owner).await;
    assert_eq!(broken["intact"], false);
    assert_eq!(broken["broken_at"], 5);
    assert_eq!(broken["entries"], 4);
    h.db.cleanup().await;
}

// --- SCIM provisioning -----------------------------------------------------------------------

impl Harness {
    async fn scim(&self, method: Method, path: &str, token: Option<&str>, body: Option<Value>) -> Reply {
        let mut req = Request::builder().method(method).uri(path).header(header::CONTENT_TYPE, "application/scim+json");
        if let Some(t) = token {
            req = req.header(header::AUTHORIZATION, format!("Bearer {t}"));
        }
        let body = body.map_or_else(Body::empty, |b| Body::from(b.to_string()));
        let response = self.app.clone().oneshot(req.body(body).unwrap()).await.unwrap();
        let status = response.status();
        if status != StatusCode::NO_CONTENT {
            let content_type = response.headers().get(header::CONTENT_TYPE).map(|v| v.to_str().unwrap().to_owned());
            assert_eq!(content_type.as_deref(), Some("application/scim+json"), "{path}");
        }
        let location = response.headers().get(header::LOCATION).map(|v| v.to_str().unwrap().to_owned());
        let bytes = axum::body::to_bytes(response.into_body(), 1 << 20).await.unwrap();
        Reply { status, body: serde_json::from_slice(&bytes).unwrap_or(Value::Null), etag: location }
    }

    /// The invitation token in the last email sent to `to`.
    fn emailed_token(&self, to: &str) -> String {
        let mails = self.outbox.0.lock().unwrap();
        let (_, _, text) = mails.iter().rev().find(|(addr, _, _)| addr == to).expect("an email");
        let start = text.find("/invite/").unwrap() + "/invite/".len();
        text[start..start + 43].to_owned()
    }

    /// Emails go out in the background: wait for at least `at_least`, then report how many.
    async fn mails_to(&self, to: &str, at_least: usize) -> usize {
        let count = || self.outbox.0.lock().unwrap().iter().filter(|(addr, _, _)| addr == to).count();
        for _ in 0..50 {
            if count() >= at_least {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(40)).await;
        }
        tokio::time::sleep(std::time::Duration::from_millis(40)).await;
        count()
    }

    async fn provisioning_token(&self, admin_token: &str) -> String {
        let reply = self
            .call(
                Method::POST,
                "/api/v1/organizations/current/provisioning/tokens",
                Some(admin_token),
                Some(json!({ "name": "Okta" })),
                None,
            )
            .await;
        assert_eq!(reply.status, StatusCode::CREATED, "{}", reply.body);
        assert_eq!(reply.body["base_url"], "http://api.test/scim/v2");
        reply.body["secret"].as_str().unwrap().to_owned()
    }
}

fn scim_user(email: &str, given: &str) -> Value {
    json!({
        "schemas": ["urn:ietf:params:scim:schemas:core:2.0:User"],
        "userName": email,
        "name": { "givenName": given, "familyName": "Example" },
        "emails": [{ "value": email, "type": "work", "primary": true }],
        "externalId": format!("ext-{given}"),
        "active": true,
    })
}

#[tokio::test]
async fn identity_providers_provision_and_deprovision_people() {
    let h = harness().await;
    let (ada, grace, bob, linus, carol) =
        (User::new("ada"), User::new("grace"), User::new("bob"), User::new("linus"), User::new("carol"));
    let (org, tenant) = h.create_org(&ada, "Acme").await;
    let owner = h.org_token(&ada, tenant, org, "owner");
    h.add_member(&owner, &grace, "admin").await;
    h.add_member(&owner, &bob, "member").await;
    let admin = h.org_token(&grace, tenant, org, "admin");

    // Only admins make provisioning tokens; the secret is shown once.
    let as_member = h
        .call(
            Method::POST,
            "/api/v1/organizations/current/provisioning/tokens",
            Some(&h.org_token(&bob, tenant, org, "member")),
            Some(json!({ "name": "x" })),
            None,
        )
        .await;
    assert_eq!(as_member.status, StatusCode::FORBIDDEN);
    let token = h.provisioning_token(&admin).await;
    assert!(token.starts_with("mscim_"));
    let listed = h.call(Method::GET, "/api/v1/organizations/current/provisioning/tokens", Some(&admin), None, None).await;
    assert_eq!(listed.body["items"][0]["name"], "Okta");
    assert!(!listed.body.to_string().contains(&token[6..]), "the secret is never listed");

    // Discovery needs no token; users do.
    let config = h.scim(Method::GET, "/scim/v2/ServiceProviderConfig", None, None).await;
    assert_eq!(config.body["patch"]["supported"], true);
    for bad in [None, Some("mscim_nope"), Some(&*format!("mscim_{}", "A".repeat(43)))] {
        let denied = h.scim(Method::GET, "/scim/v2/Users", bad, None).await;
        assert_eq!(denied.status, StatusCode::UNAUTHORIZED);
        assert_eq!(denied.body["schemas"][0], "urn:ietf:params:scim:api:messages:2.0:Error");
    }

    // A new person is invited as a member, by email.
    let created = h.scim(Method::POST, "/scim/v2/Users", Some(&token), Some(scim_user(&linus.email, "Linus"))).await;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.body);
    let linus_scim = created.body["id"].as_str().unwrap().to_owned();
    assert_eq!(created.etag.as_deref(), Some(&*format!("http://api.test/scim/v2/Users/{linus_scim}")));
    assert_eq!(created.body["active"], true);
    assert_eq!(h.mails_to(&linus.email, 1).await, 1);
    let duplicate = h.scim(Method::POST, "/scim/v2/Users", Some(&token), Some(scim_user(&linus.email.to_uppercase(), "L"))).await;
    assert_eq!(duplicate.status, StatusCode::CONFLICT);
    assert_eq!(duplicate.body["scimType"], "uniqueness");
    let found = h
        .scim(
            Method::GET,
            &format!("/scim/v2/Users?filter=userName%20eq%20%22{}%22", linus.email.to_uppercase()),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(found.body["totalResults"], 1, "{}", found.body);
    assert_eq!(found.body["Resources"][0]["id"], linus_scim);
    let bad_filter = h.scim(Method::GET, "/scim/v2/Users?filter=title%20sw%20%22x%22", Some(&token), None).await;
    assert_eq!(bad_filter.body["scimType"], "invalidFilter");

    // Accepting the invitation makes them the provisioned member.
    let accepted = h
        .call(
            Method::POST,
            &format!("/api/v1/invitations/{}/accept", h.emailed_token(&linus.email)),
            Some(&h.account(&linus)),
            None,
            None,
        )
        .await;
    assert_eq!(accepted.status, StatusCode::OK, "{}", accepted.body);
    let people = h.call(Method::GET, "/api/v1/organizations/current/provisioning/users", Some(&admin), None, None).await;
    assert_eq!(people.body["items"][0]["state"], "member", "{}", people.body);
    assert_eq!(people.body["items"][0]["user_id"], json!(linus.id));

    // Microsoft Entra style deactivation suspends them; reactivation restores them.
    let member_status = |user: Uuid| {
        let h = &h;
        async move {
            let (status,): (String,) = sqlx::query_as("SELECT status FROM memberships WHERE user_id = $1")
                .bind(user)
                .fetch_one(&h.db.owner)
                .await
                .unwrap();
            status
        }
    };
    let off = json!({ "schemas": ["urn:ietf:params:scim:api:messages:2.0:PatchOp"],
                      "Operations": [{ "op": "Replace", "path": "active", "value": "False" }] });
    let patched = h.scim(Method::PATCH, &format!("/scim/v2/Users/{linus_scim}"), Some(&token), Some(off.clone())).await;
    assert_eq!(patched.status, StatusCode::OK, "{}", patched.body);
    assert_eq!(patched.body["active"], false);
    assert_eq!(member_status(linus.id).await, "suspended");
    let on = json!({ "Operations": [{ "op": "replace", "value": { "active": true } }] });
    h.scim(Method::PATCH, &format!("/scim/v2/Users/{linus_scim}"), Some(&token), Some(on)).await;
    assert_eq!(member_status(linus.id).await, "active");

    // An existing member is linked, not invited again.
    let bob_scim = h.scim(Method::POST, "/scim/v2/Users", Some(&token), Some(scim_user(&bob.email, "Bob"))).await;
    assert_eq!(bob_scim.status, StatusCode::CREATED);
    assert_eq!(h.mails_to(&bob.email, 1).await, 1, "only the original invitation");
    // An admin's token cannot touch the owner.
    let ada_scim = h.scim(Method::POST, "/scim/v2/Users", Some(&token), Some(scim_user(&ada.email, "Ada"))).await;
    let ada_id = ada_scim.body["id"].as_str().unwrap();
    let refused = h.scim(Method::PATCH, &format!("/scim/v2/Users/{ada_id}"), Some(&token), Some(off.clone())).await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{}", refused.body);
    assert_eq!(member_status(ada.id).await, "active");
    let refused = h.scim(Method::DELETE, &format!("/scim/v2/Users/{ada_id}"), Some(&token), None).await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN);

    // Deactivating someone still invited withdraws the invitation.
    let carol_scim = h.scim(Method::POST, "/scim/v2/Users", Some(&token), Some(scim_user(&carol.email, "Carol"))).await;
    assert_eq!(h.mails_to(&carol.email, 1).await, 1);
    let carol_link = h.emailed_token(&carol.email);
    let carol_id = carol_scim.body["id"].as_str().unwrap();
    let put = h
        .scim(
            Method::PUT,
            &format!("/scim/v2/Users/{carol_id}"),
            Some(&token),
            Some(json!({ "userName": carol.email, "active": false })),
        )
        .await;
    assert_eq!(put.status, StatusCode::OK, "{}", put.body);
    let late =
        h.call(Method::POST, &format!("/api/v1/invitations/{carol_link}/accept"), Some(&h.account(&carol)), None, None).await;
    assert_eq!(late.status, StatusCode::NOT_FOUND);
    let people = h.call(Method::GET, "/api/v1/organizations/current/provisioning/users", Some(&admin), None, None).await;
    assert_eq!(people.body["total"], 4);
    assert!(people.body["items"].as_array().unwrap().iter().any(|p| p["state"] == "deactivated"));

    // Deleting a member removes them from the workspace.
    let deleted = h.scim(Method::DELETE, &format!("/scim/v2/Users/{linus_scim}"), Some(&token), None).await;
    assert_eq!(deleted.status, StatusCode::NO_CONTENT);
    assert_eq!(
        h.scim(Method::GET, &format!("/scim/v2/Users/{linus_scim}"), Some(&token), None).await.status,
        StatusCode::NOT_FOUND
    );
    let (left,): (i64,) = sqlx::query_as("SELECT count(*) FROM memberships WHERE user_id = $1")
        .bind(linus.id)
        .fetch_one(&h.db.owner)
        .await
        .unwrap();
    assert_eq!(left, 0);

    // Membership changes are attributed to the token in the audit trail.
    let (by_token,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM outbox WHERE event_type IN ('membership.changed', 'user.invited') AND payload::text LIKE '%scim:%'",
    )
    .fetch_one(&h.db.owner)
    .await
    .unwrap();
    assert_eq!(by_token, 5, "invite linus, suspend, reactivate, invite carol, remove linus");

    // Another workspace's token sees none of this.
    let (other_org, other_tenant) = h.create_org(&linus, "Other").await;
    let other = h.provisioning_token(&h.org_token(&linus, other_tenant, other_org, "owner")).await;
    assert_eq!(h.scim(Method::GET, &format!("/scim/v2/Users/{ada_id}"), Some(&other), None).await.status, StatusCode::NOT_FOUND);
    assert_eq!(h.scim(Method::GET, "/scim/v2/Users", Some(&other), None).await.body["totalResults"], 0);

    // The token is only as good as its creator's role, and revoking it ends it.
    let demoted = h
        .call(
            Method::PATCH,
            &format!("/api/v1/organizations/current/members/{}", grace.id),
            Some(&owner),
            Some(json!({ "org_role": "member" })),
            None,
        )
        .await;
    assert_eq!(demoted.status, StatusCode::OK);
    assert_eq!(h.scim(Method::GET, "/scim/v2/Users", Some(&token), None).await.status, StatusCode::UNAUTHORIZED);
    let owners_token = h.provisioning_token(&owner).await;
    assert_eq!(h.scim(Method::GET, "/scim/v2/Users", Some(&owners_token), None).await.status, StatusCode::OK);
    let tokens = h.call(Method::GET, "/api/v1/organizations/current/provisioning/tokens", Some(&owner), None, None).await;
    let newest = tokens.body["items"][0]["id"].as_str().unwrap().to_owned();
    let revoked = h
        .call(Method::DELETE, &format!("/api/v1/organizations/current/provisioning/tokens/{newest}"), Some(&owner), None, None)
        .await;
    assert_eq!(revoked.status, StatusCode::NO_CONTENT);
    assert_eq!(h.scim(Method::GET, "/scim/v2/Users", Some(&owners_token), None).await.status, StatusCode::UNAUTHORIZED);
    h.db.cleanup().await;
}

#[tokio::test]
async fn identity_provider_groups_decide_workspace_roles() {
    let h = harness().await;
    let (ada, grace, bob, linus) = (User::new("ada"), User::new("grace"), User::new("bob"), User::new("linus"));
    let (org, tenant) = h.create_org(&ada, "Acme").await;
    let owner = h.org_token(&ada, tenant, org, "owner");
    h.add_member(&owner, &grace, "admin").await;
    h.add_member(&owner, &bob, "member").await;
    let admin = h.org_token(&grace, tenant, org, "admin");
    let token = h.provisioning_token(&admin).await;
    let role_of = |user: Uuid| {
        let h = &h;
        async move {
            let (role,): (String,) = sqlx::query_as("SELECT org_role FROM memberships WHERE user_id = $1")
                .bind(user)
                .fetch_one(&h.db.owner)
                .await
                .unwrap();
            role
        }
    };
    let mut ids = HashMap::new();
    for (user, name) in [(&ada, "Ada"), (&grace, "Grace"), (&bob, "Bob"), (&linus, "Linus")] {
        let created = h.scim(Method::POST, "/scim/v2/Users", Some(&token), Some(scim_user(&user.email, name))).await;
        assert_eq!(created.status, StatusCode::CREATED, "{}", created.body);
        ids.insert(name, created.body["id"].as_str().unwrap().to_owned());
    }
    let member = |name: &str| json!({ "value": ids[name] });

    // The provider pushes a group; its members must be provisioned users of this workspace.
    let types = h.scim(Method::GET, "/scim/v2/ResourceTypes", None, None).await;
    assert_eq!(types.body["Resources"][1]["endpoint"], "/Groups");
    let unknown = h
        .scim(
            Method::POST,
            "/scim/v2/Groups",
            Some(&token),
            Some(json!({ "displayName": "Admins", "members": [{ "value": Uuid::now_v7() }] })),
        )
        .await;
    assert_eq!(unknown.status, StatusCode::BAD_REQUEST, "{}", unknown.body);
    let everyone = json!({ "displayName": "Admins", "externalId": "00g1",
        "members": [member("Ada"), member("Grace"), member("Bob"), member("Linus")] });
    let created = h.scim(Method::POST, "/scim/v2/Groups", Some(&token), Some(everyone.clone())).await;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.body);
    assert_eq!(created.body["members"].as_array().unwrap().len(), 4);
    let group = created.body["id"].as_str().unwrap().to_owned();
    let duplicate = h.scim(Method::POST, "/scim/v2/Groups", Some(&token), Some(json!({ "displayName": "ADMINS" }))).await;
    assert_eq!(duplicate.body["scimType"], "uniqueness");
    let found = h
        .scim(
            Method::GET,
            "/scim/v2/Groups?filter=displayName%20eq%20%22admins%22&excludedAttributes=members",
            Some(&token),
            None,
        )
        .await;
    assert_eq!(found.body["totalResults"], 1, "{}", found.body);
    assert!(found.body["Resources"][0].get("members").is_none());

    // Without a role the group changes nothing.
    assert_eq!(role_of(bob.id).await, "member");

    // Only admins see and map groups, never to owner.
    let groups_path = "/api/v1/organizations/current/provisioning/groups";
    let role_path = format!("{groups_path}/{group}/role");
    let as_member = h.call(Method::GET, groups_path, Some(&h.org_token(&bob, tenant, org, "member")), None, None).await;
    assert_eq!(as_member.status, StatusCode::FORBIDDEN);
    let listed = h.call(Method::GET, groups_path, Some(&admin), None, None).await;
    assert_eq!(listed.body["items"][0]["members"], 4, "{}", listed.body);
    let to_owner = h.call(Method::PUT, &role_path, Some(&admin), Some(json!({ "org_role": "owner" })), None).await;
    assert_eq!(to_owner.status, StatusCode::UNPROCESSABLE_ENTITY);

    // Mapped to admin: members become admins at once; the owner and the token's creator are left
    // alone; the invited person's invitation carries the role.
    let mapped = h.call(Method::PUT, &role_path, Some(&admin), Some(json!({ "org_role": "admin" })), None).await;
    assert_eq!(mapped.status, StatusCode::OK, "{}", mapped.body);
    assert_eq!(mapped.body["org_role"], "admin");
    assert_eq!(role_of(bob.id).await, "admin");
    assert_eq!(role_of(ada.id).await, "owner");
    assert_eq!(role_of(grace.id).await, "admin");
    let accepted = h
        .call(
            Method::POST,
            &format!("/api/v1/invitations/{}/accept", h.emailed_token(&linus.email)),
            Some(&h.account(&linus)),
            None,
            None,
        )
        .await;
    assert_eq!(accepted.status, StatusCode::OK, "{}", accepted.body);
    assert_eq!(role_of(linus.id).await, "admin");
    let people = h.call(Method::GET, "/api/v1/organizations/current/provisioning/users", Some(&admin), None, None).await;
    assert!(people.body["items"].as_array().unwrap().iter().all(|p| p["group_role"] == "admin"), "{}", people.body);

    // Okta removes Bob from the group: back to member. Entra ID adds him again: admin.
    let remove_bob = json!({ "Operations": [{ "op": "remove", "path": "members", "value": [member("Bob")] }] });
    let patched = h.scim(Method::PATCH, &format!("/scim/v2/Groups/{group}"), Some(&token), Some(remove_bob)).await;
    assert_eq!(patched.status, StatusCode::OK, "{}", patched.body);
    assert_eq!(role_of(bob.id).await, "member");
    let add_bob = json!({ "Operations": [{ "op": "Add", "path": "members", "value": [member("Bob")] }] });
    h.scim(Method::PATCH, &format!("/scim/v2/Groups/{group}"), Some(&token), Some(add_bob)).await;
    assert_eq!(role_of(bob.id).await, "admin");
    let remove_by_path = json!({ "Operations": [{ "op": "Remove", "path": format!("members[value eq \"{}\"]", ids["Bob"]) }] });
    h.scim(Method::PATCH, &format!("/scim/v2/Groups/{group}"), Some(&token), Some(remove_by_path)).await;
    assert_eq!(role_of(bob.id).await, "member");

    // The strongest of a person's groups wins; deleting a group drops what it gave.
    let guests = h
        .scim(
            Method::POST,
            "/scim/v2/Groups",
            Some(&token),
            Some(json!({ "displayName": "Contractors", "members": [member("Linus")] })),
        )
        .await;
    let guests_id = guests.body["id"].as_str().unwrap();
    h.call(Method::PUT, &format!("{groups_path}/{guests_id}/role"), Some(&admin), Some(json!({ "org_role": "guest" })), None)
        .await;
    assert_eq!(role_of(linus.id).await, "admin");
    let deleted = h.scim(Method::DELETE, &format!("/scim/v2/Groups/{group}"), Some(&token), None).await;
    assert_eq!(deleted.status, StatusCode::NO_CONTENT);
    assert_eq!(role_of(linus.id).await, "guest");

    // Taking the role away returns its members to member.
    let unmapped = h
        .call(Method::PUT, &format!("{groups_path}/{guests_id}/role"), Some(&admin), Some(json!({ "org_role": null })), None)
        .await;
    assert_eq!(unmapped.status, StatusCode::OK, "{}", unmapped.body);
    assert_eq!(role_of(linus.id).await, "member");
}
