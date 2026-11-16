//! Passkeys: checking WebAuthn (Level 2) registrations and assertions. Pure functions over
//! the bytes the browser hands back; challenges and credentials are stored in `http::passkeys`.
//!
//! What is checked, following the "verifying" procedures in §7.1 and §7.2 of the spec: the
//! client data's type, challenge and origin (an exact match against our origins, never a
//! cross-origin iframe); the relying party id hash; user presence *and* user verification
//! (a PIN or biometric, which is what lets a passkey stand in for password plus second step);
//! the signature over `authenticatorData || SHA-256(clientDataJSON)`; and the signature
//! counter, which must go up whenever either side is non-zero (a cloned authenticator).
//!
//! Attestation is requested as `none` and any statement sent anyway is ignored: we do not
//! restrict which authenticator makes are allowed, so nothing relies on it.
//!
//! Supported keys: ES256 (P-256), EdDSA (Ed25519) and RS256 (RSA 2048–8192), verified with
//! `ring`.

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use ring::signature::{self, RsaPublicKeyComponents, UnparsedPublicKey};
use serde::Deserialize;
use subtle::ConstantTimeEq;

use crate::{cbor, crypto};

/// COSE algorithm ids we accept, in order of preference.
pub const ALGORITHMS: [i64; 3] = [ES256, EDDSA, RS256];
const ES256: i64 = -7;
const EDDSA: i64 = -8;
const RS256: i64 = -257;

const FLAG_UP: u8 = 0x01;
const FLAG_UV: u8 = 0x04;
const FLAG_BE: u8 = 0x08;
const FLAG_BS: u8 = 0x10;
const FLAG_AT: u8 = 0x40;
const FLAG_ED: u8 = 0x80;

pub const MAX_CREDENTIAL_ID: usize = 1023;

/// Who we are to authenticators: the domain passkeys are bound to, and the exact origins
/// (scheme, host and port) pages may ask from.
#[derive(Debug, Clone)]
pub struct RelyingParty {
    pub id: String,
    pub name: String,
    pub origins: Vec<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    Malformed(&'static str),
    WrongType,
    WrongChallenge,
    WrongOrigin,
    WrongRelyingParty,
    NotPresent,
    NotVerified,
    UnsupportedKey,
    BadSignature,
    /// The counter did not go up: the authenticator may have been cloned.
    CounterWentBack,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for Error {}

impl From<cbor::Error> for Error {
    fn from(e: cbor::Error) -> Self {
        Error::Malformed(e.0)
    }
}

/// The host of an origin such as `https://app.example.com:8443`.
pub fn host_of(origin: &str) -> Option<String> {
    let rest = origin.split_once("://")?.1;
    let host = rest.split(['/', ':']).next()?;
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

/// Whether pages on `origin` may use passkeys bound to `rp_id`: the same host or a
/// subdomain of it, over https (plain http only for localhost, as browsers allow).
pub fn origin_matches_rp(origin: &str, rp_id: &str) -> bool {
    let Some(host) = host_of(origin) else { return false };
    let secure = origin.starts_with("https://") || (origin.starts_with("http://") && host == "localhost");
    secure && (host == rp_id || host.ends_with(&format!(".{rp_id}")))
}

/// A random challenge for one ceremony.
pub fn new_challenge() -> Vec<u8> {
    let mut bytes = vec![0_u8; 32];
    rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut bytes);
    bytes
}

pub fn b64(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

pub fn unb64(text: &str) -> Result<Vec<u8>, Error> {
    URL_SAFE_NO_PAD.decode(text).map_err(|_| Error::Malformed("base64url"))
}

#[derive(Deserialize)]
struct ClientData {
    #[serde(rename = "type")]
    kind: String,
    challenge: String,
    origin: String,
    #[serde(default, rename = "crossOrigin")]
    cross_origin: bool,
}

/// The challenge a response answers, so the caller can find the ceremony it belongs to.
/// Nothing else about the response is trusted yet.
pub fn challenge_of(client_data_json: &[u8]) -> Result<Vec<u8>, Error> {
    let data: ClientData = serde_json::from_slice(client_data_json).map_err(|_| Error::Malformed("client data"))?;
    unb64(&data.challenge)
}

fn check_client_data(rp: &RelyingParty, json: &[u8], kind: &str, expected_challenge: &[u8]) -> Result<(), Error> {
    let data: ClientData = serde_json::from_slice(json).map_err(|_| Error::Malformed("client data"))?;
    if data.kind != kind {
        return Err(Error::WrongType);
    }
    let challenge = unb64(&data.challenge)?;
    if !bool::from(challenge.ct_eq(expected_challenge)) {
        return Err(Error::WrongChallenge);
    }
    if data.cross_origin || !rp.origins.iter().any(|o| o == &data.origin) {
        return Err(Error::WrongOrigin);
    }
    Ok(())
}

struct AuthData<'a> {
    flags: u8,
    sign_count: u32,
    /// `(credential id, COSE public key)` when the AT flag is set.
    attested: Option<(&'a [u8], &'a [u8])>,
}

fn parse_auth_data<'a>(rp: &RelyingParty, bytes: &'a [u8]) -> Result<AuthData<'a>, Error> {
    if bytes.len() < 37 {
        return Err(Error::Malformed("authenticator data too short"));
    }
    if !bool::from(bytes[..32].ct_eq(&crypto::sha256(rp.id.as_bytes()))) {
        return Err(Error::WrongRelyingParty);
    }
    let flags = bytes[32];
    let sign_count = u32::from_be_bytes([bytes[33], bytes[34], bytes[35], bytes[36]]);
    if flags & FLAG_UP == 0 {
        return Err(Error::NotPresent);
    }
    if flags & FLAG_UV == 0 {
        return Err(Error::NotVerified);
    }
    let rest = &bytes[37..];
    let mut used = 0;
    let attested = if flags & FLAG_AT != 0 {
        // aaguid (16) | credential id length (2) | credential id | COSE key
        if rest.len() < 18 {
            return Err(Error::Malformed("attested credential data too short"));
        }
        let id_len = usize::from(u16::from_be_bytes([rest[16], rest[17]]));
        if id_len == 0 || id_len > MAX_CREDENTIAL_ID || rest.len() < 18 + id_len {
            return Err(Error::Malformed("credential id"));
        }
        let id = &rest[18..18 + id_len];
        let key_start = 18 + id_len;
        let (_, key_len) = cbor::decode_prefix(&rest[key_start..])?;
        used = key_start + key_len;
        Some((id, &rest[key_start..used]))
    } else {
        None
    };
    if flags & FLAG_ED != 0 {
        // Extension outputs: one CBOR map. We read none of them, only check its shape.
        let (value, len) = cbor::decode_prefix(&rest[used..])?;
        if value.as_map().is_none() {
            return Err(Error::Malformed("extensions"));
        }
        used += len;
    }
    if used != rest.len() {
        return Err(Error::Malformed("trailing authenticator data"));
    }
    Ok(AuthData { flags, sign_count, attested })
}

