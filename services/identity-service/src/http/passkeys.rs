//! Passkeys: adding them to an account, removing them, and signing in with one.
//!
//! Sign-in uses discoverable credentials: the browser offers the person's passkeys for this
//! site without being told who they are, so the options endpoint reveals nothing about which
//! accounts exist. The passkey's user handle is the account id, and it must match the account
//! that owns the credential.

use std::net::SocketAddr;

use auth_middleware::AuthContext;
use axum::{
    extract::{ConnectInfo, Path, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use axum_extra::extract::cookie::CookieJar;
use error_handling::{ApiError, ApiResult};
use event_contracts::{outbox, payloads::LoginSucceededV1, Actor, EventEnvelope};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::FromRow;
use time::OffsetDateTime;
use utoipa::ToSchema;
use uuid::Uuid;

use super::{
    auth::{client_info, issue_access_token, refresh_cookie, require_session, start_session},
    dto::TokenResponse,
    mfa::{confirm_password, current_account, log_event, PasswordConfirmation},
};
use crate::{
    crypto, store,
    webauthn::{self, b64, unb64},
    AppState,
};

const CHALLENGE_TTL_SECS: i64 = 300;
/// Passkeys one account may have.
const MAX_PASSKEYS: i64 = 20;
const SIGN_IN_FAILED: ApiError = ApiError::Unauthenticated("That passkey didn't work. Try again, or sign in with your password.");

#[derive(Debug, Serialize, ToSchema, FromRow)]
pub struct PasskeyDto {
    pub id: Uuid,
    pub name: String,
    /// Whether it syncs between the person's devices (a password manager or platform
    /// keychain) rather than living on one security key.
    pub synced: bool,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_used_at: Option<OffsetDateTime>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct PasskeyList {
    pub items: Vec<PasskeyDto>,
}

/// Options for `navigator.credentials.create()` / `.get()`, in the JSON form of the
/// WebAuthn spec: binary values are base64url strings.
#[derive(Debug, Serialize, ToSchema)]
pub struct PasskeyOptions {
    #[schema(value_type = Object)]
    pub public_key: Value,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct NewPasskey {
    /// What the person calls it, e.g. "MacBook" or "YubiKey".
    pub name: String,
    /// `response.clientDataJSON`, base64url.
    pub client_data_json: String,
    /// `response.attestationObject`, base64url.
    pub attestation_object: String,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PasskeySignIn {
    /// The credential's `rawId`, base64url.
    pub credential_id: String,
    pub client_data_json: String,
    pub authenticator_data: String,
    pub signature: String,
    /// `response.userHandle`, base64url.
    pub user_handle: String,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RenamePasskey {
    pub name: String,
}

fn malformed(field: &'static str) -> ApiError {
    ApiError::invalid(field, "malformed", "The browser's response could not be read. Try again.")
}

fn clean_name(name: &str) -> ApiResult<String> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 60 {
        return Err(ApiError::invalid("name", "length", "Use 1 to 60 characters."));
    }
    Ok(name.to_owned())
}

async fn new_challenge(state: &AppState, purpose: &str, account_id: Option<Uuid>) -> ApiResult<Vec<u8>> {
    let challenge = webauthn::new_challenge();
    sqlx::query(
        "INSERT INTO webauthn_challenges (id, challenge_hash, purpose, account_id, expires_at) \
         VALUES ($1, $2, $3, $4, now() + make_interval(secs => $5))",
    )
    .bind(platform_common::new_id())
    .bind(crypto::sha256(&challenge))
    .bind(purpose)
    .bind(account_id)
    .bind(CHALLENGE_TTL_SECS as f64)
    .execute(&state.pool)
    .await?;
    sqlx::query("DELETE FROM webauthn_challenges WHERE expires_at < now() - interval '1 day'").execute(&state.pool).await?;
    Ok(challenge)
}

/// Spend the ceremony a response answers. Returns the challenge and the account it was for
/// (registration only), or `None` if it is unknown, used, expired or for something else.
/// Commits on its own, so a response that then fails its checks cannot be tried again.
async fn take_challenge(
    pool: &sqlx::PgPool,
    client_data_json: &[u8],
    purpose: &str,
) -> ApiResult<Option<(Vec<u8>, Option<Uuid>)>> {
    let Ok(challenge) = webauthn::challenge_of(client_data_json) else { return Ok(None) };
    let found: Option<(Option<Uuid>,)> = sqlx::query_as(
        "UPDATE webauthn_challenges SET used_at = now() \
         WHERE challenge_hash = $1 AND purpose = $2 AND used_at IS NULL AND expires_at > now() RETURNING account_id",
    )
    .bind(crypto::sha256(&challenge))
    .bind(purpose)
    .fetch_optional(pool)
    .await?;
    Ok(found.map(|(account,)| (challenge, account)))
}

#[utoipa::path(get, path = "/api/v1/auth/passkeys", security(("bearer" = [])), responses((status = 200, body = PasskeyList)))]
pub async fn list(State(state): State<AppState>, auth: AuthContext) -> ApiResult<Json<PasskeyList>> {
    require_session(&auth)?;
    let items = sqlx::query_as::<_, PasskeyDto>(
        "SELECT id, name, backed_up AS synced, created_at, last_used_at FROM passkeys WHERE account_id = $1 ORDER BY created_at",
    )
    .bind(auth.user_id)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(PasskeyList { items }))
}

/// Start adding a passkey. The password is asked for again, as for other security changes.
#[utoipa::path(post, path = "/api/v1/auth/passkeys/options", request_body = PasswordConfirmation, security(("bearer" = [])),
    responses((status = 200, body = PasskeyOptions), (status = 409, description = "Too many passkeys"), (status = 422, description = "Wrong password")))]
pub async fn registration_options(
    State(state): State<AppState>,
    auth: AuthContext,
    Json(body): Json<PasswordConfirmation>,
) -> ApiResult<Json<PasskeyOptions>> {
    let account = current_account(&state, &auth).await?;
    confirm_password(&state, &account, body.password).await?;
    let existing: Vec<(Vec<u8>,)> = sqlx::query_as("SELECT credential_id FROM passkeys WHERE account_id = $1")
        .bind(account.id)
        .fetch_all(&state.pool)
        .await?;
    if existing.len() as i64 >= MAX_PASSKEYS {
        return Err(ApiError::Conflict(format!("You can have at most {MAX_PASSKEYS} passkeys. Remove one first.")));
    }
    let challenge = new_challenge(&state, "register", Some(account.id)).await?;
    let rp = &state.settings.passkeys;
    Ok(Json(PasskeyOptions {
        public_key: json!({
            "rp": { "id": rp.id, "name": rp.name },
            "user": { "id": b64(account.id.as_bytes()), "name": account.email, "displayName": account.display_name },
            "challenge": b64(&challenge),
            "pubKeyCredParams": webauthn::ALGORITHMS.iter().map(|alg| json!({ "type": "public-key", "alg": alg })).collect::<Vec<_>>(),
            "timeout": CHALLENGE_TTL_SECS * 1000,
            // The same authenticator cannot be added twice.
            "excludeCredentials": existing.iter().map(|(id,)| json!({ "type": "public-key", "id": b64(id) })).collect::<Vec<_>>(),
            "authenticatorSelection": { "residentKey": "required", "requireResidentKey": true, "userVerification": "required" },
            "attestation": "none",
        }),
    }))
}

/// Finish adding a passkey with the browser's response.
#[utoipa::path(post, path = "/api/v1/auth/passkeys", request_body = NewPasskey, security(("bearer" = [])),
    responses((status = 201, body = PasskeyDto), (status = 409, description = "Already added, or the set-up expired"),
        (status = 422, description = "The response did not check out")))]
