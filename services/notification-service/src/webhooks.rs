//! Outgoing webhooks: a workspace's administrators subscribe an HTTPS endpoint to the
//! workspace's events, and each matching event is POSTed to it, signed. A webhook of kind
//! `slack` instead posts a short message to a Slack channel through the channel's incoming
//! webhook; that URL is the credential, so it is stored sealed and shown only redacted.
//!
//! Safety: endpoints must be HTTPS on public addresses. The host is resolved at delivery
//! time, every address it resolves to must be public, and the request is pinned to the
//! checked address (so a DNS answer cannot change between the check and the connection);
//! redirects are not followed. Signing secrets are stored encrypted and shown once.

use std::{
    net::{IpAddr, SocketAddr},
    sync::Arc,
    time::Duration,
};

use async_trait::async_trait;
use event_bus::{inbox, Event, Handler, HandlerError};
use rand::RngCore;
use reqwest::Url;
use ring::{aead, hmac};
use serde_json::{json, Value};
use sqlx::{FromRow, PgConnection};
use tenant_context::DbRouter;
use time::OffsetDateTime;
use uuid::Uuid;

pub const CONSUMER: &str = "notification-service.webhooks";
pub const MAX_WEBHOOKS: i64 = 20;
pub const MAX_PROJECTS: usize = 50;
/// The test delivery's type; not an event anyone can subscribe to.
pub const PING: &str = "webhook.ping";
/// Older events are not delivered (e.g. history replayed when the consumer first starts).
pub const STALE_AFTER: Duration = Duration::from_secs(24 * 3600);

/// Events a webhook can subscribe to, with what they mean.
pub const EVENTS: [(&str, &str); 7] = [
    ("issue.created", "An issue was created"),
    ("issue.updated", "An issue's fields changed"),
    ("issue.transitioned", "An issue moved to another status"),
    ("issue.assigned", "An issue's assignee changed"),
    ("issue.deleted", "An issue was deleted"),
    ("comment.created", "A comment was added"),
    ("comment.deleted", "A comment was deleted"),
];

pub fn is_supported(event: &str) -> bool {
    EVENTS.iter().any(|(e, _)| *e == event)
}

/// Where webhooks may be delivered. Production: HTTPS to public addresses only. The two
/// relaxations exist for local development and tests and are off unless configured.
#[derive(Debug, Clone, Copy, Default)]
pub struct Reach {
    pub allow_http: bool,
    pub allow_private: bool,
}

impl Reach {
    pub fn from_env() -> Self {
        let flag = |name| platform_common::config::optional(name).is_some_and(|v| v == "true");
        Self { allow_http: flag("WEBHOOKS_ALLOW_HTTP"), allow_private: flag("WEBHOOKS_ALLOW_PRIVATE_NETWORKS") }
    }
}

/// Everything webhooks need from configuration.
pub struct Settings {
    pub secrets: SecretBox,
    pub reach: Reach,
}

/// What a webhook delivers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// Signed JSON to any HTTPS endpoint.
    Webhook,
    /// A message to a Slack channel's incoming webhook.
    Slack,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Webhook => "webhook",
            Kind::Slack => "slack",
        }
    }
}

// --- addresses ---------------------------------------------------------------------

pub use platform_common::net::is_public;

/// Check an endpoint URL when it is saved.
pub fn validate_url(raw: &str, reach: Reach) -> Result<Url, &'static str> {
    let url = Url::parse(raw.trim()).map_err(|_| "Enter a full URL, starting with https://.")?;
    match url.scheme() {
        "https" => {}
        "http" if reach.allow_http => {}
        _ => return Err("Webhook URLs must use https://."),
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("Put credentials in the signing secret, not in the URL.");
    }
    if url.fragment().is_some() {
        return Err("Webhook URLs cannot have a #fragment.");
    }
    let host = url.host_str().ok_or("Webhook URLs need a host.")?;
    let internal_name = host.eq_ignore_ascii_case("localhost")
        || host.ends_with(".localhost")
        || host.ends_with(".internal")
        || host.ends_with(".local");
    if internal_name && !reach.allow_private {
        return Err("Webhooks can only be delivered to public addresses.");
    }
    if let Some(ip) = literal_ip(&url) {
        if !reach.allow_private && !is_public(ip) {
            return Err("Webhooks can only be delivered to public addresses.");
        }
    }
    Ok(url)
}

