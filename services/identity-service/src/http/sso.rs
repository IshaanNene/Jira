//! Single sign-on for workspaces, with OpenID Connect or SAML 2.0 (one per workspace).
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
//!
//! SAML: the provider POSTs its response to `sso/saml/acs`. A cross-site POST does not carry
//! the SameSite=Lax cookie, so the response is only parked with its sign-in (found by
//! `RelayState`) and the browser is sent on to `sso/saml/continue`, a top-level GET that does
//! carry it; there the cookie is checked and the response verified (see `crate::saml`).

use std::net::SocketAddr;

use auth_middleware::{AuthContext, OrgContext};
use axum::{
    extract::{ConnectInfo, Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Redirect, Response},
    Form, Json,
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
use crate::{crypto, saml, sso, store, AppState};

const STATE_COOKIE: &str = "sso_state";
const STATE_COOKIE_PATH: &str = "/api/v1/auth/sso";
const STATE_TTL_SECS: i64 = 600;

fn redirect_uri(state: &AppState) -> String {
    format!("{}/api/v1/auth/sso/callback", state.settings.app_url)
}

fn secret_context(organization_id: Uuid) -> Vec<u8> {
    [b"sso-client-secret:".as_slice(), organization_id.as_bytes()].concat()
}

/// What to enter at the identity provider for a SAML connection.
#[derive(Debug, Serialize, ToSchema)]
pub struct SamlServiceProvider {
    /// Entity ID (audience) of this workspace; its metadata is served at the same URL.
    pub entity_id: String,
    /// Assertion consumer service (reply) URL, HTTP-POST binding.
    pub acs_url: String,
    pub metadata_url: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SsoConnectionDto {
    /// `oidc` or `saml`.
    pub protocol: String,
    pub sign_in_name: String,
    /// OIDC: the issuer URL. SAML: the identity provider's entity ID.
    pub issuer: String,
    /// OIDC only.
    pub client_id: Option<String>,
    pub enabled: bool,
    pub require_sso: bool,
    /// OIDC: register this as the redirect (callback) URI at the identity provider.
    pub redirect_uri: String,
    /// SAML: what to enter at the identity provider.
    pub saml: Option<SamlServiceProvider>,
    /// Members who have linked their account.
    pub linked_members: i64,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SaveSsoConnection {
    /// `oidc` (the default) or `saml`.
    #[serde(default)]
    pub protocol: Option<String>,
    /// Lower-case letters, digits and dashes; what members type to find the sign-in.
    pub sign_in_name: String,
    /// OIDC: the provider's issuer URL; `/.well-known/openid-configuration` is read from it.
    #[serde(default)]
    pub issuer: String,
    /// OIDC only.
    #[serde(default)]
    pub client_id: String,
    /// SAML only: the identity provider's metadata XML, as downloaded from its SAML app. Blank
    /// keeps the stored metadata.
    #[serde(default)]
    pub saml_metadata: Option<String>,
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
    protocol: String,
    sign_in_name: String,
    issuer: String,
    client_id: Option<String>,
    client_secret_enc: Option<Vec<u8>>,
    client_secret_nonce: Option<Vec<u8>>,
    saml_metadata: Option<String>,
    saml_sso_url: Option<String>,
    enabled: bool,
    require_sso: bool,
    updated_at: OffsetDateTime,
}

const CONNECTION_COLUMNS: &str = "organization_id, protocol, sign_in_name, issuer, client_id, client_secret_enc, \
     client_secret_nonce, saml_metadata, saml_sso_url, enabled, require_sso, updated_at";

fn saml_sp(state: &AppState, organization_id: Uuid) -> saml::Sp {
    saml::Sp::new(&state.settings.app_url, organization_id)
}

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
    let saml = (c.protocol == "saml").then(|| {
        let sp = saml_sp(state, c.organization_id);
        SamlServiceProvider { metadata_url: sp.entity_id.clone(), entity_id: sp.entity_id, acs_url: sp.acs_url }
    });
    Ok(SsoConnectionDto {
        protocol: c.protocol,
        sign_in_name: c.sign_in_name,
        issuer: c.issuer,
        client_id: c.client_id,
        enabled: c.enabled,
        require_sso: c.require_sso,
        redirect_uri: redirect_uri(state),
        saml,
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
    let existing = connection(&state, ctx.organization_id).await?;
    let protocol = body.protocol.as_deref().unwrap_or("oidc");
    let (issuer, client_id, secret, metadata, sso_url) = match protocol {
        "oidc" => {
            let issuer = body.issuer.trim().trim_end_matches('/').to_owned();
            let client_id = body.client_id.trim().to_owned();
            if client_id.is_empty() || client_id.len() > 255 {
                return Err(ApiError::invalid("client_id", "length", "Enter the client ID from your identity provider."));
            }
            let kept = existing.as_ref().and_then(|c| c.client_secret_enc.clone().zip(c.client_secret_nonce.clone()));
            let secret = match (body.client_secret.as_deref().map(str::trim).filter(|s| !s.is_empty()), kept) {
                (Some(secret), _) if secret.len() <= 1000 => {
                    state.sealer.seal_for(secret.as_bytes(), &secret_context(ctx.organization_id)).map_err(ApiError::internal)?
                }
                (Some(_), _) => return Err(ApiError::invalid("client_secret", "length", "That client secret is too long.")),
                (None, Some(kept)) => kept,
                (None, None) => return Err(ApiError::invalid("client_secret", "required", "Enter the client secret.")),
            };
            if let Err(e) = state.sso.discover(&issuer).await {
                return Err(ApiError::invalid("issuer", "unreachable", &e.to_string()));
            }
            (issuer, Some(client_id), Some(secret), None, None)
        }
        "saml" => {
            // Blank keeps the stored metadata (the form does not show it again).
            let pasted = body.saml_metadata.as_deref().map(str::trim).filter(|x| !x.is_empty());
            let kept = existing.as_ref().filter(|c| c.protocol == "saml").and_then(|c| c.saml_metadata.as_deref());
            let xml = pasted.or(kept).unwrap_or_default().to_owned();
            let parsed = saml::parse_metadata(&xml, state.settings.sso_reach)
                .map_err(|message| ApiError::invalid("saml_metadata", "invalid", &message))?;
            (parsed.entity_id, None, None, Some(xml), Some(parsed.sso_url))
        }
        _ => return Err(ApiError::invalid("protocol", "unknown", "Choose OpenID Connect or SAML.")),
    };
    let (secret_enc, secret_nonce) = secret.unzip();

    // Links belong to the provider: a different one (or protocol) starts them over.
    let issuer_changed = existing.as_ref().is_some_and(|c| c.issuer != issuer || c.protocol != protocol);
    let mut tx = state.pool.begin().await?;
    let saved = sqlx::query_as::<_, Connection>(&format!(
        "INSERT INTO sso_connections (organization_id, tenant_id, sign_in_name, issuer, client_id, client_secret_enc, \
             client_secret_nonce, enabled, require_sso, created_by, protocol, saml_metadata, saml_sso_url) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13) \
         ON CONFLICT (organization_id) DO UPDATE SET sign_in_name = $3, issuer = $4, client_id = $5, client_secret_enc = $6, \
             client_secret_nonce = $7, enabled = $8, require_sso = $9, protocol = $11, saml_metadata = $12, \
             saml_sso_url = $13, updated_at = now() \
         RETURNING {CONNECTION_COLUMNS}"
    ))
    .bind(ctx.organization_id)
    .bind(ctx.tenant_id)
    .bind(&name)
    .bind(&issuer)
    .bind(&client_id)
    .bind(&secret_enc)
    .bind(&secret_nonce)
    .bind(body.enabled)
    .bind(body.require_sso)
    .bind(ctx.user_id)
    .bind(protocol)
    .bind(&metadata)
    .bind(&sso_url)
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
            ("protocol", protocol.to_owned()),
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
    // (where to send the browser, state, nonce or SAML request ID, PKCE verifier)
    let (url, flow_state, nonce, pkce_verifier) = if found.protocol == "saml" {
        let relay_state = crypto::random_token();
        let idp =
            saml::IdpMetadata { entity_id: found.issuer.clone(), sso_url: found.saml_sso_url.clone().ok_or_else(unavailable)? };
        let request = saml::authn_request(
            &saml_sp(state, found.organization_id),
            &idp,
            found.saml_metadata.as_deref().unwrap_or_default(),
            &relay_state,
        )
        .map_err(|e| {
            tracing::error!(organization_id = %found.organization_id, error = %e, "SAML request failed");
            unavailable()
        })?;
        (request.url, relay_state, request.request_id, None)
    } else {
        let metadata = state.sso.discover(&found.issuer).await.map_err(|e| {
            tracing::warn!(organization_id = %found.organization_id, error = %e, "SSO discovery failed");
            unavailable()
        })?;
        let client_id = found.client_id.as_deref().ok_or_else(unavailable)?;
        let flow = state.sso.authorization(metadata, client_id, &redirect_uri(state)).map_err(|_| unavailable())?;
        (flow.url, flow.state, flow.nonce, Some(flow.pkce_verifier))
    };
    let browser = crypto::random_token();
    sqlx::query(
        "INSERT INTO sso_states (id, state_hash, browser_hash, organization_id, purpose, account_id, nonce, pkce_verifier, \
             protocol, expires_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, now() + make_interval(secs => $10))",
    )
    .bind(platform_common::new_id())
    .bind(crypto::sha256(flow_state.as_bytes()))
    .bind(crypto::sha256(browser.as_bytes()))
    .bind(found.organization_id)
    .bind(purpose)
    .bind(account_id)
    .bind(&nonce)
    .bind(&pkce_verifier)
    .bind(if found.protocol == "saml" { "saml" } else { "oidc" })
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
    Ok((jar.add(cookie), Json(SsoRedirect { authorization_url: url })))
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
    pkce_verifier: Option<String>,
    saml_response: Option<String>,
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
    let outcome = finish(&state, peer, &headers, browser, query).await;
    conclude_redirect(&state, jar, outcome)
}

/// Clear the flow's cookie and send the browser to the right page of the web app.
fn conclude_redirect(state: &AppState, jar: CookieJar, outcome: Result<Done, Failure>) -> (CookieJar, Redirect) {
    let jar = jar.remove(Cookie::build(STATE_COOKIE).path(STATE_COOKIE_PATH));
    let app = state.settings.app_url.clone();
    match outcome {
        Ok(Done::SignedIn { organization_id, refresh }) => {
            (jar.add(refresh_cookie(state, refresh)), Redirect::to(&format!("{app}/sso/complete?workspace={organization_id}")))
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

fn internal(e: &dyn std::fmt::Display) -> Failure {
    tracing::error!(error = %e, "SSO callback failed");
    ("sign_in", "unavailable")
}

/// Spend a sign-in by its `state` (OIDC) or `RelayState` (SAML), whatever happens next, and
/// check it came back to the browser that started it.
async fn spend(state: &AppState, state_param: &str, protocol: &str, browser: Option<String>) -> Result<PendingSignIn, Failure> {
    let pending: PendingSignIn = sqlx::query_as(
        // The parked response is taken (RETURNING gives the old value through the self-join)
        // and cleared in one step.
        "UPDATE sso_states s SET used_at = now(), saml_response = NULL FROM sso_states old \
         WHERE old.id = s.id AND s.state_hash = $1 AND s.protocol = $2 AND s.used_at IS NULL AND s.expires_at > now() \
         RETURNING s.browser_hash, s.organization_id, s.purpose, s.account_id, s.nonce, s.pkce_verifier, old.saml_response",
    )
    .bind(crypto::sha256(state_param.as_bytes()))
    .bind(protocol)
    .fetch_optional(&state.pool)
    .await
    .map_err(|e| internal(&e))?
    .ok_or(("sign_in", "expired"))?;
    let same_browser =
        browser.is_some_and(|b| bool::from(crypto::sha256(b.as_bytes()).as_slice().ct_eq(pending.browser_hash.as_slice())));
    if !same_browser {
        tracing::warn!(organization_id = %pending.organization_id, "SSO callback from a browser that did not start it");
        return Err((if pending.purpose == "link" { "link" } else { "sign_in" }, "expired"));
    }
    Ok(pending)
}

async fn finish(
    state: &AppState,
    peer: SocketAddr,
    headers: &HeaderMap,
    browser: Option<String>,
    query: CallbackQuery,
) -> Result<Done, Failure> {
    let state_param = query.state.ok_or(("sign_in", "expired"))?;
    let pending = spend(state, &state_param, "oidc", browser).await?;
    let purpose: &'static str = if pending.purpose == "link" { "link" } else { "sign_in" };
    let fail = |reason: &'static str| (purpose, reason);
    if query.error.is_some() {
        return Err(fail("cancelled"));
    }
    let code = query.code.ok_or(fail("provider"))?;
    let found = connection(state, pending.organization_id)
        .await
        .map_err(|e| (purpose, internal(&e).1))?
        .filter(|c| c.enabled && c.protocol == "oidc")
        .ok_or(fail("unavailable"))?;
    let (Some(client_id), Some(sealed), Some(nonce)) =
        (found.client_id.as_deref(), found.client_secret_enc.as_deref(), found.client_secret_nonce.as_deref())
    else {
        return Err(fail("unavailable"));
    };
    let secret =
        state.sealer.open_for(sealed, nonce, &secret_context(found.organization_id)).map_err(|e| (purpose, internal(&e).1))?;
    let secret = String::from_utf8(secret).map_err(|e| (purpose, internal(&e).1))?;
    let metadata = state.sso.discover(&found.issuer).await.map_err(|e| {
        tracing::warn!(organization_id = %found.organization_id, error = %e, "SSO discovery failed");
        fail("unavailable")
    })?;
    let identity = state
        .sso
        .complete(
            metadata,
            client_id,
            &secret,
            &redirect_uri(state),
            &sso::Answer {
                code: &code,
                nonce: &pending.nonce,
                pkce_verifier: pending.pkce_verifier.as_deref().unwrap_or_default(),
            },
        )
        .await
        .map_err(|e| {
            tracing::warn!(organization_id = %found.organization_id, error = %e, "SSO sign-in refused");
            fail("provider")
        })?;
    conclude(state, peer, headers, &pending, &found, identity).await
}

/// Link the identity or sign the person in, the same for both protocols.
async fn conclude(
    state: &AppState,
    peer: SocketAddr,
    headers: &HeaderMap,
    pending: &PendingSignIn,
    found: &Connection,
    identity: sso::Identity,
) -> Result<Done, Failure> {
    let purpose: &'static str = if pending.purpose == "link" { "link" } else { "sign_in" };
    let fail = |reason: &'static str| (purpose, reason);
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

// --- SAML -----------------------------------------------------------------------------------

/// The workspace's service-provider metadata (its entity ID resolves here), for its admins to
/// give their identity provider. Public: it holds nothing secret.
#[utoipa::path(get, path = "/api/v1/auth/sso/saml/{organization_id}", params(("organization_id" = Uuid, Path)),
    responses((status = 200, description = "SAML metadata XML", content_type = "application/samlmetadata+xml"), (status = 404)))]
pub async fn saml_metadata(State(state): State<AppState>, Path(organization_id): Path<Uuid>) -> ApiResult<Response> {
    connection(&state, organization_id).await?.filter(|c| c.protocol == "saml").ok_or(ApiError::NotFound("SAML connection"))?;
    let xml = saml_sp(&state, organization_id).metadata();
    Ok(([(header::CONTENT_TYPE, "application/samlmetadata+xml")], xml).into_response())
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct SamlPost {
    #[serde(rename = "SAMLResponse")]
    pub saml_response: Option<String>,
    #[serde(rename = "RelayState")]
    pub relay_state: Option<String>,
}

/// The identity provider POSTs here (HTTP-POST binding). The response is parked with the
/// sign-in it names and the browser sent on to `continue`, where it is checked.
#[utoipa::path(post, path = "/api/v1/auth/sso/saml/acs", request_body(content = SamlPost, content_type = "application/x-www-form-urlencoded"),
    responses((status = 303, description = "On to `sso/saml/continue`, or to the sign-in page when the sign-in is unknown")))]
pub async fn saml_acs(State(state): State<AppState>, Form(form): Form<SamlPost>) -> Redirect {
    let app = state.settings.app_url.clone();
    let expired = Redirect::to(&format!("{app}/login?sso_error=expired"));
    // No RelayState: an unsolicited (IdP-initiated) response, which is not accepted.
    let (Some(relay_state), Some(response)) = (form.relay_state, form.saml_response) else { return expired };
    if relay_state.len() > 100 || response.is_empty() || response.len() > saml::MAX_RESPONSE {
        return expired;
    }
    let parked = sqlx::query(
        "UPDATE sso_states SET saml_response = $2 \
         WHERE state_hash = $1 AND protocol = 'saml' AND used_at IS NULL AND saml_response IS NULL AND expires_at > now()",
    )
    .bind(crypto::sha256(relay_state.as_bytes()))
    .bind(&response)
    .execute(&state.pool)
    .await;
    match parked {
        Ok(done) if done.rows_affected() == 1 => {
            let mut next = reqwest::Url::parse(&format!("{app}/api/v1/auth/sso/saml/continue")).expect("app URL");
            next.query_pairs_mut().append_pair("state", &relay_state);
            Redirect::to(next.as_str())
        }
        Ok(_) => expired,
        Err(error) => {
            tracing::error!(%error, "parking a SAML response failed");
            Redirect::to(&format!("{app}/login?sso_error=unavailable"))
        }
    }
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct SamlContinue {
    pub state: Option<String>,
}

/// Back in the browser that started the sign-in, with its cookie: verify the parked response.
#[utoipa::path(get, path = "/api/v1/auth/sso/saml/continue", params(SamlContinue),
    responses((status = 303, description = "To the web app: signed in, linked, or why not")))]
pub async fn saml_continue(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Query(query): Query<SamlContinue>,
) -> (CookieJar, Redirect) {
    let browser = jar.get(STATE_COOKIE).map(|c| c.value().to_owned());
    let outcome = finish_saml(&state, peer, &headers, browser, query).await;
    conclude_redirect(&state, jar, outcome)
}

async fn finish_saml(
    state: &AppState,
    peer: SocketAddr,
    headers: &HeaderMap,
    browser: Option<String>,
    query: SamlContinue,
) -> Result<Done, Failure> {
    let state_param = query.state.ok_or(("sign_in", "expired"))?;
    let pending = spend(state, &state_param, "saml", browser).await?;
    let purpose: &'static str = if pending.purpose == "link" { "link" } else { "sign_in" };
    let fail = |reason: &'static str| (purpose, reason);
    let response = pending.saml_response.clone().ok_or(fail("expired"))?;
    let found = connection(state, pending.organization_id)
        .await
        .map_err(|e| (purpose, internal(&e).1))?
        .filter(|c| c.enabled && c.protocol == "saml")
        .ok_or(fail("unavailable"))?;
    let sp = saml_sp(state, found.organization_id);
    let metadata = found.saml_metadata.clone().unwrap_or_default();
    let request_id = pending.nonce.clone();
    // xmlsec is synchronous work: keep it off the async workers.
    let verified = tokio::task::spawn_blocking(move || saml::verify(&sp, &metadata, &response, &request_id))
        .await
        .map_err(|e| (purpose, internal(&e).1))?;
    let identity = verified.map_err(|e| {
        tracing::warn!(organization_id = %found.organization_id, error = %e, "SAML sign-in refused");
        fail("provider")
    })?;
    // The links are keyed by the entity ID the admin configured, which samael has checked
    // against the response's issuer.
    let identity = sso::Identity { issuer: found.issuer.clone(), subject: identity.subject, email: identity.email };
    conclude(state, peer, headers, &pending, &found, identity).await
}