pub async fn register(
    State(state): State<AppState>,
    auth: AuthContext,
    Json(body): Json<NewPasskey>,
) -> ApiResult<(StatusCode, Json<PasskeyDto>)> {
    let account = current_account(&state, &auth).await?;
    let name = clean_name(&body.name)?;
    let client_data = unb64(&body.client_data_json).map_err(|_| malformed("client_data_json"))?;
    let attestation = unb64(&body.attestation_object).map_err(|_| malformed("attestation_object"))?;
    let expired = || ApiError::Conflict("Adding the passkey took too long. Start again.".into());

    let (challenge, for_account) = take_challenge(&state.pool, &client_data, "register").await?.ok_or_else(expired)?;
    if for_account != Some(account.id) {
        return Err(expired());
    }
    let credential =
        webauthn::verify_registration(&state.settings.passkeys, &client_data, &attestation, &challenge).map_err(|e| {
            tracing::info!(user_id = %account.id, reason = %e, "passkey registration refused");
            ApiError::invalid(
                "attestation_object",
                "rejected",
                "That passkey could not be added. Try again or use another device.",
            )
        })?;
    let mut tx = state.pool.begin().await?;
    let inserted = sqlx::query_as::<_, PasskeyDto>(
        "INSERT INTO passkeys (id, account_id, credential_id, public_key, algorithm, sign_count, name, backup_eligible, backed_up) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) ON CONFLICT (credential_id) DO NOTHING \
         RETURNING id, name, backed_up AS synced, created_at, last_used_at",
    )
    .bind(platform_common::new_id())
    .bind(account.id)
    .bind(&credential.credential_id)
    .bind(&credential.public_key)
    .bind(credential.algorithm as i32)
    .bind(i64::from(credential.sign_count))
    .bind(name)
    .bind(credential.backup_eligible)
    .bind(credential.backed_up)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| ApiError::Conflict("This passkey has already been added.".into()))?;
    tx.commit().await?;
    tracing::info!(user_id = %account.id, passkey_id = %inserted.id, "passkey added");
    Ok((StatusCode::CREATED, Json(inserted)))
}

