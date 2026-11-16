//! Single sign-on (OpenID Connect) for workspaces.
//!
//! * Admins connect their workspace to its identity provider (`sso/connection`).
//! * Members link their own account to their identity there, while signed in (`sso/link`).
//!   A link is never made from what a provider asserts: an email address in an ID token proves
//!   nothing about a Meridian account, and the provider is run by the workspace's admins.
//! * Linked members then sign in by the workspace's sign-in name (`sso/start`). That session
//!   can enter only that workspace.
//!
//! The browser comes back to `sso/callback`. The `state` parameter finds the sign-in, and a
//! cookie set when it started must come back with it, so a link that someone else started
//! cannot sign a victim in to the wrong account (login CSRF). Each sign-in works once, for ten
//! minutes, with a nonce in the ID token and PKCE on the code exchange.

use std::net::SocketAddr;

use auth_middleware::{AuthContext, OrgContext};
use axum::{
    extract::{ConnectInfo, Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::Redirect,
    Json,
};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use error_handling::{ApiError, ApiResult};
use event_contracts::{
    outbox,
    payloads::{AuditRecordedV1, LoginSucceededV1},
    Actor, EventEnvelope,
};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use subtle::ConstantTimeEq;
use time::OffsetDateTime;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use super::{
    admin::admin,
    auth::{client_info, refresh_cookie, require_session, start_session, SignInMethod},
    mfa::{current_account, log_event},
};
use crate::{crypto, sso, store, AppState};

const STATE_COOKIE: &str = "sso_state";
const STATE_COOKIE_PATH: &str = "/api/v1/auth/sso";
const STATE_TTL_SECS: i64 = 600;

fn redirect_uri(state: &AppState) -> String {
    format!("{}/api/v1/auth/sso/callback", state.settings.app_url)
}

fn secret_context(organization_id: Uuid) -> Vec<u8> {
    [b"sso-client-secret:".as_slice(), organization_id.as_bytes()].concat()
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SsoConnectionDto {
    pub sign_in_name: String,
    pub issuer: String,
    pub client_id: String,
    pub enabled: bool,
    pub require_sso: bool,
    /// Register this as the redirect (callback) URI at the identity provider.
    pub redirect_uri: String,
    /// Members who have linked their account.
    pub linked_members: i64,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SaveSsoConnection {
    /// Lower-case letters, digits and dashes; what members type to find the sign-in.
    pub sign_in_name: String,
    /// The provider's issuer URL; `/.well-known/openid-configuration` is read from it.
    pub issuer: String,
    pub client_id: String,
    /// Required the first time; leave out to keep the stored one.
    pub client_secret: Option<String>,
    pub enabled: bool,
    /// Members other than owners may enter the workspace only after signing in through it.
    pub require_sso: bool,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct StartSso {
    pub sign_in_name: String,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct LinkSso {
    pub organization_id: Uuid,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SsoRedirect {
    /// Send the browser here (a full-page navigation).
    pub authorization_url: String,
}

#[derive(Debug, Serialize, ToSchema, FromRow)]
pub struct SsoLinkDto {
    pub organization_id: Uuid,
    pub sign_in_name: String,
    /// The address the provider reported when the link was made.
    pub email: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_used_at: Option<OffsetDateTime>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SsoLinkList {
    pub items: Vec<SsoLinkDto>,
}

#[derive(FromRow)]
struct Connection {
    organization_id: Uuid,
    sign_in_name: String,
    issuer: String,
    client_id: String,
    client_secret_enc: Vec<u8>,
    client_secret_nonce: Vec<u8>,
    enabled: bool,
    require_sso: bool,
    updated_at: OffsetDateTime,
}

const CONNECTION_COLUMNS: &str = "organization_id, sign_in_name, issuer, client_id, client_secret_enc, \
     client_secret_nonce, enabled, require_sso, updated_at";

async fn connection(state: &AppState, organization_id: Uuid) -> ApiResult<Option<Connection>> {
    Ok(sqlx::query_as(&format!("SELECT {CONNECTION_COLUMNS} FROM sso_connections WHERE organization_id = $1"))
        .bind(organization_id)
        .fetch_optional(&state.pool)
        .await?)
}

async fn dto(state: &AppState, c: Connection) -> ApiResult<SsoConnectionDto> {
    let (linked,): (i64,) = sqlx::query_as("SELECT count(*) FROM sso_identities WHERE organization_id = $1")
        .bind(c.organization_id)
        .fetch_one(&state.pool)
        .await?;
    Ok(SsoConnectionDto {
        sign_in_name: c.sign_in_name,
        issuer: c.issuer,
        client_id: c.client_id,
        enabled: c.enabled,
        require_sso: c.require_sso,
        redirect_uri: redirect_uri(state),
        linked_members: linked,
        updated_at: c.updated_at,
    })
}

fn normalize_name(raw: &str) -> ApiResult<String> {
    let name = raw.trim().to_ascii_lowercase();
    let ok = (1..=40).contains(&name.len())
        && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !name.starts_with('-')
        && !name.ends_with('-');
    if ok {
        Ok(name)
    } else {
        Err(ApiError::invalid("sign_in_name", "format", "Use 1 to 40 lower-case letters, digits and dashes."))
    }
}

async fn audit(conn: &mut sqlx::PgConnection, ctx: &OrgContext, action: &str, details: Vec<(&str, String)>) -> ApiResult<()> {
    outbox::enqueue(
        conn,
        &EventEnvelope::new(
            Some(ctx.tenant_id),
            ctx.organization_id,
            0,
            Actor::User(ctx.user_id),
            AuditRecordedV1 {
                action: action.into(),
                resource_type: "sso_connection".into(),
                resource_id: ctx.organization_id.to_string(),
                resource_label: "Single sign-on".into(),
                details: details.into_iter().map(|(k, v)| (k.to_owned(), v)).collect(),
            },
        ),
    )
    .await?;
    Ok(())
}

#[utoipa::path(get, path = "/api/v1/auth/sso/connection", security(("bearer" = [])),
    responses((status = 200, body = SsoConnectionDto), (status = 403), (status = 404, description = "Not set up")))]
pub async fn get_connection(
    State(state): State<AppState>,
    auth: AuthContext,
    ctx: OrgContext,
) -> ApiResult<Json<SsoConnectionDto>> {
    admin(&auth, &ctx)?;
    let found = connection(&state, ctx.organization_id).await?.ok_or(ApiError::NotFound("SSO connection"))?;
    Ok(Json(dto(&state, found).await?))
}

/// Set up or change the workspace's identity provider. The provider's discovery document is
/// read first, so a typo is caught here rather than at someone's sign-in. Changing the issuer
/// removes every member's link (their identities belong to the old provider).
#[utoipa::path(put, path = "/api/v1/auth/sso/connection", request_body = SaveSsoConnection, security(("bearer" = [])),
    responses((status = 200, body = SsoConnectionDto), (status = 403), (status = 409, description = "Sign-in name taken"),
        (status = 422, description = "Invalid, or the provider could not be reached")))]
pub async fn save_connection(
    State(state): State<AppState>,
    auth: AuthContext,
    ctx: OrgContext,
    Json(body): Json<SaveSsoConnection>,
) -> ApiResult<Json<SsoConnectionDto>> {
    admin(&auth, &ctx)?;
    let name = normalize_name(&body.sign_in_name)?;
    let issuer = body.issuer.trim().trim_end_matches('/').to_owned();
    let client_id = body.client_id.trim().to_owned();
    if client_id.is_empty() || client_id.len() > 255 {
        return Err(ApiError::invalid("client_id", "length", "Enter the client ID from your identity provider."));
    }
    let existing = connection(&state, ctx.organization_id).await?;
    let secret = match (body.client_secret.as_deref().map(str::trim).filter(|s| !s.is_empty()), &existing) {
        (Some(secret), _) if secret.len() <= 1000 => {
            state.sealer.seal_for(secret.as_bytes(), &secret_context(ctx.organization_id)).map_err(ApiError::internal)?
        }
        (Some(_), _) => return Err(ApiError::invalid("client_secret", "length", "That client secret is too long.")),
        (None, Some(c)) => (c.client_secret_enc.clone(), c.client_secret_nonce.clone()),
        (None, None) => return Err(ApiError::invalid("client_secret", "required", "Enter the client secret.")),
    };
    if let Err(e) = state.sso.discover(&issuer).await {
        return Err(ApiError::invalid("issuer", "unreachable", &e.to_string()));
    }

    let issuer_changed = existing.as_ref().is_some_and(|c| c.issuer != issuer);
    let mut tx = state.pool.begin().await?;
    let saved = sqlx::query_as::<_, Connection>(&format!(
        "INSERT INTO sso_connections (organization_id, tenant_id, sign_in_name, issuer, client_id, client_secret_enc, \
             client_secret_nonce, enabled, require_sso, created_by) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10) \
         ON CONFLICT (organization_id) DO UPDATE SET sign_in_name = $3, issuer = $4, client_id = $5, client_secret_enc = $6, \
             client_secret_nonce = $7, enabled = $8, require_sso = $9, updated_at = now() \
         RETURNING {CONNECTION_COLUMNS}"
    ))
    .bind(ctx.organization_id)
    .bind(ctx.tenant_id)
    .bind(&name)
    .bind(&issuer)
    .bind(&client_id)
    .bind(&secret.0)
    .bind(&secret.1)
    .bind(body.enabled)
    .bind(body.require_sso)
    .bind(ctx.user_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| match &e {
        sqlx::Error::Database(db) if db.constraint() == Some("sso_connections_name_idx") => {
            ApiError::Conflict("That sign-in name is taken. Choose another.".into())
        }
        _ => e.into(),
    })?;
    if issuer_changed {
        sqlx::query("DELETE FROM sso_identities WHERE organization_id = $1").bind(ctx.organization_id).execute(&mut *tx).await?;
    }
    audit(
        &mut tx,
        &ctx,
        if existing.is_some() { "sso.updated" } else { "sso.configured" },
        vec![
            ("issuer", issuer.clone()),
            ("sign_in_name", name.clone()),
            ("enabled", body.enabled.to_string()),
            ("require_sso", body.require_sso.to_string()),
        ],
    )
    .await?;
    tx.commit().await?;
    tracing::info!(organization_id = %ctx.organization_id, issuer_changed, "SSO connection saved");
    Ok(Json(dto(&state, saved).await?))
}

/// Remove single sign-on from the workspace, with every member's link.
#[utoipa::path(delete, path = "/api/v1/auth/sso/connection", security(("bearer" = [])),
    responses((status = 204), (status = 403), (status = 404)))]
pub async fn delete_connection(State(state): State<AppState>, auth: AuthContext, ctx: OrgContext) -> ApiResult<StatusCode> {
    admin(&auth, &ctx)?;
    let mut tx = state.pool.begin().await?;
    let removed =
        sqlx::query("DELETE FROM sso_connections WHERE organization_id = $1").bind(ctx.organization_id).execute(&mut *tx).await?;
    if removed.rows_affected() == 0 {
        return Err(ApiError::NotFound("SSO connection"));
    }
    audit(&mut tx, &ctx, "sso.removed", vec![]).await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Begin a round trip to the provider: remember the state, nonce and PKCE verifier, and tie
/// it to this browser with a cookie.
async fn begin(
    state: &AppState,
    jar: CookieJar,
    found: &Connection,
    purpose: &str,
    account_id: Option<Uuid>,
) -> ApiResult<(CookieJar, Json<SsoRedirect>)> {
    let unavailable = || ApiError::DependencyUnavailable("identity provider");
    let metadata = state.sso.discover(&found.issuer).await.map_err(|e| {
        tracing::warn!(organization_id = %found.organization_id, error = %e, "SSO discovery failed");
        unavailable()
    })?;
    let flow = state.sso.authorization(metadata, &found.client_id, &redirect_uri(state)).map_err(|_| unavailable())?;
    let browser = crypto::random_token();
    sqlx::query(
        "INSERT INTO sso_states (id, state_hash, browser_hash, organization_id, purpose, account_id, nonce, pkce_verifier, expires_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, now() + make_interval(secs => $9))",
    )
    .bind(platform_common::new_id())
    .bind(crypto::sha256(flow.state.as_bytes()))
    .bind(crypto::sha256(browser.as_bytes()))
    .bind(found.organization_id)
    .bind(purpose)
    .bind(account_id)
    .bind(&flow.nonce)
    .bind(&flow.pkce_verifier)
    .bind(STATE_TTL_SECS as f64)
    .execute(&state.pool)
    .await?;
    sqlx::query("DELETE FROM sso_states WHERE expires_at < now() - interval '1 day'").execute(&state.pool).await?;
    let cookie = Cookie::build((STATE_COOKIE, browser))
        .http_only(true)
        .secure(state.settings.cookie_secure)
        // Lax: the provider sends the browser back with a top-level GET from its own site.
        .same_site(SameSite::Lax)
        .path(STATE_COOKIE_PATH)
        .max_age(time::Duration::seconds(STATE_TTL_SECS))
        .build();
    Ok((jar.add(cookie), Json(SsoRedirect { authorization_url: flow.url })))
}

/// Start signing in through a workspace's identity provider.
#[utoipa::path(post, path = "/api/v1/auth/sso/start", request_body = StartSso,
    responses((status = 200, body = SsoRedirect), (status = 404, description = "No single sign-on under that name")))]
