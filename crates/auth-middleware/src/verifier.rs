use std::{
    collections::HashMap,
    sync::RwLock,
    time::{Duration, Instant},
};

use error_handling::ApiError;
use jsonwebtoken::{Algorithm, DecodingKey, Validation};
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::{
    claims::{Claims, API_AUDIENCE},
    keys::{Jwk, JwkSet},
    redis_handle::RedisHandle,
};

const MIN_REFRESH_INTERVAL: Duration = Duration::from_secs(10);

/// Verifies access tokens locally. Keys come from the issuer's JWKS endpoint and are
/// refreshed when an unknown `kid` is seen (rate-limited so bad tokens cannot be used
/// to hammer the identity service).
pub struct TokenVerifier {
    issuer: String,
    keys: RwLock<HashMap<String, DecodingKey>>,
    jwks_url: Option<String>,
    http: reqwest::Client,
    last_refresh: Mutex<Option<Instant>>,
    revocations: Option<RedisHandle>,
}

impl TokenVerifier {
    /// Verifier that fetches keys from `jwks_url` on demand.
    pub fn remote(issuer: impl Into<String>, jwks_url: impl Into<String>) -> anyhow::Result<Self> {
        Ok(Self {
            issuer: issuer.into(),
            keys: RwLock::new(HashMap::new()),
            jwks_url: Some(jwks_url.into()),
            http: reqwest::Client::builder().timeout(Duration::from_secs(3)).build()?,
            last_refresh: Mutex::new(None),
            revocations: None,
        })
    }

    /// Verifier with keys supplied directly (the identity service itself, and tests).
    pub fn local(issuer: impl Into<String>) -> Self {
        Self {
            issuer: issuer.into(),
            keys: RwLock::new(HashMap::new()),
            jwks_url: None,
            http: reqwest::Client::new(),
            last_refresh: Mutex::new(None),
            revocations: None,
        }
    }

    /// Enable the session revocation list (`revoked:{sid}` keys in Redis).
    #[must_use]
    pub fn with_revocations(mut self, redis: RedisHandle) -> Self {
        self.revocations = Some(redis);
        self
    }

    pub fn add_key(&self, jwk: &Jwk) -> anyhow::Result<()> {
        if jwk.kty != "OKP" || jwk.crv != "Ed25519" {
            anyhow::bail!("unsupported key type {}/{}", jwk.kty, jwk.crv);
        }
        let key = DecodingKey::from_ed_components(&jwk.x)?;
        if let Ok(mut keys) = self.keys.write() {
            keys.insert(jwk.kid.clone(), key);
        }
        Ok(())
    }

    fn key(&self, kid: &str) -> Option<DecodingKey> {
        self.keys.read().ok().and_then(|k| k.get(kid).cloned())
    }

    async fn refresh_keys(&self) {
        let Some(url) = &self.jwks_url else { return };
        let mut last = self.last_refresh.lock().await;
        if last.is_some_and(|t| t.elapsed() < MIN_REFRESH_INTERVAL) {
            return;
        }
        *last = Some(Instant::now());
        match self.http.get(url).send().await.and_then(reqwest::Response::error_for_status) {
            Ok(resp) => match resp.json::<JwkSet>().await {
                Ok(set) => {
                    for jwk in &set.keys {
                        if let Err(err) = self.add_key(jwk) {
                            tracing::warn!(kid = %jwk.kid, error = %err, "ignoring unusable signing key");
                        }
                    }
                }
                Err(err) => tracing::warn!(error = %err, "jwks response was not valid"),
            },
            Err(err) => tracing::warn!(error = %err, "failed to fetch jwks"),
        }
    }

    /// Verify signature, issuer, audience and expiry, then the revocation list.
    pub async fn verify(&self, token: &str) -> Result<Claims, ApiError> {
        const INVALID: ApiError = ApiError::Unauthenticated("The access token is invalid or expired.");

        let header = jsonwebtoken::decode_header(token).map_err(|_| INVALID)?;
        if header.alg != Algorithm::EdDSA {
            return Err(INVALID);
        }
        let kid = header.kid.ok_or(INVALID)?;
        let key = match self.key(&kid) {
            Some(k) => k,
            None => {
                self.refresh_keys().await;
                match self.key(&kid) {
                    Some(k) => k,
                    None if self.jwks_url.is_some() && self.keys.read().map(|k| k.is_empty()).unwrap_or(true) => {
                        return Err(ApiError::DependencyUnavailable("identity-service"));
                    }
                    None => return Err(INVALID),
                }
            }
        };

        let mut validation = Validation::new(Algorithm::EdDSA);
        validation.set_issuer(&[self.issuer.as_str()]);
        validation.set_audience(&[API_AUDIENCE]);
        validation.leeway = 5;
        let claims = jsonwebtoken::decode::<Claims>(token, &key, &validation).map_err(|_| INVALID)?.claims;

        if self.is_revoked(claims.sid).await {
            return Err(ApiError::Unauthenticated("This session has been signed out."));
        }
        Ok(claims)
    }

    /// Redis being unreachable must not take the whole API down: the check fails open and
    /// the token's short lifetime bounds the exposure (see SECURITY_DESIGN.md §2).
    async fn is_revoked(&self, sid: Uuid) -> bool {
        let Some(mut conn) = self.revocations.as_ref().and_then(RedisHandle::get) else {
            return false;
        };
        let mut cmd = redis::cmd("EXISTS");
        cmd.arg(revocation_key(sid));
        let lookup = cmd.query_async::<bool>(&mut conn);
        match tokio::time::timeout(Duration::from_millis(250), lookup).await {
            Ok(Ok(revoked)) => revoked,
            Ok(Err(err)) => {
                tracing::warn!(error = %err, "revocation check failed; allowing until token expiry");
                false
            }
            Err(_) => {
                tracing::warn!("revocation check timed out; allowing until token expiry");
                false
            }
        }
    }
}

pub fn revocation_key(sid: Uuid) -> String {
    format!("revoked:{sid}")
}
