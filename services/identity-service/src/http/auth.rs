use std::net::SocketAddr;

use auth_middleware::{revocation_key, AuthContext, Claims, JwkSet, TokenKind, ACCESS_TOKEN_TTL_SECS, API_AUDIENCE};
use axum::{
    extract::{ConnectInfo, State},
    http::{header::USER_AGENT, HeaderMap, StatusCode},
    Json,
};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use error_handling::{ApiError, ApiResult};
use event_contracts::{
    outbox,
    payloads::{LoginSucceededV1, SessionRevokedV1, UserRegisteredV1},
    Actor, EventEnvelope,
};
use sqlx::PgConnection;
use time::OffsetDateTime;
use uuid::Uuid;
use validator::Validate;

use super::dto::{AccountDto, ExchangeRequest, LoginRequest, RefreshRequest, RegisterRequest, TokenOrganization, TokenResponse};
use crate::{
    crypto,
    membership::MembershipInfo,
    store::{self, Account},
    AppState,
};

pub const REFRESH_COOKIE: &str = "refresh_token";
const COOKIE_PATH: &str = "/api/v1/auth";
const BAD_CREDENTIALS: ApiError = ApiError::Unauthenticated("Incorrect email or password.");
const SESSION_ENDED: ApiError = ApiError::Unauthenticated("Your session has ended. Sign in again.");
/// A second presentation of a refresh token inside this window is treated as a benign
/// race between browser tabs rather than token theft.
const REUSE_GRACE_SECS: i64 = 10;

struct ClientInfo {
    ip: Option<String>,
    user_agent: Option<String>,
}

fn client_info(state: &AppState, headers: &HeaderMap, peer: SocketAddr) -> ClientInfo {
    // Only believe X-Forwarded-For when we are deployed behind our own gateway, and then
    // only the right-most entry, which is the one the gateway appended.
    let forwarded = state
        .settings
        .trust_forwarded_for
        .then(|| headers.get("x-forwarded-for").and_then(|v| v.to_str().ok()))
        .flatten()
        .and_then(|v| v.rsplit(',').next())
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty());
    ClientInfo {
        ip: Some(forwarded.unwrap_or_else(|| peer.ip().to_string())),
        user_agent: headers.get(USER_AGENT).and_then(|v| v.to_str().ok()).map(|v| v.chars().take(300).collect()),
    }
}

fn refresh_cookie(state: &AppState, value: String) -> Cookie<'static> {
    Cookie::build((REFRESH_COOKIE, value))
        .http_only(true)
        .secure(state.settings.cookie_secure)
        .same_site(SameSite::Strict)
        .path(COOKIE_PATH)
        .max_age(time::Duration::days(i64::from(state.settings.session_ttl_days)))
        .build()
}

fn expired_cookie(state: &AppState) -> Cookie<'static> {
    Cookie::build((REFRESH_COOKIE, ""))
        .http_only(true)
        .secure(state.settings.cookie_secure)
        .same_site(SameSite::Strict)
        .path(COOKIE_PATH)
        .max_age(time::Duration::ZERO)
        .build()
}

fn issue_access_token(
    state: &AppState,
    account: &Account,
    session_id: Uuid,
    organization: Option<(Uuid, &MembershipInfo)>,
) -> ApiResult<TokenResponse> {
    let now = OffsetDateTime::now_utc().unix_timestamp();
    let claims = Claims {
        iss: state.settings.issuer.clone(),
        aud: API_AUDIENCE.to_owned(),
        sub: account.id,
        sid: session_id,
        jti: platform_common::new_id(),
        iat: now,
        exp: now + ACCESS_TOKEN_TTL_SECS,
        typ: if organization.is_some() { TokenKind::Org } else { TokenKind::Account },
        email: account.email.clone(),
        name: account.display_name.clone(),
        tid: organization.map(|(_, m)| m.tenant_id),
        org: organization.map(|(id, _)| id),
        roles: organization.map(|(_, m)| vec![m.role.as_str().to_owned()]).unwrap_or_default(),
        pv: 0,
    };
    Ok(TokenResponse {
        access_token: state.signer.sign(&claims).map_err(ApiError::internal)?,
        token_type: "Bearer",
        expires_in: ACCESS_TOKEN_TTL_SECS,
        account: AccountDto::from(account),
        organization: organization.map(|(id, m)| TokenOrganization { id, role: m.role.as_str().to_owned() }),
    })
}