pub async fn start(
    State(state): State<AppState>,
    jar: CookieJar,
    Json(body): Json<StartSso>,
) -> ApiResult<(CookieJar, Json<SsoRedirect>)> {
    let not_found = || ApiError::invalid("sign_in_name", "unknown", "No workspace uses single sign-on under that name.");
    let name = normalize_name(&body.sign_in_name).map_err(|_| not_found())?;
    let found: Connection =
        sqlx::query_as(&format!("SELECT {CONNECTION_COLUMNS} FROM sso_connections WHERE sign_in_name = $1 AND enabled"))
            .bind(&name)
            .fetch_optional(&state.pool)
            .await?
            .ok_or_else(not_found)?;
    begin(&state, jar, &found, "sign_in", None).await
}

/// Link the signed-in account to its identity at a workspace's provider.
#[utoipa::path(post, path = "/api/v1/auth/sso/link", request_body = LinkSso, security(("bearer" = [])),
    responses((status = 200, body = SsoRedirect), (status = 403, description = "Not a member"), (status = 404)))]
pub async fn link(
    State(state): State<AppState>,
    auth: AuthContext,
    jar: CookieJar,
    Json(body): Json<LinkSso>,
) -> ApiResult<(CookieJar, Json<SsoRedirect>)> {
    let account = current_account(&state, &auth).await?;
    state.membership.check(account.id, body.organization_id).await?.ok_or(ApiError::Forbidden)?;
    let found =
        connection(&state, body.organization_id).await?.filter(|c| c.enabled).ok_or(ApiError::NotFound("SSO connection"))?;
    begin(&state, jar, &found, "link", Some(account.id)).await
}