/// A credential public key, parsed from its COSE form.
pub enum PublicKey {
    Es256(Vec<u8>),
    EdDsa(Vec<u8>),
    Rs256 { n: Vec<u8>, e: Vec<u8> },
}

impl PublicKey {
    pub fn from_cose(bytes: &[u8]) -> Result<(Self, i64), Error> {
        let value = cbor::decode(bytes)?;
        let map = value.as_map().ok_or(Error::Malformed("COSE key"))?;
        let get = |k: i128| map.get(&cbor::Value::Int(k));
        let int = |k: i128| get(k).and_then(cbor::Value::as_int);
        let bytes_of = |k: i128| get(k).and_then(cbor::Value::as_bytes).map(<[u8]>::to_vec);
        let alg = int(3).and_then(|a| i64::try_from(a).ok()).ok_or(Error::UnsupportedKey)?;
        let key = match (int(1), alg) {
            // EC2 on P-256: an uncompressed point.
            (Some(2), ES256) => {
                let (x, y) = (bytes_of(-2).ok_or(Error::UnsupportedKey)?, bytes_of(-3).ok_or(Error::UnsupportedKey)?);
                if int(-1) != Some(1) || x.len() != 32 || y.len() != 32 {
                    return Err(Error::UnsupportedKey);
                }
                PublicKey::Es256([&[0x04][..], &x, &y].concat())
            }
            // OKP on Ed25519.
            (Some(1), EDDSA) => {
                let x = bytes_of(-2).ok_or(Error::UnsupportedKey)?;
                if int(-1) != Some(6) || x.len() != 32 {
                    return Err(Error::UnsupportedKey);
                }
                PublicKey::EdDsa(x)
            }
            (Some(3), RS256) => {
                let (n, e) = (bytes_of(-1).ok_or(Error::UnsupportedKey)?, bytes_of(-2).ok_or(Error::UnsupportedKey)?);
                PublicKey::Rs256 { n, e }
            }
            _ => return Err(Error::UnsupportedKey),
        };
        Ok((key, alg))
    }

    pub fn verify(&self, message: &[u8], sig: &[u8]) -> bool {
        match self {
            PublicKey::Es256(point) => {
                UnparsedPublicKey::new(&signature::ECDSA_P256_SHA256_ASN1, point).verify(message, sig).is_ok()
            }
            PublicKey::EdDsa(x) => UnparsedPublicKey::new(&signature::ED25519, x).verify(message, sig).is_ok(),
            PublicKey::Rs256 { n, e } => {
                RsaPublicKeyComponents { n, e }.verify(&signature::RSA_PKCS1_2048_8192_SHA256, message, sig).is_ok()
            }
        }
    }
}