/// Create a session and its first refresh token. Returns `(session_id, refresh_token)`.
async fn start_session(
    conn: &mut PgConnection,
    state: &AppState,
    account: &Account,
    client: &ClientInfo,
) -> ApiResult<(Uuid, String)> {
    let session = store::insert_session(
        conn,
        account.id,
        state.settings.session_ttl_days,
        client.ip.as_deref(),
        client.user_agent.as_deref(),
    )
    .await?;
    let refresh = crypto::random_token();
    store::insert_refresh_token(conn, session.id, &crypto::sha256(refresh.as_bytes())).await?;
    Ok((session.id, refresh))
}

fn normalize_email(email: &str) -> String {
    email.trim().to_lowercase()
}

#[utoipa::path(post, path = "/api/v1/auth/register", request_body = RegisterRequest,
    responses((status = 201, body = TokenResponse), (status = 409, description = "Email already registered"), (status = 422)))]
pub async fn register(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(mut body): Json<RegisterRequest>,
) -> ApiResult<(StatusCode, CookieJar, Json<TokenResponse>)> {
    body.email = normalize_email(&body.email);
    body.display_name = body.display_name.trim().to_owned();
    body.validate()?;
    let client = client_info(&state, &headers, peer);

    let password_hash = crypto::hash_password(body.password).await?;
    let account_id = platform_common::new_id();

    let mut tx = state.pool.begin().await?;
    let account = match store::insert_account(&mut tx, account_id, &body.email, &body.display_name, &password_hash).await {
        Ok(account) => account,
        Err(sqlx::Error::Database(db)) if db.is_unique_violation() => {
            return Err(ApiError::Conflict("An account with this email already exists.".into()));
        }
        Err(err) => return Err(err.into()),
    };
    let (session_id, refresh) = start_session(&mut tx, &state, &account, &client).await?;
    outbox::enqueue(
        &mut tx,
        &EventEnvelope::new(
            None,
            account.id,
            1,
            Actor::User(account.id),
            UserRegisteredV1 { user_id: account.id, email: account.email.clone(), display_name: account.display_name.clone() },
        ),
    )
    .await?;
    tx.commit().await?;

    tracing::info!(user_id = %account.id, "account registered");
    let response = issue_access_token(&state, &account, session_id, None)?;
    Ok((StatusCode::CREATED, jar.add(refresh_cookie(&state, refresh)), Json(response)))
}

#[utoipa::path(post, path = "/api/v1/auth/login", request_body = LoginRequest,
    responses((status = 200, body = TokenResponse), (status = 401, description = "Incorrect email or password")))]
pub async fn login(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(mut body): Json<LoginRequest>,
) -> ApiResult<(CookieJar, Json<TokenResponse>)> {
    body.email = normalize_email(&body.email);
    body.validate()?;
    let client = client_info(&state, &headers, peer);
    let email_hash = crypto::sha256(body.email.as_bytes());
    let log = |account_id: Option<Uuid>, outcome: &'static str| {
        let (pool, hash, ip, ua) = (state.pool.clone(), email_hash.clone(), client.ip.clone(), client.user_agent.clone());
        async move {
            if let Err(err) = store::insert_login_event(&pool, account_id, &hash, outcome, ip.as_deref(), ua.as_deref()).await {
                tracing::warn!(error = %err, "could not record login event");
            }
        }
    };

    let Some(account) = store::account_by_email(&state.pool, &body.email).await? else {
        // Spend the same time as a real verification so response timing does not reveal
        // whether the address has an account.
        let _ = crypto::verify_password(body.password, state.dummy_hash.as_ref().clone()).await;
        log(None, "bad_credentials").await;
        return Err(BAD_CREDENTIALS);
    };

    if account.locked_until.is_some_and(|until| until > OffsetDateTime::now_utc()) {
        let _ = crypto::verify_password(body.password, state.dummy_hash.as_ref().clone()).await;
        log(Some(account.id), "locked").await;
        return Err(BAD_CREDENTIALS);
    }

    if !crypto::verify_password(body.password, account.password_hash.clone()).await? {
        store::record_failed_login(&state.pool, account.id).await?;
        log(Some(account.id), "bad_credentials").await;
        return Err(BAD_CREDENTIALS);
    }

    if account.status != "active" {
        log(Some(account.id), "suspended").await;
        return Err(ApiError::Forbidden);
    }

    let mut tx = state.pool.begin().await?;
    store::reset_failed_logins(&mut tx, account.id).await?;
    let (session_id, refresh) = start_session(&mut tx, &state, &account, &client).await?;
    outbox::enqueue(
        &mut tx,
        &EventEnvelope::new(None, account.id, 1, Actor::User(account.id), LoginSucceededV1 { user_id: account.id, session_id }),
    )
    .await?;
    tx.commit().await?;
    log(Some(account.id), "success").await;

    let response = issue_access_token(&state, &account, session_id, None)?;
    Ok((jar.add(refresh_cookie(&state, refresh)), Json(response)))
}