#[derive(Debug, Serialize, ToSchema)]
pub struct WorkspaceSso {
    /// Whether the workspace signs people in through an identity provider.
    pub available: bool,
    /// What to type on the "Sign in with SSO" page.
    pub sign_in_name: Option<String>,
    pub require_sso: bool,
    /// Whether the caller has linked their account.
    pub linked: bool,
}

/// What any member needs to know about the current workspace's single sign-on.
#[utoipa::path(get, path = "/api/v1/auth/sso/workspace", security(("bearer" = [])), responses((status = 200, body = WorkspaceSso)))]
pub async fn workspace(State(state): State<AppState>, ctx: OrgContext) -> ApiResult<Json<WorkspaceSso>> {
    let found = connection(&state, ctx.organization_id).await?.filter(|c| c.enabled);
    let (linked,): (bool,) =
        sqlx::query_as("SELECT EXISTS (SELECT 1 FROM sso_identities WHERE organization_id = $1 AND account_id = $2)")
            .bind(ctx.organization_id)
            .bind(ctx.user_id)
            .fetch_one(&state.pool)
            .await?;
    Ok(Json(WorkspaceSso {
        available: found.is_some(),
        require_sso: found.as_ref().is_some_and(|c| c.require_sso),
        sign_in_name: found.map(|c| c.sign_in_name),
        linked,
    }))
}