#[utoipa::path(patch, path = "/api/v1/auth/passkeys/{id}", request_body = RenamePasskey, security(("bearer" = [])),
    params(("id" = Uuid, Path)), responses((status = 200, body = PasskeyDto), (status = 404)))]
pub async fn rename(
    State(state): State<AppState>,
    auth: AuthContext,
    Path(id): Path<Uuid>,
    Json(body): Json<RenamePasskey>,
) -> ApiResult<Json<PasskeyDto>> {
    require_session(&auth)?;
    let name = clean_name(&body.name)?;
    sqlx::query_as::<_, PasskeyDto>(
        "UPDATE passkeys SET name = $3 WHERE id = $1 AND account_id = $2 RETURNING id, name, backed_up AS synced, created_at, last_used_at",
    )
    .bind(id)
    .bind(auth.user_id)
    .bind(name)
    .fetch_optional(&state.pool)
    .await?
    .map(Json)
    .ok_or(ApiError::NotFound("passkey"))
}

/// Remove a passkey; it stops working at once. Needs the password, like other security changes.
#[utoipa::path(post, path = "/api/v1/auth/passkeys/{id}/remove", request_body = PasswordConfirmation, security(("bearer" = [])),
    params(("id" = Uuid, Path)), responses((status = 204), (status = 404), (status = 422, description = "Wrong password")))]
pub async fn remove(
    State(state): State<AppState>,
    auth: AuthContext,
    Path(id): Path<Uuid>,
    Json(body): Json<PasswordConfirmation>,
) -> ApiResult<StatusCode> {
    let account = current_account(&state, &auth).await?;
    confirm_password(&state, &account, body.password).await?;
    let removed = sqlx::query("DELETE FROM passkeys WHERE id = $1 AND account_id = $2")
        .bind(id)
        .bind(account.id)
        .execute(&state.pool)
        .await?;
    if removed.rows_affected() == 0 {
        return Err(ApiError::NotFound("passkey"));
    }
    tracing::info!(user_id = %account.id, passkey_id = %id, "passkey removed");
    Ok(StatusCode::NO_CONTENT)
}

/// Start signing in with a passkey. Anyone may ask; the answer names no account.
#[utoipa::path(post, path = "/api/v1/auth/passkeys/sign-in/options", responses((status = 200, body = PasskeyOptions)))]
pub async fn sign_in_options(State(state): State<AppState>) -> ApiResult<Json<PasskeyOptions>> {
    let challenge = new_challenge(&state, "sign_in", None).await?;
    Ok(Json(PasskeyOptions {
        public_key: json!({
            "rpId": state.settings.passkeys.id,
            "challenge": b64(&challenge),
            "timeout": CHALLENGE_TTL_SECS * 1000,
            "allowCredentials": [],
            "userVerification": "required",
        }),
    }))
}