async fn revoke_everywhere(
    state: &AppState,
    conn: &mut PgConnection,
    session_id: Uuid,
    account_id: Uuid,
    reason: &str,
) -> ApiResult<()> {
    if store::revoke_session(conn, session_id, account_id, reason).await? {
        outbox::enqueue(
            conn,
            &EventEnvelope::new(
                None,
                account_id,
                1,
                Actor::User(account_id),
                SessionRevokedV1 { user_id: account_id, session_id, reason: reason.to_owned() },
            ),
        )
        .await?;
    }
    // Make outstanding access tokens for this session unusable before they expire.
    if let Some(mut redis) = state.redis.get() {
        let result: Result<(), redis::RedisError> = redis::cmd("SET")
            .arg(revocation_key(session_id))
            .arg(1)
            .arg("EX")
            .arg(ACCESS_TOKEN_TTL_SECS + 30)
            .query_async(&mut redis)
            .await;
        if let Err(err) = result {
            tracing::warn!(error = %err, "could not publish session revocation; access token stays valid until expiry");
        }
    } else {
        tracing::warn!("redis unavailable; revoked session's access token stays valid until expiry");
    }
    Ok(())
}

#[utoipa::path(post, path = "/api/v1/auth/token/refresh", request_body = RefreshRequest,
    responses((status = 200, body = TokenResponse), (status = 401, description = "Session ended"),
              (status = 409, description = "Another request is rotating this token; retry")))]
pub async fn refresh(
    State(state): State<AppState>,
    jar: CookieJar,
    body: Option<Json<RefreshRequest>>,
) -> Result<(CookieJar, Json<TokenResponse>), (CookieJar, ApiError)> {
    let Json(body) = body.unwrap_or_default();
    let clear = |err: ApiError| (jar.clone().add(expired_cookie(&state)), err);
    let keep = |err: ApiError| (jar.clone(), err);

    let Some(presented) = jar.get(REFRESH_COOKIE).map(|c| c.value().to_owned()).filter(|v| !v.is_empty()) else {
        return Err(keep(SESSION_ENDED));
    };

    let mut tx = state.pool.begin().await.map_err(|e| keep(e.into()))?;
    let Some(found) = store::lock_refresh_token(&mut tx, &crypto::sha256(presented.as_bytes())).await.map_err(&keep)? else {
        return Err(clear(SESSION_ENDED));
    };
    if found.session_revoked || found.session_expired || found.token_expired {
        return Err(clear(SESSION_ENDED));
    }
    if let Some(used_at) = found.used_at {
        if (OffsetDateTime::now_utc() - used_at).whole_seconds() <= REUSE_GRACE_SECS {
            // The cookie jar already holds (or is about to hold) the rotated token.
            return Err(keep(ApiError::Conflict("Token rotation in progress. Retry.".into())));
        }
        // An old refresh token came back: assume it was stolen and end the session.
        tracing::warn!(session_id = %found.session_id, user_id = %found.account_id, "refresh token reuse detected; revoking session");
        revoke_everywhere(&state, &mut tx, found.session_id, found.account_id, "refresh_token_reuse").await.map_err(&keep)?;
        tx.commit().await.map_err(|e| keep(e.into()))?;
        return Err(clear(SESSION_ENDED));
    }

    let Some(account) = store::account_by_id(&state.pool, found.account_id).await.map_err(&keep)? else {
        return Err(clear(SESSION_ENDED));
    };
    if account.status != "active" {
        return Err(clear(ApiError::Forbidden));
    }

    // Resolve the organization scope before rotating, so a dependency outage leaves the
    // presented token usable for a retry.
    let membership = match body.organization_id {
        Some(org_id) => state.membership.check(account.id, org_id).await.map_err(&keep)?.map(|m| (org_id, m)),
        None => None,
    };

    store::mark_refresh_token_used(&mut tx, found.token_id).await.map_err(&keep)?;
    let next = crypto::random_token();
    store::insert_refresh_token(&mut tx, found.session_id, &crypto::sha256(next.as_bytes())).await.map_err(&keep)?;
    store::touch_session(&mut tx, found.session_id).await.map_err(&keep)?;
    tx.commit().await.map_err(|e| keep(e.into()))?;

    let response =
        issue_access_token(&state, &account, found.session_id, membership.as_ref().map(|(id, m)| (*id, m))).map_err(&keep)?;
    Ok((jar.add(refresh_cookie(&state, next)), Json(response)))
}

