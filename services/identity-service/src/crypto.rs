//! Password hashing, opaque-token generation and at-rest sealing of signing keys.
//! Everything here is a thin wrapper over audited libraries (argon2, ring, sha2).

use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Algorithm, Argon2, Params, Version,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use error_handling::{ApiError, ApiResult};
use rand::RngCore;
use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM, NONCE_LEN};
use sha2::{Digest, Sha256};

/// OWASP baseline for Argon2id: 19 MiB, 2 iterations, 1 lane.
fn argon2() -> ApiResult<Argon2<'static>> {
    let params = Params::new(19 * 1024, 2, 1, None).map_err(|e| ApiError::internal(anyhow::anyhow!("argon2 params: {e}")))?;
    Ok(Argon2::new(Algorithm::Argon2id, Version::V0x13, params))
}

/// Hashing is CPU-bound (tens of milliseconds): run it off the async runtime threads.
pub async fn hash_password(password: String) -> ApiResult<String> {
    tokio::task::spawn_blocking(move || {
        let salt = SaltString::generate(&mut OsRng);
        argon2()?
            .hash_password(password.as_bytes(), &salt)
            .map(|h| h.to_string())
            .map_err(|e| ApiError::internal(anyhow::anyhow!("password hashing failed: {e}")))
    })
    .await
    .map_err(ApiError::internal)?
}

pub async fn verify_password(password: String, phc: String) -> ApiResult<bool> {
    tokio::task::spawn_blocking(move || {
        let parsed = PasswordHash::new(&phc).map_err(|e| ApiError::internal(anyhow::anyhow!("stored hash is invalid: {e}")))?;
        Ok(argon2()?.verify_password(password.as_bytes(), &parsed).is_ok())
    })
    .await
    .map_err(ApiError::internal)?
}

/// 256 bits from the OS CSPRNG, base64url. Used for refresh tokens.
pub fn random_token() -> String {
    let mut bytes = [0_u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

pub fn sha256(input: &[u8]) -> Vec<u8> {
    Sha256::digest(input).to_vec()
}

/// AES-256-GCM sealing of private signing keys under the key-encryption key (KEK).
/// In production the KEK comes from a KMS-backed secret; see SECURITY_DESIGN.md §6.
pub struct KeySealer(LessSafeKey);

impl KeySealer {
    pub fn from_base64(kek: &str) -> anyhow::Result<Self> {
        let raw = base64::engine::general_purpose::STANDARD.decode(kek.trim())?;
        if raw.len() != 32 {
            anyhow::bail!("IDENTITY_KEY_ENCRYPTION_KEY must be 32 bytes, base64-encoded");
        }
        let key = UnboundKey::new(&AES_256_GCM, &raw).map_err(|_| anyhow::anyhow!("invalid key-encryption key"))?;
        Ok(Self(LessSafeKey::new(key)))
    }

    /// Returns `(ciphertext || tag, nonce)`. A fresh random nonce per sealed key.
    pub fn seal(&self, plaintext: &[u8]) -> anyhow::Result<(Vec<u8>, Vec<u8>)> {
        let mut nonce = [0_u8; NONCE_LEN];
        rand::rngs::OsRng.fill_bytes(&mut nonce);
        let mut buf = plaintext.to_vec();
        self.0
            .seal_in_place_append_tag(Nonce::assume_unique_for_key(nonce), Aad::empty(), &mut buf)
            .map_err(|_| anyhow::anyhow!("sealing failed"))?;
        Ok((buf, nonce.to_vec()))
    }

    pub fn open(&self, sealed: &[u8], nonce: &[u8]) -> anyhow::Result<Vec<u8>> {
        let nonce = Nonce::try_assume_unique_for_key(nonce).map_err(|_| anyhow::anyhow!("invalid nonce"))?;
        let mut buf = sealed.to_vec();
        let plain = self
            .0
            .open_in_place(nonce, Aad::empty(), &mut buf)
            .map_err(|_| anyhow::anyhow!("signing key could not be decrypted (wrong key-encryption key?)"))?;
        Ok(plain.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn password_round_trip_and_rejection() {
        let hash = hash_password("correct horse battery".into()).await.unwrap();
        assert!(hash.starts_with("$argon2id$v=19$m=19456,t=2,p=1$"));
        assert!(verify_password("correct horse battery".into(), hash.clone()).await.unwrap());
        assert!(!verify_password("wrong".into(), hash).await.unwrap());
    }

    #[tokio::test]
    async fn same_password_hashes_differently() {
        let a = hash_password("same password!".into()).await.unwrap();
        let b = hash_password("same password!".into()).await.unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn sealing_round_trip_and_tamper_detection() {
        let kek = base64::engine::general_purpose::STANDARD.encode([7_u8; 32]);
        let sealer = KeySealer::from_base64(&kek).unwrap();
        let (mut sealed, nonce) = sealer.seal(b"private key bytes").unwrap();
        assert_eq!(sealer.open(&sealed, &nonce).unwrap(), b"private key bytes");
        sealed[0] ^= 1;
        assert!(sealer.open(&sealed, &nonce).is_err());
        let other = KeySealer::from_base64(&base64::engine::general_purpose::STANDARD.encode([8_u8; 32])).unwrap();
        sealed[0] ^= 1;
        assert!(other.open(&sealed, &nonce).is_err());
    }

    #[test]
    fn kek_must_be_32_bytes() {
        assert!(KeySealer::from_base64("c2hvcnQ=").is_err());
    }

    #[test]
    fn tokens_are_unique_and_long() {
        let a = random_token();
        assert_eq!(a.len(), 43);
        assert_ne!(a, random_token());
    }
}