/// A passkey that has just been created and checked.
#[derive(Debug)]
pub struct NewCredential {
    pub credential_id: Vec<u8>,
    /// The COSE public key, stored as received.
    pub public_key: Vec<u8>,
    pub algorithm: i64,
    pub sign_count: u32,
    /// Whether it can be synced to the person's other devices, and whether it has been.
    pub backup_eligible: bool,
    pub backed_up: bool,
}

/// Check the response to `navigator.credentials.create()`.
pub fn verify_registration(
    rp: &RelyingParty,
    client_data_json: &[u8],
    attestation_object: &[u8],
    expected_challenge: &[u8],
) -> Result<NewCredential, Error> {
    check_client_data(rp, client_data_json, "webauthn.create", expected_challenge)?;
    let attestation = cbor::decode(attestation_object)?;
    let map = attestation.as_map().ok_or(Error::Malformed("attestation object"))?;
    let field = |name: &str| map.get(&cbor::Value::Text(name.into()));
    field("fmt").and_then(cbor::Value::as_text).ok_or(Error::Malformed("attestation format"))?;
    let auth_data = field("authData").and_then(cbor::Value::as_bytes).ok_or(Error::Malformed("authenticator data"))?;
    let parsed = parse_auth_data(rp, auth_data)?;
    let (id, key) = parsed.attested.ok_or(Error::Malformed("no attested credential"))?;
    let (_, algorithm) = PublicKey::from_cose(key)?;
    if !ALGORITHMS.contains(&algorithm) {
        return Err(Error::UnsupportedKey);
    }
    Ok(NewCredential {
        credential_id: id.to_vec(),
        public_key: key.to_vec(),
        algorithm,
        sign_count: parsed.sign_count,
        backup_eligible: parsed.flags & FLAG_BE != 0,
        backed_up: parsed.flags & FLAG_BS != 0,
    })
}

/// What a successful sign-in changes about the stored passkey.
#[derive(Debug, PartialEq, Eq)]
pub struct Assertion {
    pub sign_count: u32,
    pub backed_up: bool,
}

/// Check the response to `navigator.credentials.get()` against a stored passkey.
pub fn verify_assertion(
    rp: &RelyingParty,
    client_data_json: &[u8],
    authenticator_data: &[u8],
    sig: &[u8],
    expected_challenge: &[u8],
    stored_public_key: &[u8],
    stored_sign_count: u32,
) -> Result<Assertion, Error> {
    check_client_data(rp, client_data_json, "webauthn.get", expected_challenge)?;
    let parsed = parse_auth_data(rp, authenticator_data)?;
    let (key, _) = PublicKey::from_cose(stored_public_key)?;
    let message = [authenticator_data, &crypto::sha256(client_data_json)].concat();
    if !key.verify(&message, sig) {
        return Err(Error::BadSignature);
    }
    if (parsed.sign_count != 0 || stored_sign_count != 0) && parsed.sign_count <= stored_sign_count {
        return Err(Error::CounterWentBack);
    }
    Ok(Assertion { sign_count: parsed.sign_count, backed_up: parsed.flags & FLAG_BS != 0 })
}

#[cfg(test)]
#[path = "../tests/support/soft_authenticator.rs"]
mod soft_authenticator;

#[cfg(test)]
mod tests {
    use super::{soft_authenticator::SoftAuthenticator, *};

    const ORIGIN: &str = "https://app.example.com";

    fn rp() -> RelyingParty {
        RelyingParty { id: "app.example.com".into(), name: "Meridian".into(), origins: vec![ORIGIN.into()] }
    }

    fn registered() -> (SoftAuthenticator, NewCredential) {
        let mut device = SoftAuthenticator::new("app.example.com", b"user-handle");
        let challenge = new_challenge();
        let (client, attestation) = device.create(ORIGIN, &challenge);
        let credential = verify_registration(&rp(), &client, &attestation, &challenge).unwrap();
        (device, credential)
    }

    #[test]
    fn registers_and_signs_in_with_a_p256_passkey() {
        let (mut device, credential) = registered();
        assert_eq!(credential.credential_id, device.credential_id);
        assert_eq!(credential.algorithm, ES256);
        assert!(credential.backup_eligible && credential.backed_up);

        let challenge = new_challenge();
        let (client, auth_data, sig) = device.get(ORIGIN, &challenge);
        assert_eq!(challenge_of(&client).unwrap(), challenge);
        let out = verify_assertion(&rp(), &client, &auth_data, &sig, &challenge, &credential.public_key, 0).unwrap();
        assert_eq!(out.sign_count, 1);
        // The same response again: the counter has not moved past what is stored.
        assert_eq!(
            verify_assertion(&rp(), &client, &auth_data, &sig, &challenge, &credential.public_key, 1),
            Err(Error::CounterWentBack)
        );
    }