#[utoipa::path(get, path = "/api/v1/auth/sso/links", security(("bearer" = [])), responses((status = 200, body = SsoLinkList)))]
pub async fn links(State(state): State<AppState>, auth: AuthContext) -> ApiResult<Json<SsoLinkList>> {
    require_session(&auth)?;
    let items = sqlx::query_as::<_, SsoLinkDto>(
        "SELECT i.organization_id, c.sign_in_name, i.email, i.created_at, i.last_used_at \
         FROM sso_identities i JOIN sso_connections c USING (organization_id) WHERE i.account_id = $1 ORDER BY i.created_at",
    )
    .bind(auth.user_id)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(SsoLinkList { items }))
}

#[utoipa::path(delete, path = "/api/v1/auth/sso/links/{organization_id}", security(("bearer" = [])),
    params(("organization_id" = Uuid, Path)), responses((status = 204), (status = 404)))]
pub async fn unlink(
    State(state): State<AppState>,
    auth: AuthContext,
    Path(organization_id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    require_session(&auth)?;
    let removed = sqlx::query("DELETE FROM sso_identities WHERE organization_id = $1 AND account_id = $2")
        .bind(organization_id)
        .bind(auth.user_id)
        .execute(&state.pool)
        .await?;
    if removed.rows_affected() == 0 {
        return Err(ApiError::NotFound("SSO link"));
    }
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct CallbackQuery {
    pub state: Option<String>,
    pub code: Option<String>,
    /// Set by the provider when the person cancelled or was refused there.
    pub error: Option<String>,
}

#[derive(FromRow)]
struct PendingSignIn {
    browser_hash: Vec<u8>,
    organization_id: Uuid,
    purpose: String,
    account_id: Option<Uuid>,
    nonce: String,
    pkce_verifier: String,
}

/// The provider sends the browser back here. Always answers with a redirect into the web app.
#[utoipa::path(get, path = "/api/v1/auth/sso/callback", params(CallbackQuery),
    responses((status = 303, description = "To the web app: signed in, linked, or why not")))]
