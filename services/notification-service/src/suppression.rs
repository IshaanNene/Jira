//! Addresses not to email: ones whose mailbox does not exist (a hard bounce, from the SMTP
//! reply or the mail provider's report) and ones whose owner marked our email as spam.
//!
//! Kept on the global database, because a mailbox is the same in every workspace, and only
//! as SHA-256 of the trimmed, lower-cased address. Sending to a listed address is skipped
//! (`suppressed`); the person sees "email paused" in their settings and can resume it, which
//! clears the entry (they may have fixed their mailbox or changed their mind).
//!
//! Mail providers report bounces and complaints to `POST /hooks/email-events`, signed with
//! `EMAIL_EVENTS_SECRET` (HMAC-SHA256 of the raw body, hex, in `X-Meridian-Signature:
//! sha256=<hex>`). Without the secret the endpoint does not exist. Only permanent bounces
//! and complaints are recorded; a full mailbox or a greylisting delay is not a reason to stop.

use axum::{body::Bytes, extract::State, http::HeaderMap, Json};
use error_handling::{ApiError, ApiResult};
use ring::{digest, hmac};
use serde::{Deserialize, Serialize};
use sqlx::PgExecutor;

use crate::AppState;

pub const SIGNATURE_HEADER: &str = "x-meridian-signature";
/// Most events in one report, and the largest report accepted.
pub const MAX_EVENTS: usize = 100;
pub const MAX_BODY: usize = 256 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    Bounce,
    Complaint,
}

impl Reason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Bounce => "bounce",
            Self::Complaint => "complaint",
        }
    }
}

pub fn address_hash(address: &str) -> Vec<u8> {
    digest::digest(&digest::SHA256, address.trim().to_lowercase().as_bytes()).as_ref().to_vec()
}

/// Why the address is not emailed, if it is not: `bounce` or `complaint`.
pub async fn reason_of<'e>(db: impl PgExecutor<'e>, address: &str) -> sqlx::Result<Option<String>> {
    sqlx::query_scalar("SELECT reason FROM email_suppressions WHERE address_hash = $1")
        .bind(address_hash(address))
        .fetch_optional(db)
        .await
}

/// Stop emailing the address. A complaint outranks a bounce (it is never downgraded).
pub async fn record<'e>(db: impl PgExecutor<'e>, address: &str, reason: Reason) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO email_suppressions (address_hash, reason) VALUES ($1, $2) \
         ON CONFLICT (address_hash) DO UPDATE SET reason = CASE WHEN email_suppressions.reason = 'complaint' \
         THEN 'complaint' ELSE excluded.reason END",
    )
    .bind(address_hash(address))
    .bind(reason.as_str())
    .execute(db)
    .await?;
    metrics::counter!("notification_email_suppressions_total", "reason" => reason.as_str()).increment(1);
    Ok(())
}

pub async fn clear<'e>(db: impl PgExecutor<'e>, address: &str) -> sqlx::Result<bool> {
    Ok(sqlx::query("DELETE FROM email_suppressions WHERE address_hash = $1")
        .bind(address_hash(address))
        .execute(db)
        .await?
        .rows_affected()
        > 0)
}

/// Key for checking reports' signatures, from `EMAIL_EVENTS_SECRET`.
pub fn events_key(secret: &str) -> hmac::Key {
    hmac::Key::new(hmac::HMAC_SHA256, secret.as_bytes())
}

/// Whether `header` (`sha256=<hex>`) is the signature of `body`. Constant time.
pub fn signature_ok(key: &hmac::Key, body: &[u8], header: Option<&str>) -> bool {
    let Some(tag) = header.and_then(|h| h.trim().strip_prefix("sha256=")).and_then(|h| hex::decode(h).ok()) else {
        return false;
    };
    hmac::verify(key, body, &tag).is_ok()
}

#[derive(Debug, Deserialize)]
pub struct Report {
    pub events: Vec<ReportedEvent>,
}

