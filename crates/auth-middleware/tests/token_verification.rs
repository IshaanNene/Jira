//! Token misuse cases: every one of these must be rejected.

use auth_middleware::{
    generate_ed25519, AuthContext, Claims, Jwk, OrgContext, TokenKind, TokenSigner, TokenVerifier, API_AUDIENCE,
};
use uuid::Uuid;

const ISSUER: &str = "http://identity.test";

fn claims() -> Claims {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
    Claims {
        iss: ISSUER.into(),
        aud: API_AUDIENCE.into(),
        sub: Uuid::now_v7(),
        sid: Uuid::now_v7(),
        jti: Uuid::now_v7(),
        iat: now,
        exp: now + 600,
        typ: TokenKind::Account,
        email: "a@example.com".into(),
        name: "A".into(),
        tid: None,
        org: None,
        roles: vec![],
        pv: 0,
        api: false,
    }
}

fn setup() -> (TokenSigner, TokenVerifier) {
    let key = generate_ed25519().unwrap();
    let verifier = TokenVerifier::local(ISSUER);
    verifier.add_key(&Jwk::ed25519("k1", key.public_x.clone())).unwrap();
    (TokenSigner::new("k1", &key.pkcs8), verifier)
}

#[tokio::test]
async fn valid_token_verifies() {
    let (signer, verifier) = setup();
    let c = claims();
    let verified = verifier.verify(&signer.sign(&c).unwrap()).await.unwrap();
    assert_eq!(verified.sub, c.sub);
}

#[tokio::test]
async fn expired_token_is_rejected() {
    let (signer, verifier) = setup();
    let mut c = claims();
    c.iat -= 3600;
    c.exp = c.iat + 600;
    assert!(verifier.verify(&signer.sign(&c).unwrap()).await.is_err());
}

#[tokio::test]
async fn wrong_issuer_and_audience_are_rejected() {
    let (signer, verifier) = setup();
    let mut c = claims();
    c.iss = "http://evil.test".into();
    assert!(verifier.verify(&signer.sign(&c).unwrap()).await.is_err());
    let mut c = claims();
    c.aud = "another-api".into();
    assert!(verifier.verify(&signer.sign(&c).unwrap()).await.is_err());
}

#[tokio::test]
async fn token_signed_by_unknown_key_is_rejected() {
    let (_, verifier) = setup();
    let other = generate_ed25519().unwrap();
    // Same kid, different key: signature must not verify.
    let forged = TokenSigner::new("k1", &other.pkcs8).sign(&claims()).unwrap();
    assert!(verifier.verify(&forged).await.is_err());
    let unknown_kid = TokenSigner::new("k2", &other.pkcs8).sign(&claims()).unwrap();
    assert!(verifier.verify(&unknown_kid).await.is_err());
}

#[tokio::test]
async fn tampered_payload_is_rejected() {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    let (signer, verifier) = setup();
    let token = signer.sign(&claims()).unwrap();
    let parts: Vec<&str> = token.split('.').collect();
    let mut payload: serde_json::Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[1]).unwrap()).unwrap();
    payload["typ"] = "org".into();
    payload["tid"] = Uuid::now_v7().to_string().into();
    payload["roles"] = serde_json::json!(["owner"]);
    let forged = format!("{}.{}.{}", parts[0], URL_SAFE_NO_PAD.encode(payload.to_string()), parts[2]);
    assert!(verifier.verify(&forged).await.is_err());
}

#[tokio::test]
async fn alg_none_is_rejected() {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    let (_, verifier) = setup();
    let header = URL_SAFE_NO_PAD.encode(r#"{"alg":"none","kid":"k1"}"#);
    let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims()).unwrap());
    assert!(verifier.verify(&format!("{header}.{payload}.")).await.is_err());
}

fn ctx(c: Claims) -> AuthContext {
    AuthContext { user_id: c.sub, session_id: c.sid, email: c.email.clone(), name: c.name.clone(), claims: c }
}

#[test]
fn account_token_cannot_become_org_context() {
    assert!(OrgContext::try_from(ctx(claims())).is_err());
}

#[test]
fn org_token_with_unknown_role_gets_no_access() {
    let mut c = claims();
    c.typ = TokenKind::Org;
    c.tid = Some(Uuid::now_v7());
    c.org = Some(Uuid::now_v7());
    c.roles = vec!["superuser".into()];
    assert!(OrgContext::try_from(ctx(c.clone())).is_err());
    c.roles = vec!["member".into(), "admin".into()];
    let org = OrgContext::try_from(ctx(c)).unwrap();
    assert_eq!(org.role, auth_middleware::OrgRole::Admin);
}
