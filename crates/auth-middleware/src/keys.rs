//! Ed25519 signing keys and JWKS representation. Key generation and signing are
//! delegated to `ring` / `jsonwebtoken`; nothing cryptographic is implemented here.

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use ring::{
    rand::SystemRandom,
    signature::{Ed25519KeyPair, KeyPair},
};
use serde::{Deserialize, Serialize};

use crate::claims::Claims;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Jwk {
    pub kty: String,
    pub crv: String,
    pub x: String,
    pub kid: String,
    pub alg: String,
    #[serde(rename = "use")]
    pub use_: String,
}

impl Jwk {
    pub fn ed25519(kid: impl Into<String>, public_x: impl Into<String>) -> Self {
        Self {
            kty: "OKP".into(),
            crv: "Ed25519".into(),
            x: public_x.into(),
            kid: kid.into(),
            alg: "EdDSA".into(),
            use_: "sig".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct JwkSet {
    pub keys: Vec<Jwk>,
}

/// A freshly generated key pair: PKCS#8 private key bytes and the base64url public key (`x`).
pub struct GeneratedKey {
    pub pkcs8: Vec<u8>,
    pub public_x: String,
}

pub fn generate_ed25519() -> anyhow::Result<GeneratedKey> {
    let rng = SystemRandom::new();
    let doc = Ed25519KeyPair::generate_pkcs8(&rng).map_err(|_| anyhow::anyhow!("key generation failed"))?;
    let pair = Ed25519KeyPair::from_pkcs8(doc.as_ref()).map_err(|_| anyhow::anyhow!("generated key is invalid"))?;
    Ok(GeneratedKey { pkcs8: doc.as_ref().to_vec(), public_x: URL_SAFE_NO_PAD.encode(pair.public_key().as_ref()) })
}

/// Signs access tokens with one active key.
pub struct TokenSigner {
    kid: String,
    key: EncodingKey,
}

impl TokenSigner {
    pub fn new(kid: impl Into<String>, pkcs8: &[u8]) -> Self {
        Self { kid: kid.into(), key: EncodingKey::from_ed_der(pkcs8) }
    }

    pub fn kid(&self) -> &str {
        &self.kid
    }

    pub fn sign(&self, claims: &Claims) -> anyhow::Result<String> {
        let mut header = Header::new(Algorithm::EdDSA);
        header.kid = Some(self.kid.clone());
        Ok(jsonwebtoken::encode(&header, claims, &self.key)?)
    }
}
