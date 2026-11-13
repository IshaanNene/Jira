//! HTTP + database tests for the user/organization service against a real PostgreSQL,
//! connected as an unprivileged role so row-level security is genuinely enforced.

use std::sync::{Arc, Mutex};

use api_contracts::user::v1::{membership_server::Membership, CheckMembershipRequest, GetMembersRequest};
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
    let state = AppState {
        db: Arc::clone(&router),
        verifier: Arc::clone(&auth.verifier),
        tenants: provisioner.clone(),
        settings: Arc::new(Settings { allowed_origins: vec![], web_base_url: "http://web.test".into() }),
    };
    Harness { db, app: http::router(state), auth, router, provisioner }
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
           AND c.relname NOT IN ('outbox', 'user_org_directory', 'invitation_tokens') \
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
    h.db.cleanup().await;
}