/// Check a Slack incoming-webhook URL: in production, `https://hooks.slack.com/services/…`
/// and nothing else (development and tests may point it at a local receiver).
pub fn validate_slack_url(raw: &str, reach: Reach) -> Result<Url, &'static str> {
    const WRONG: &str = "Paste the channel's incoming webhook URL, https://hooks.slack.com/services/…";
    let url = validate_url(raw, reach).map_err(|_| WRONG)?;
    if reach.allow_private {
        return Ok(url);
    }
    let path_ok = url.path().starts_with("/services/") && url.path().len() > "/services/".len();
    if url.scheme() != "https"
        || url.host_str() != Some("hooks.slack.com")
        || url.port().is_some()
        || !path_ok
        || url.query().is_some()
    {
        return Err(WRONG);
    }
    Ok(url)
}

/// What is shown for a Slack webhook's URL: the host and the last four characters.
pub fn redact(url: &Url) -> String {
    let path = url.path();
    let tail: String = path.chars().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect();
    format!("{}://{}/…{tail}", url.scheme(), url.host_str().unwrap_or_default())
}

fn literal_ip(url: &Url) -> Option<IpAddr> {
    url.host_str()?.trim_start_matches('[').trim_end_matches(']').parse().ok()
}

/// Resolve the endpoint's host and choose the address to connect to; refuses the whole
/// endpoint if any address is not public, so a name cannot mix a public and a private answer.
pub async fn resolve(url: &Url, reach: Reach) -> Result<SocketAddr, String> {
    let host = url.host_str().ok_or("no host")?;
    let port = url.port_or_known_default().ok_or("no port")?;
    let addrs: Vec<SocketAddr> = match literal_ip(url) {
        Some(ip) => vec![SocketAddr::new(ip, port)],
        None => {
            tokio::net::lookup_host((host, port)).await.map_err(|_| "The host name could not be resolved.".to_owned())?.collect()
        }
    };
    if addrs.is_empty() {
        return Err("The host name could not be resolved.".into());
    }
    if !reach.allow_private && addrs.iter().any(|a| !is_public(a.ip())) {
        return Err("The host resolves to a private or reserved address; nothing was sent.".into());
    }
    Ok(addrs[0])
}

// --- secrets and signatures --------------------------------------------------------

/// Encrypts signing secrets at rest with a key derived from the service secret.
pub struct SecretBox(aead::LessSafeKey);

impl std::fmt::Debug for SecretBox {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SecretBox(..)")
    }
}

impl SecretBox {
    pub fn derive(secret: &str) -> Self {
        let root = hmac::Key::new(hmac::HMAC_SHA256, secret.as_bytes());
        let key = hmac::sign(&root, b"meridian/webhook-secrets/v1");
        Self(aead::LessSafeKey::new(aead::UnboundKey::new(&aead::AES_256_GCM, key.as_ref()).expect("32-byte key")))
    }

    pub fn seal(&self, plaintext: &str) -> Vec<u8> {
        let mut nonce = [0u8; 12];
        rand::thread_rng().fill_bytes(&mut nonce);
        let mut data = plaintext.as_bytes().to_vec();
        self.0.seal_in_place_append_tag(aead::Nonce::assume_unique_for_key(nonce), aead::Aad::empty(), &mut data).expect("seal");
        [nonce.as_slice(), &data].concat()
    }

    pub fn open(&self, sealed: &[u8]) -> Option<String> {
        if sealed.len() < 12 + 16 {
            return None;
        }
        let (nonce, rest) = sealed.split_at(12);
        let mut data = rest.to_vec();
        let nonce = aead::Nonce::try_assume_unique_for_key(nonce).ok()?;
        let plain = self.0.open_in_place(nonce, aead::Aad::empty(), &mut data).ok()?;
        String::from_utf8(plain.to_vec()).ok()
    }
}

/// A new signing secret: `whsec_` and 32 random bytes in hex.
pub fn new_secret() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    format!("whsec_{}", hex::encode(bytes))
}