#[derive(Debug, Deserialize)]
pub struct ReportedEvent {
    /// bounce | complaint (anything else is ignored).
    #[serde(rename = "type")]
    pub kind: String,
    pub email: String,
    /// For bounces: whether the mailbox will never accept mail. Soft bounces are ignored.
    #[serde(default)]
    pub permanent: bool,
}

#[derive(Debug, Serialize)]
pub struct Recorded {
    pub recorded: usize,
}

/// What an event asks for: nothing, or to stop emailing the address for this reason.
pub fn reason_for(event: &ReportedEvent) -> Option<Reason> {
    let looks_like_address = (3..=320).contains(&event.email.len())
        && event.email.contains('@')
        && !event.email.chars().any(|c| c.is_control() || c.is_whitespace());
    if !looks_like_address {
        return None;
    }
    match event.kind.as_str() {
        "bounce" if event.permanent => Some(Reason::Bounce),
        "complaint" => Some(Reason::Complaint),
        _ => None,
    }
}

/// `POST /hooks/email-events`: bounces and complaints reported by the mail provider.
pub async fn email_events(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> ApiResult<Json<Recorded>> {
    let Some(key) = state.email_events.as_deref() else { return Err(ApiError::NotFound("route")) };
    if body.len() > MAX_BODY {
        return Err(ApiError::invalid("body", "too_large", "The report is too large."));
    }
    let signature = headers.get(SIGNATURE_HEADER).and_then(|v| v.to_str().ok());
    if !signature_ok(key, &body, signature) {
        return Err(ApiError::Unauthenticated("invalid signature"));
    }
    let report: Report = serde_json::from_slice(&body)
        .map_err(|_| ApiError::invalid("body", "invalid_report", "Not a report of email events."))?;
    if report.events.len() > MAX_EVENTS {
        return Err(ApiError::invalid("events", "too_many", "At most 100 events per report."));
    }
    let pool = state.db.global()?;
    let mut recorded = 0;
    for event in &report.events {
        if let Some(reason) = reason_for(event) {
            record(pool, &event.email, reason).await?;
            recorded += 1;
        }
    }
    Ok(Json(Recorded { recorded }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sign(key: &hmac::Key, body: &[u8]) -> String {
        format!("sha256={}", hex::encode(hmac::sign(key, body).as_ref()))
    }

    #[test]
    fn signatures_are_checked() {
        let key = events_key("s3cret");
        let body = br#"{"events":[]}"#;
        assert!(signature_ok(&key, body, Some(&sign(&key, body))));
        assert!(!signature_ok(&key, b"{}", Some(&sign(&key, body))));
        assert!(!signature_ok(&events_key("other"), body, Some(&sign(&key, body))));
        assert!(!signature_ok(&key, body, None));
        assert!(!signature_ok(&key, body, Some("sha256=zz")));
        assert!(!signature_ok(&key, body, Some(&sign(&key, body).replace("sha256=", "sha1="))));
    }

    #[test]
    fn only_hard_bounces_and_complaints_count() {
        let event = |kind: &str, email: &str, permanent| ReportedEvent { kind: kind.into(), email: email.into(), permanent };
        assert_eq!(reason_for(&event("bounce", "a@example.com", true)), Some(Reason::Bounce));
        assert_eq!(reason_for(&event("bounce", "a@example.com", false)), None);
        assert_eq!(reason_for(&event("complaint", "a@example.com", false)), Some(Reason::Complaint));
        assert_eq!(reason_for(&event("delivery", "a@example.com", true)), None);
        assert_eq!(reason_for(&event("bounce", "not an address", true)), None);
        assert_eq!(reason_for(&event("bounce", "", true)), None);
    }

    #[test]
    fn addresses_are_compared_case_insensitively() {
        assert_eq!(address_hash(" Maya@Example.COM "), address_hash("maya@example.com"));
        assert_ne!(address_hash("maya@example.com"), address_hash("maia@example.com"));
    }
}