#[derive(FromRow)]
struct StoredPasskey {
    id: Uuid,
    account_id: Uuid,
    public_key: Vec<u8>,
    sign_count: i64,
}

/// Finish signing in with a passkey: on success, a session just like a password sign-in
/// with two-step verification.
#[utoipa::path(post, path = "/api/v1/auth/passkeys/sign-in", request_body = PasskeySignIn,
    responses((status = 200, body = TokenResponse), (status = 401, description = "The passkey did not check out")))]
pub async fn sign_in(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(body): Json<PasskeySignIn>,
) -> ApiResult<(CookieJar, Json<TokenResponse>)> {
    let client = client_info(&state, &headers, peer);
    let decode = |v: &str| unb64(v).map_err(|_| SIGN_IN_FAILED);
    let (credential_id, client_data, auth_data, signature, user_handle) = (
        decode(&body.credential_id)?,
        decode(&body.client_data_json)?,
        decode(&body.authenticator_data)?,
        decode(&body.signature)?,
        decode(&body.user_handle)?,
    );

    let Some((challenge, _)) = take_challenge(&state.pool, &client_data, "sign_in").await? else {
        return Err(ApiError::Unauthenticated("This sign-in attempt has expired. Try again."));
    };

    let mut tx = state.pool.begin().await?;
    let stored = sqlx::query_as::<_, StoredPasskey>(
        "SELECT id, account_id, public_key, sign_count FROM passkeys WHERE credential_id = $1 FOR UPDATE",
    )
    .bind(&credential_id)
    .fetch_optional(&mut *tx)
    .await?;
    // The user handle is the account id the passkey was made for; it must be the owner's.
    let Some(stored) = stored.filter(|p| user_handle == p.account_id.as_bytes()) else {
        tracing::info!("passkey sign-in with an unknown credential");
        return Err(SIGN_IN_FAILED);
    };
    let account = store::account_by_id(&state.pool, stored.account_id).await?.ok_or(SIGN_IN_FAILED)?;
    let email_hash = crypto::sha256(account.email.to_lowercase().as_bytes());
    if account.status != "active" {
        log_event(&state.pool, account.id, &email_hash, "suspended", &client).await;
        return Err(ApiError::Forbidden);
    }
    // A locked account stays locked: the lockout is about this account being attacked.
    if account.locked_until.is_some_and(|until| until > OffsetDateTime::now_utc()) {
        log_event(&state.pool, account.id, &email_hash, "locked", &client).await;
        return Err(SIGN_IN_FAILED);
    }
    let stored_count = u32::try_from(stored.sign_count).unwrap_or(u32::MAX);
    let verified = webauthn::verify_assertion(
        &state.settings.passkeys,
        &client_data,
        &auth_data,
        &signature,
        &challenge,
        &stored.public_key,
        stored_count,
    );
    let assertion = match verified {
        Ok(assertion) => assertion,
        Err(error) => {
            if error == webauthn::Error::CounterWentBack {
                tracing::warn!(user_id = %account.id, passkey_id = %stored.id, "passkey signature counter went back; possible clone");
            } else {
                tracing::info!(user_id = %account.id, reason = %error, "passkey sign-in refused");
            }
            drop(tx);
            log_event(&state.pool, account.id, &email_hash, "passkey_failed", &client).await;
            return Err(SIGN_IN_FAILED);
        }
    };

    sqlx::query("UPDATE passkeys SET sign_count = $2, backed_up = $3, last_used_at = now() WHERE id = $1")
        .bind(stored.id)
        .bind(i64::from(assertion.sign_count))
        .bind(assertion.backed_up)
        .execute(&mut *tx)
        .await?;
    store::reset_failed_logins(&mut tx, account.id).await?;
    let (session_id, refresh) = start_session(&mut tx, &state, &account, &client, "passkey").await?;
    outbox::enqueue(
        &mut tx,
        &EventEnvelope::new(None, account.id, 1, Actor::User(account.id), LoginSucceededV1 { user_id: account.id, session_id }),
    )
    .await?;
    tx.commit().await?;
    log_event(&state.pool, account.id, &email_hash, "success", &client).await;
    let response = issue_access_token(&state, &account, session_id, None)?;
    Ok((jar.add(refresh_cookie(&state, refresh)), Json(response)))
}