/// The `X-Meridian-Signature` header: `t=<unix seconds>,v1=<hex HMAC-SHA256 of "<t>.<body>">`.
/// Receivers recompute it with their secret and reject old timestamps to stop replays.
pub fn signature(secret: &str, timestamp: i64, body: &[u8]) -> String {
    let key = hmac::Key::new(hmac::HMAC_SHA256, secret.as_bytes());
    let mut ctx = hmac::Context::with_key(&key);
    ctx.update(timestamp.to_string().as_bytes());
    ctx.update(b".");
    ctx.update(body);
    format!("t={timestamp},v1={}", hex::encode(ctx.sign().as_ref()))
}

/// What a receiver gets: the event's id, type, time and payload. No tenant id or actor email.
pub fn body(event_id: Uuid, event_type: &str, occurred_at: OffsetDateTime, data: &Value) -> Value {
    json!({
        "id": event_id,
        "type": event_type,
        "occurred_at": occurred_at.format(&time::format_description::well_known::Rfc3339).unwrap_or_default(),
        "data": data,
    })
}

// --- Slack messages --------------------------------------------------------------------

/// Escape text for Slack's mrkdwn: `&`, `<` and `>` are control characters there, so a
/// summary can never become a link, a mention or `@channel`.
pub fn slack_escape(text: &str) -> String {
    let flat: String = text.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    flat.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn readable(status: &str) -> String {
    let words = status.replace('_', " ");
    let mut chars = words.chars();
    chars.next().map(|c| c.to_uppercase().collect::<String>() + chars.as_str()).unwrap_or_default()
}

/// The message posted to Slack for an event: one line, the issue key linked when the app's
/// address is configured. Names of people are left out: events carry only their ids.
pub fn slack_message(event_type: &str, data: &Value, app_url: Option<&str>) -> Value {
    let field = |name: &str| data.get(name).and_then(Value::as_str).unwrap_or_default();
    let key = field("key");
    // Keys are `PROJECT-123`; anything else is not linked.
    let well_formed = key.split_once('-').is_some_and(|(p, n)| {
        !p.is_empty() && p.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()) && n.parse::<u64>().is_ok()
    });
    let issue = match (app_url, well_formed) {
        (Some(base), true) => format!("<{base}/issues/{key}|{key}>"),
        _ => format!("*{}*", slack_escape(key)),
    };
    let text = match event_type {
        "issue.created" => format!("{issue} was created: {}", slack_escape(field("summary"))),
        "issue.updated" => {
            let fields: Vec<String> = data
                .get("changed_fields")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_str).map(|f| slack_escape(&f.replace('_', " "))).collect())
                .unwrap_or_default();
            if fields.is_empty() {
                format!("{issue} was updated")
            } else {
                format!("{issue} was updated: {}", fields.join(", "))
            }
        }
        "issue.transitioned" => format!(
            "{issue} moved from {} to {}",
            slack_escape(&readable(field("from_status"))),
            slack_escape(&readable(field("to_status")))
        ),
        "issue.assigned" if data.get("assignee_id").is_some_and(Value::is_null) => format!("{issue} was unassigned"),
        "issue.assigned" => format!("{issue} was assigned"),
        "issue.deleted" => format!("{} was deleted", slack_escape(key)),
        "comment.created" => format!("New comment on {issue}"),
        "comment.deleted" => format!("A comment on {issue} was deleted"),
        PING => "Meridian is connected to this channel.".to_owned(),
        other => format!("{} on {issue}", slack_escape(other)),
    };
    json!({ "text": text })
}

// --- storage -------------------------------------------------------------------------