    #[test]
    fn refuses_responses_for_someone_else() {
        let (mut device, credential) = registered();
        let challenge = new_challenge();
        let check = |client: &[u8], data: &[u8], sig: &[u8]| {
            verify_assertion(&rp(), client, data, sig, &challenge, &credential.public_key, 0)
        };

        let (client, data, sig) = device.get("https://evil.example.net", &challenge);
        assert_eq!(check(&client, &data, &sig), Err(Error::WrongOrigin));

        let (client, data, sig) = device.get(ORIGIN, &new_challenge());
        assert_eq!(check(&client, &data, &sig), Err(Error::WrongChallenge));

        let mut other_site = SoftAuthenticator::new("evil.example.net", b"user-handle");
        let (client, data, sig) = other_site.get(ORIGIN, &challenge);
        assert_eq!(check(&client, &data, &sig), Err(Error::WrongRelyingParty));

        // A different key signing for the same credential.
        let mut impostor = SoftAuthenticator::new("app.example.com", b"user-handle");
        let (client, data, sig) = impostor.get(ORIGIN, &challenge);
        assert_eq!(check(&client, &data, &sig), Err(Error::BadSignature));

        // Tampered client data no longer matches the signature.
        let (client, data, sig) = device.get(ORIGIN, &challenge);
        let mut tampered = client.clone();
        tampered.extend_from_slice(b" ");
        assert_eq!(check(&tampered, &data, &sig), Err(Error::BadSignature));

        // A registration response is not a sign-in.
        let (client, _) = device.create(ORIGIN, &challenge);
        let (_, data, sig) = device.get(ORIGIN, &challenge);
        assert_eq!(check(&client, &data, &sig), Err(Error::WrongType));
    }

    #[test]
    fn requires_presence_and_verification() {
        let (mut device, credential) = registered();
        let challenge = new_challenge();
        device.flags &= !FLAG_UV;
        let (client, data, sig) = device.get(ORIGIN, &challenge);
        assert_eq!(verify_assertion(&rp(), &client, &data, &sig, &challenge, &credential.public_key, 0), Err(Error::NotVerified));
        device.flags &= !FLAG_UP;
        let (client, attestation) = device.create(ORIGIN, &challenge);
        assert_eq!(verify_registration(&rp(), &client, &attestation, &challenge).unwrap_err(), Error::NotPresent);
    }

    #[test]
    fn origins_must_sit_on_the_passkey_domain() {
        assert_eq!(host_of("https://App.Example.com:8443/x").as_deref(), Some("app.example.com"));
        assert!(origin_matches_rp("https://app.example.com", "example.com"));
        assert!(origin_matches_rp("http://localhost:5173", "localhost"));
        assert!(!origin_matches_rp("http://app.example.com", "example.com"));
        assert!(!origin_matches_rp("https://badexample.com", "example.com"));
    }

    #[test]
    fn verifies_ed25519_keys() {
        use ring::signature::{Ed25519KeyPair, KeyPair};
        let rng = ring::rand::SystemRandom::new();
        let pair = Ed25519KeyPair::from_pkcs8(Ed25519KeyPair::generate_pkcs8(&rng).unwrap().as_ref()).unwrap();
        let x = pair.public_key().as_ref().to_vec();
        // {1: 1, 3: -8, -1: 6, -2: x}
        let cose = [&[0xa4, 0x01, 0x01, 0x03, 0x27, 0x20, 0x06, 0x21, 0x58, 0x20][..], &x].concat();
        let (key, alg) = PublicKey::from_cose(&cose).unwrap();
        assert_eq!(alg, EDDSA);
        assert!(key.verify(b"message", pair.sign(b"message").as_ref()));
        assert!(!key.verify(b"other", pair.sign(b"message").as_ref()));
    }

    #[test]
    fn refuses_cross_origin_iframes_and_junk() {
        let mut device = SoftAuthenticator::new("app.example.com", b"user-handle");
        let challenge = new_challenge();
        device.cross_origin = true;
        let (client, attestation) = device.create(ORIGIN, &challenge);
        assert_eq!(verify_registration(&rp(), &client, &attestation, &challenge).unwrap_err(), Error::WrongOrigin);
        assert!(matches!(verify_registration(&rp(), b"{}", &attestation, &challenge), Err(Error::Malformed(_))));
        assert!(verify_registration(&rp(), &client, &[0xa0], &challenge).is_err());
        assert!(PublicKey::from_cose(&[0xa1, 0x01, 0x02]).is_err());
    }
}
