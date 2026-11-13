//! Signed unsubscribe links, usable without signing in (RFC 8058 one-click and a
//! confirmation page). A token names one person in one workspace and one kind of email;
//! it can only turn email off, never on, and expires after 90 days.

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use ring::hmac;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const TTL_SECONDS: i64 = 90 * 24 * 3600;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Claims {
    #[serde(rename = "t")]
    pub tenant_id: Uuid,
    #[serde(rename = "u")]
    pub user_id: Uuid,
    /// Notification kind the email was about.
    #[serde(rename = "k")]
    pub kind: String,
    /// Issued at, Unix seconds.
    #[serde(rename = "i")]
    pub issued_at: i64,
}

pub struct UnsubscribeKey(hmac::Key);

impl std::fmt::Debug for UnsubscribeKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("UnsubscribeKey(..)")
    }
}

impl UnsubscribeKey {
    /// Derived from the service secret with a purpose label, so a token for this can never
    /// be confused with anything else signed by the same secret.
    pub fn derive(secret: &str) -> Self {
        let root = hmac::Key::new(hmac::HMAC_SHA256, secret.as_bytes());
        let derived = hmac::sign(&root, b"meridian/notification-unsubscribe/v1");
        Self(hmac::Key::new(hmac::HMAC_SHA256, derived.as_ref()))
    }

    pub fn issue(&self, claims: &Claims) -> String {
        let body = URL_SAFE_NO_PAD.encode(serde_json::to_vec(claims).expect("claims serialise"));
        let tag = hmac::sign(&self.0, body.as_bytes());
        format!("{body}.{}", URL_SAFE_NO_PAD.encode(tag.as_ref()))
    }

    /// The claims, if the token is genuine and not expired at `now` (Unix seconds).
    pub fn verify(&self, token: &str, now: i64) -> Option<Claims> {
        if token.len() > 512 {
            return None;
        }
        let (body, tag) = token.split_once('.')?;
        let tag = URL_SAFE_NO_PAD.decode(tag).ok()?;
        hmac::verify(&self.0, body.as_bytes(), &tag).ok()?;
        let claims: Claims = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(body).ok()?).ok()?;
        let age = now - claims.issued_at;
        (0..=TTL_SECONDS).contains(&age).then_some(claims)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claims(issued_at: i64) -> Claims {
        Claims { tenant_id: Uuid::now_v7(), user_id: Uuid::now_v7(), kind: "commented".into(), issued_at }
    }

    #[test]
    fn genuine_tokens_verify_until_they_expire() {
        let key = UnsubscribeKey::derive("a-long-enough-development-secret");
        let c = claims(1_000_000);
        let token = key.issue(&c);
        assert_eq!(key.verify(&token, 1_000_000 + 60), Some(c.clone()));
        assert_eq!(key.verify(&token, 1_000_000 + TTL_SECONDS + 1), None, "expired");
        assert_eq!(key.verify(&token, 1_000_000 - 3600), None, "from the future");
    }

    #[test]
    fn forged_or_altered_tokens_are_refused() {
        let key = UnsubscribeKey::derive("a-long-enough-development-secret");
        let other = UnsubscribeKey::derive("a-different-development-secret!!");
        let c = claims(1_000_000);
        let token = key.issue(&c);
        assert_eq!(other.verify(&token, 1_000_060), None, "signed with another key");
        // Swap in someone else's claims under the original signature.
        let (_, tag) = token.split_once('.').unwrap();
        let forged_body = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims(1_000_000)).unwrap());
        assert_eq!(key.verify(&format!("{forged_body}.{tag}"), 1_000_060), None);
        for junk in ["", ".", "abc", "a.b.c", &"x".repeat(600)] {
            assert_eq!(key.verify(junk, 1_000_060), None, "{junk}");
        }
    }
}