#[derive(Debug, Clone, FromRow)]
pub struct Webhook {
    pub id: Uuid,
    pub kind: String,
    pub name: String,
    pub url: String,
    pub events: Vec<String>,
    pub project_ids: Vec<Uuid>,
    pub secret_sealed: Vec<u8>,
    pub enabled: bool,
    pub disabled_reason: Option<String>,
    pub consecutive_failures: i32,
    pub created_by: Uuid,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

pub const WEBHOOK: &str =
    "id, kind, name, url, events, project_ids, secret_sealed, enabled, disabled_reason, consecutive_failures, \
                           created_by, created_at, updated_at";

pub async fn list(conn: &mut PgConnection, tenant_id: Uuid) -> sqlx::Result<Vec<Webhook>> {
    sqlx::query_as(&format!("SELECT {WEBHOOK} FROM webhooks WHERE tenant_id = $1 ORDER BY created_at, id"))
        .bind(tenant_id)
        .fetch_all(conn)
        .await
}

pub async fn get(conn: &mut PgConnection, tenant_id: Uuid, id: Uuid) -> sqlx::Result<Option<Webhook>> {
    sqlx::query_as(&format!("SELECT {WEBHOOK} FROM webhooks WHERE tenant_id = $1 AND id = $2"))
        .bind(tenant_id)
        .bind(id)
        .fetch_optional(conn)
        .await
}

/// Queue one delivery; a redelivered event is queued once per webhook.
pub async fn enqueue(
    conn: &mut PgConnection,
    tenant_id: Uuid,
    webhook_id: Uuid,
    event_id: Uuid,
    event_type: &str,
    body: &Value,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO webhook_deliveries (id, tenant_id, webhook_id, event_id, event_type, body) \
         VALUES ($1, $2, $3, $4, $5, $6) ON CONFLICT (webhook_id, event_id) DO NOTHING",
    )
    .bind(platform_common::new_id())
    .bind(tenant_id)
    .bind(webhook_id)
    .bind(event_id)
    .bind(event_type)
    .bind(body)
    .execute(conn)
    .await?;
    Ok(())
}

// --- the consumer --------------------------------------------------------------------

/// Queues a delivery of each supported event to every enabled webhook of its workspace that
/// subscribes to it (and to its project, when limited), created before the event happened.
pub struct Dispatcher {
    pub db: Arc<DbRouter>,
    /// The web app's address, for links in Slack messages; none links nothing.
    pub app_url: Option<String>,
}

/// What a webhook of `kind` is sent for an event.
pub fn body_for(
    kind: &str,
    event_id: Uuid,
    event_type: &str,
    occurred_at: OffsetDateTime,
    data: &Value,
    app_url: Option<&str>,
) -> Value {
    if kind == Kind::Slack.as_str() {
        slack_message(event_type, data, app_url)
    } else {
        body(event_id, event_type, occurred_at, data)
    }
}

pub fn project_of(payload: &Value) -> Option<Uuid> {
    payload.get("project_id").and_then(Value::as_str).and_then(|s| s.parse().ok())
}

pub fn wants(hook: &Webhook, event_type: &str, project: Option<Uuid>, occurred_at: OffsetDateTime) -> bool {
    hook.enabled
        && hook.created_at <= occurred_at
        && hook.events.iter().any(|e| e == event_type)
        && (hook.project_ids.is_empty() || project.is_some_and(|p| hook.project_ids.contains(&p)))
}

#[async_trait]
impl Handler for Dispatcher {
    async fn handle(&self, event: &Event) -> Result<(), HandlerError> {
        let event_type = event.event_type();
        if !is_supported(event_type) {
            return Ok(());
        }
        let envelope = &event.envelope;
        let age = OffsetDateTime::now_utc() - envelope.occurred_at;
        if age > time::Duration::try_from(STALE_AFTER).unwrap_or(time::Duration::DAY) {
            return Ok(());
        }
        let tenant_id = event.tenant_id()?;
        let mut tx = self.db.begin(tenant_id).await?;
        if !inbox::claim(tx.conn(), CONSUMER, event.event_id()).await? {
            tx.rollback().await?;
            return Ok(());
        }
        let project = project_of(&envelope.payload);
        let hooks = list(tx.conn(), tenant_id).await?;
        for hook in hooks.iter().filter(|h| wants(h, event_type, project, envelope.occurred_at)) {
            let body = body_for(
                &hook.kind,
                event.event_id(),
                event_type,
                envelope.occurred_at,
                &envelope.payload,
                self.app_url.as_deref(),
            );
            enqueue(tx.conn(), tenant_id, hook.id, event.event_id(), event_type, &body).await?;
        }
        tx.commit().await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_public_addresses_are_reachable() {
        for private in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            "::1",
            "fd00::1",
            "fe80::1",
            "::ffff:127.0.0.1",
            "::ffff:10.0.0.1",
            "240.0.0.1",
            "198.18.0.1",
        ] {
            assert!(!is_public(private.parse().unwrap()), "{private}");
        }
        for public in ["8.8.8.8", "1.1.1.1", "2606:4700:4700::1111"] {
            assert!(is_public(public.parse().unwrap()), "{public}");
        }
    }

