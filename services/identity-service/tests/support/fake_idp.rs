//! A small OpenID Connect provider for tests, on a local port: discovery, keys, and a token
//! endpoint that checks the client secret and PKCE and returns a real ES256-signed ID token.
//! "Signing in" at it is done by the test with `approve`, which hands back a code.
#![allow(dead_code)]

use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{Arc, Mutex},
};

use axum::{
    extract::{Form, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post},
    Json, Router,
};
use base64::{
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
    Engine,
};
use ring::{
    rand::SystemRandom,
    signature::{EcdsaKeyPair, KeyPair, ECDSA_P256_SHA256_FIXED_SIGNING},
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

pub const CLIENT_ID: &str = "meridian-test";
pub const CLIENT_SECRET: &str = "test-client-secret";

struct Grant {
    subject: String,
    email: String,
    nonce: String,
    challenge: String,
}

struct Inner {
    issuer: String,
    key: EcdsaKeyPair,
    grants: Mutex<HashMap<String, Grant>>,
    /// Sign tokens with a different key than the published one.
    pub forge: Mutex<bool>,
}

#[derive(Clone)]
pub struct FakeIdp(Arc<Inner>);

fn query_param(url: &str, name: &str) -> String {
    let url = reqwest::Url::parse(url).unwrap();
    url.query_pairs().find(|(k, _)| k == name).map(|(_, v)| v.into_owned()).unwrap_or_default()
}

impl FakeIdp {
    pub async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr: SocketAddr = listener.local_addr().unwrap();
        let rng = SystemRandom::new();
        let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng).unwrap();
        let key = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8.as_ref(), &rng).unwrap();
        let idp = FakeIdp(Arc::new(Inner {
            issuer: format!("http://{addr}"),
            key,
            grants: Mutex::new(HashMap::new()),
            forge: Mutex::new(false),
        }));
        let app = Router::new()
            .route("/.well-known/openid-configuration", get(discovery))
            .route("/jwks", get(jwks))
            .route("/token", post(token))
            .with_state(idp.clone());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        idp
    }

    pub fn issuer(&self) -> String {
        self.0.issuer.clone()
    }

    pub fn set_forge(&self, forge: bool) {
        *self.0.forge.lock().unwrap() = forge;
    }

    /// The person signs in at the provider as `subject`; returns `(state, code)` for the
    /// redirect back. `nonce` overrides the one from the authorization URL.
    pub fn approve(&self, authorization_url: &str, subject: &str, email: &str, nonce: Option<&str>) -> (String, String) {
        assert!(authorization_url.starts_with(&self.0.issuer), "{authorization_url}");
        assert_eq!(query_param(authorization_url, "client_id"), CLIENT_ID);
        assert_eq!(query_param(authorization_url, "code_challenge_method"), "S256");
        let code = format!("code-{}", uuid::Uuid::now_v7());
        self.0.grants.lock().unwrap().insert(
            code.clone(),
            Grant {
                subject: subject.into(),
                email: email.into(),
                nonce: nonce.map_or_else(|| query_param(authorization_url, "nonce"), str::to_owned),
                challenge: query_param(authorization_url, "code_challenge"),
            },
        );
        (query_param(authorization_url, "state"), code)
    }

    fn sign(&self, claims: &Value) -> String {
        let header = URL_SAFE_NO_PAD.encode(json!({ "alg": "ES256", "kid": "k1", "typ": "JWT" }).to_string());
        let payload = URL_SAFE_NO_PAD.encode(claims.to_string());
        let input = format!("{header}.{payload}");
        let rng = SystemRandom::new();
        let signature = if *self.0.forge.lock().unwrap() {
            let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng).unwrap();
            let other = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8.as_ref(), &rng).unwrap();
            other.sign(&rng, input.as_bytes()).unwrap()
        } else {
            self.0.key.sign(&rng, input.as_bytes()).unwrap()
        };
        format!("{input}.{}", URL_SAFE_NO_PAD.encode(signature.as_ref()))
    }
}

async fn discovery(State(idp): State<FakeIdp>) -> Json<Value> {
    let issuer = &idp.0.issuer;
    Json(json!({
        "issuer": issuer,
        "authorization_endpoint": format!("{issuer}/authorize"),
        "token_endpoint": format!("{issuer}/token"),
        "jwks_uri": format!("{issuer}/jwks"),
        "response_types_supported": ["code"],
        "subject_types_supported": ["public"],
        "id_token_signing_alg_values_supported": ["ES256"],
    }))
}

async fn jwks(State(idp): State<FakeIdp>) -> Json<Value> {
    let point = idp.0.key.public_key().as_ref();
    Json(json!({ "keys": [{
        "kty": "EC", "crv": "P-256", "kid": "k1", "use": "sig", "alg": "ES256",
        "x": URL_SAFE_NO_PAD.encode(&point[1..33]), "y": URL_SAFE_NO_PAD.encode(&point[33..65]),
    }]}))
}

async fn token(
    State(idp): State<FakeIdp>,
    headers: HeaderMap,
    Form(form): Form<HashMap<String, String>>,
) -> (StatusCode, Json<Value>) {
    let refuse = |error: &str| (StatusCode::BAD_REQUEST, Json(json!({ "error": error })));
    let expected = format!("Basic {}", STANDARD.encode(format!("{CLIENT_ID}:{CLIENT_SECRET}")));
    if headers.get("authorization").and_then(|v| v.to_str().ok()) != Some(expected.as_str()) {
        return refuse("invalid_client");
    }
    let Some(grant) = form.get("code").and_then(|c| idp.0.grants.lock().unwrap().remove(c)) else {
        return refuse("invalid_grant");
    };
    let verifier = form.get("code_verifier").cloned().unwrap_or_default();
    if URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes())) != grant.challenge {
        return refuse("invalid_grant");
    }
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    let id_token = idp.sign(&json!({
        "iss": idp.0.issuer, "sub": grant.subject, "aud": CLIENT_ID, "iat": now, "exp": now + 300,
        "nonce": grant.nonce, "email": grant.email, "email_verified": true,
    }));
    (StatusCode::OK, Json(json!({ "access_token": "at", "token_type": "Bearer", "expires_in": 300, "id_token": id_token })))
}