pub async fn callback(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Query(query): Query<CallbackQuery>,
) -> (CookieJar, Redirect) {
    let browser = jar.get(STATE_COOKIE).map(|c| c.value().to_owned());
    let jar = jar.remove(Cookie::build(STATE_COOKIE).path(STATE_COOKIE_PATH));
    let app = state.settings.app_url.clone();
    match finish(&state, peer, &headers, browser, query).await {
        Ok(Done::SignedIn { organization_id, refresh }) => {
            (jar.add(refresh_cookie(&state, refresh)), Redirect::to(&format!("{app}/sso/complete?workspace={organization_id}")))
        }
        Ok(Done::Linked) => (jar, Redirect::to(&format!("{app}/settings/security?sso=linked"))),
        Err((purpose, reason)) => {
            let page = if purpose == "link" { "settings/security" } else { "login" };
            (jar, Redirect::to(&format!("{app}/{page}?sso_error={reason}")))
        }
    }
}

enum Done {
    SignedIn { organization_id: Uuid, refresh: String },
    Linked,
}

type Failure = (&'static str, &'static str);

async fn finish(
    state: &AppState,
    peer: SocketAddr,
    headers: &HeaderMap,
    browser: Option<String>,
    query: CallbackQuery,
) -> Result<Done, Failure> {
    let internal = |e: &dyn std::fmt::Display| {
        tracing::error!(error = %e, "SSO callback failed");
        ("sign_in", "unavailable")
    };
    let state_param = query.state.ok_or(("sign_in", "expired"))?;
    // Spend the sign-in first, whatever happens next.
    let pending: PendingSignIn = sqlx::query_as(
        "UPDATE sso_states SET used_at = now() WHERE state_hash = $1 AND used_at IS NULL AND expires_at > now() \
         RETURNING browser_hash, organization_id, purpose, account_id, nonce, pkce_verifier",
    )
    .bind(crypto::sha256(state_param.as_bytes()))
    .fetch_optional(&state.pool)
    .await
    .map_err(|e| internal(&e))?
    .ok_or(("sign_in", "expired"))?;
    let purpose: &'static str = if pending.purpose == "link" { "link" } else { "sign_in" };
    let fail = |reason: &'static str| (purpose, reason);

    let same_browser =
        browser.is_some_and(|b| bool::from(crypto::sha256(b.as_bytes()).as_slice().ct_eq(pending.browser_hash.as_slice())));
    if !same_browser {
        tracing::warn!(organization_id = %pending.organization_id, "SSO callback from a browser that did not start it");
        return Err(fail("expired"));
    }
    if query.error.is_some() {
        return Err(fail("cancelled"));
    }
    let code = query.code.ok_or(fail("provider"))?;
    let found = connection(state, pending.organization_id)
        .await
        .map_err(|e| (purpose, internal(&e).1))?
        .filter(|c| c.enabled)
        .ok_or(fail("unavailable"))?;
    let secret = state
        .sealer
        .open_for(&found.client_secret_enc, &found.client_secret_nonce, &secret_context(found.organization_id))
        .map_err(|e| (purpose, internal(&e).1))?;
    let secret = String::from_utf8(secret).map_err(|e| (purpose, internal(&e).1))?;
    let metadata = state.sso.discover(&found.issuer).await.map_err(|e| {
        tracing::warn!(organization_id = %found.organization_id, error = %e, "SSO discovery failed");
        fail("unavailable")
    })?;
    let identity = state
        .sso
        .complete(
            metadata,
            &found.client_id,
            &secret,
            &redirect_uri(state),
            &sso::Answer { code: &code, nonce: &pending.nonce, pkce_verifier: &pending.pkce_verifier },
        )
        .await
        .map_err(|e| {
            tracing::warn!(organization_id = %found.organization_id, error = %e, "SSO sign-in refused");
            fail("provider")
        })?;

    if let Some(account_id) = pending.account_id {
        // Linking: still a member, and neither side already linked to someone else.
        if state.membership.check(account_id, found.organization_id).await.map_err(|_| fail("unavailable"))?.is_none() {
            return Err(fail("not_member"));
        }
        let inserted = sqlx::query(
            "INSERT INTO sso_identities (id, organization_id, issuer, subject, account_id, email) VALUES ($1, $2, $3, $4, $5, $6) \
             ON CONFLICT DO NOTHING",
        )
        .bind(platform_common::new_id())
        .bind(found.organization_id)
        .bind(&identity.issuer)
        .bind(&identity.subject)
        .bind(account_id)
        .bind(&identity.email)
        .execute(&state.pool)
        .await
        .map_err(|e| (purpose, internal(&e).1))?;
        if inserted.rows_affected() == 0 {
            return Err(fail("already_linked"));
        }
        tracing::info!(user_id = %account_id, organization_id = %found.organization_id, "SSO identity linked");
        return Ok(Done::Linked);
    }

    let client = client_info(state, headers, peer);
    let linked: Option<(Uuid,)> = sqlx::query_as(
        "UPDATE sso_identities SET last_used_at = now() WHERE organization_id = $1 AND issuer = $2 AND subject = $3 RETURNING account_id",
    )
    .bind(found.organization_id)
    .bind(&identity.issuer)
    .bind(&identity.subject)
    .fetch_optional(&state.pool)
    .await
    .map_err(|e| internal(&e))?;
    let Some((account_id,)) = linked else { return Err(fail("not_linked")) };
    let account = store::account_by_id(&state.pool, account_id).await.map_err(|e| internal(&e))?.ok_or(fail("not_linked"))?;
    let email_hash = crypto::sha256(account.email.to_lowercase().as_bytes());
    let refuse = |outcome: &'static str, reason: &'static str| {
        let (pool, hash) = (state.pool.clone(), email_hash.clone());
        let client = &client;
        async move {
            log_event(&pool, account_id, &hash, outcome, client).await;
            Err::<Done, Failure>(fail(reason))
        }
    };
    if account.status != "active" {
        return refuse("suspended", "suspended").await;
    }
    if account.locked_until.is_some_and(|until| until > OffsetDateTime::now_utc()) {
        return refuse("locked", "suspended").await;
    }
    if state.membership.check(account.id, found.organization_id).await.map_err(|_| fail("unavailable"))?.is_none() {
        return refuse("sso_failed", "not_member").await;
    }

    let mut tx = state.pool.begin().await.map_err(|e| internal(&e))?;
    let (session_id, refresh) = start_session(&mut tx, state, &account, &client, SignInMethod::Sso(found.organization_id))
        .await
        .map_err(|e| internal(&e))?;
    outbox::enqueue(
        &mut tx,
        &EventEnvelope::new(None, account.id, 1, Actor::User(account.id), LoginSucceededV1 { user_id: account.id, session_id }),
    )
    .await
    .map_err(|e| internal(&e))?;
    tx.commit().await.map_err(|e| internal(&e))?;
    log_event(&state.pool, account.id, &email_hash, "success", &client).await;
    Ok(Done::SignedIn { organization_id: found.organization_id, refresh })
}