#[utoipa::path(post, path = "/api/v1/auth/token/exchange", request_body = ExchangeRequest, security(("bearer" = [])),
    responses((status = 200, body = TokenResponse), (status = 403, description = "Not a member of the organization")))]
pub async fn exchange(
    State(state): State<AppState>,
    auth: AuthContext,
    Json(body): Json<ExchangeRequest>,
) -> ApiResult<Json<TokenResponse>> {
    if !store::session_is_active(&state.pool, auth.session_id, auth.user_id).await? {
        return Err(SESSION_ENDED);
    }
    let account = store::account_by_id(&state.pool, auth.user_id).await?.ok_or(SESSION_ENDED)?;
    if account.status != "active" {
        return Err(ApiError::Forbidden);
    }
    // Membership and role are read from the user service now — never from the request and
    // never copied from the caller's current token.
    let membership = state.membership.check(account.id, body.organization_id).await?.ok_or(ApiError::Forbidden)?;
    Ok(Json(issue_access_token(&state, &account, auth.session_id, Some((body.organization_id, &membership)))?))
}

#[utoipa::path(post, path = "/api/v1/auth/logout", responses((status = 204)))]
pub async fn logout(State(state): State<AppState>, jar: CookieJar) -> ApiResult<(StatusCode, CookieJar)> {
    if let Some(presented) = jar.get(REFRESH_COOKIE).map(|c| c.value().to_owned()).filter(|v| !v.is_empty()) {
        let mut tx = state.pool.begin().await?;
        if let Some(found) = store::lock_refresh_token(&mut tx, &crypto::sha256(presented.as_bytes())).await? {
            revoke_everywhere(&state, &mut tx, found.session_id, found.account_id, "logout").await?;
        }
        tx.commit().await?;
    }
    Ok((StatusCode::NO_CONTENT, jar.add(expired_cookie(&state))))
}

#[utoipa::path(get, path = "/api/v1/auth/me", security(("bearer" = [])), responses((status = 200, body = AccountDto)))]
pub async fn me(State(state): State<AppState>, auth: AuthContext) -> ApiResult<Json<AccountDto>> {
    let account = store::account_by_id(&state.pool, auth.user_id).await?.ok_or(ApiError::NotFound("account"))?;
    Ok(Json(AccountDto::from(&account)))
}

#[utoipa::path(get, path = "/api/v1/auth/.well-known/jwks.json", responses((status = 200, description = "Public signing keys")))]
pub async fn jwks(State(state): State<AppState>) -> Json<JwkSet> {
    Json(state.jwks.as_ref().clone())
}

pub(crate) async fn revoke_session_everywhere(
    state: &AppState,
    session_id: Uuid,
    account_id: Uuid,
    reason: &str,
) -> ApiResult<bool> {
    let mut tx = state.pool.begin().await?;
    let owned: Option<(Uuid,)> = sqlx::query_as("SELECT id FROM sessions WHERE id = $1 AND account_id = $2")
        .bind(session_id)
        .bind(account_id)
        .fetch_optional(&mut *tx)
        .await?;
    if owned.is_none() {
        return Ok(false);
    }
    revoke_everywhere(state, &mut tx, session_id, account_id, reason).await?;
    tx.commit().await?;
    Ok(true)
}