    #[test]
    fn urls_are_https_to_public_hosts_without_credentials() {
        let strict = Reach::default();
        assert!(validate_url("https://hooks.example.com/meridian?x=1", strict).is_ok());
        for bad in [
            "http://hooks.example.com/",
            "ftp://example.com/",
            "https://user:pass@example.com/",
            "https://127.0.0.1/hook",
            "https://[::1]/hook",
            "https://169.254.169.254/latest/meta-data",
            "https://localhost/",
            "https://metadata.google.internal/",
            "https://example.com/#x",
            "not a url",
        ] {
            assert!(validate_url(bad, strict).is_err(), "{bad}");
        }
        let dev = Reach { allow_http: true, allow_private: true };
        assert!(validate_url("http://127.0.0.1:9000/hook", dev).is_ok());
    }

    #[test]
    fn secrets_round_trip_sealed_and_tamper_evident() {
        let sealer = SecretBox::derive("service-secret");
        let secret = new_secret();
        assert!(secret.starts_with("whsec_") && secret.len() == 70);
        let sealed = sealer.seal(&secret);
        assert!(!sealed.windows(6).any(|w| w == b"whsec_"), "stored encrypted");
        assert_eq!(sealer.open(&sealed).as_deref(), Some(secret.as_str()));
        let mut tampered = sealed.clone();
        *tampered.last_mut().unwrap() ^= 1;
        assert_eq!(sealer.open(&tampered), None);
        assert_eq!(SecretBox::derive("another-secret").open(&sealed), None);
    }

    #[test]
    fn signatures_cover_the_timestamp_and_body() {
        let s = signature("whsec_abc", 1_700_000_000, b"{\"a\":1}");
        assert!(s.starts_with("t=1700000000,v1="));
        assert_ne!(s, signature("whsec_abc", 1_700_000_001, b"{\"a\":1}"));
        assert_ne!(s, signature("whsec_abc", 1_700_000_000, b"{\"a\":2}"));
        assert_ne!(s, signature("whsec_abd", 1_700_000_000, b"{\"a\":1}"));
    }

    #[test]
    fn slack_urls_are_slack_only_and_shown_redacted() {
        let strict = Reach::default();
        let ok = validate_slack_url("https://hooks.slack.com/services/T000/B000/XXXXabcd", strict).unwrap();
        assert_eq!(redact(&ok), "https://hooks.slack.com/…abcd");
        for bad in [
            "https://hooks.slack.com.evil.example/services/T/B/X",
            "https://example.com/services/T/B/X",
            "http://hooks.slack.com/services/T/B/X",
            "https://hooks.slack.com/api/chat.postMessage",
            "https://hooks.slack.com/services/",
            "https://hooks.slack.com:8443/services/T/B/X",
            "https://hooks.slack.com/services/T/B/X?redirect=1",
        ] {
            assert!(validate_slack_url(bad, strict).is_err(), "{bad}");
        }
    }

    #[test]
    fn slack_messages_escape_everything_they_did_not_write() {
        let created = slack_message(
            "issue.created",
            &json!({ "key": "PAY-12", "summary": "<!channel> & <https://evil|click>" }),
            Some("https://meridian.example"),
        );
        assert_eq!(
            created["text"],
            "<https://meridian.example/issues/PAY-12|PAY-12> was created: &lt;!channel&gt; &amp; &lt;https://evil|click&gt;"
        );
        let moved = slack_message(
            "issue.transitioned",
            &json!({ "key": "PAY-1", "from_status": "in_progress", "to_status": "done" }),
            None,
        );
        assert_eq!(moved["text"], "*PAY-1* moved from In progress to Done");
        let odd = slack_message("comment.created", &json!({ "key": "<x|y>" }), Some("https://m.example"));
        assert_eq!(odd["text"], "New comment on *&lt;x|y&gt;*", "only well-formed keys are linked");
        let updated =
            slack_message("issue.updated", &json!({ "key": "PAY-2", "changed_fields": ["story_points", "labels"] }), None);
        assert_eq!(updated["text"], "*PAY-2* was updated: story points, labels");
    }
}
