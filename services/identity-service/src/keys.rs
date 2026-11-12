//! Signing-key lifecycle: load the active key, or create the first one.

use auth_middleware::{generate_ed25519, Jwk, JwkSet, TokenSigner};
use sqlx::PgPool;

use crate::crypto::KeySealer;

pub struct KeyMaterial {
    pub signer: TokenSigner,
    /// Active and retiring public keys.
    pub jwks: JwkSet,
}

/// Load the active signing key, creating one if none exists. Safe when several
/// instances start at once: the partial unique index lets exactly one insert win and
/// the others read the winner's key.
pub async fn load_or_create(pool: &PgPool, sealer: &KeySealer) -> anyhow::Result<KeyMaterial> {
    for _ in 0..3 {
        let active: Option<(String, Vec<u8>, Vec<u8>)> =
            sqlx::query_as("SELECT kid, private_key_enc, nonce FROM signing_keys WHERE status = 'active'")
                .fetch_optional(pool)
                .await?;
        if let Some((kid, sealed, nonce)) = active {
            let pkcs8 = sealer.open(&sealed, &nonce)?;
            let public: Vec<(String, String)> = sqlx::query_as(
                "SELECT kid, public_x FROM signing_keys WHERE status IN ('active', 'retiring') ORDER BY created_at",
            )
            .fetch_all(pool)
            .await?;
            let jwks = JwkSet { keys: public.into_iter().map(|(kid, x)| Jwk::ed25519(kid, x)).collect() };
            return Ok(KeyMaterial { signer: TokenSigner::new(kid, &pkcs8), jwks });
        }

        let generated = generate_ed25519()?;
        let (sealed, nonce) = sealer.seal(&generated.pkcs8)?;
        let kid = platform_common::new_id().simple().to_string();
        sqlx::query(
            "INSERT INTO signing_keys (kid, algorithm, public_x, private_key_enc, nonce, status) \
             VALUES ($1, 'EdDSA', $2, $3, $4, 'active') ON CONFLICT DO NOTHING",
        )
        .bind(&kid)
        .bind(&generated.public_x)
        .bind(sealed)
        .bind(nonce)
        .execute(pool)
        .await?;
        tracing::info!(kid, "signing key created (if no other instance won the race)");
    }
    anyhow::bail!("could not load or create a signing key")
}
