//! Links sent by email to prove a person controls their address: verifying it, and resetting a
//! forgotten password. A link carries a 256-bit random token; only its SHA-256 is stored, it
//! works once, expires, and stops working if the account's address changes. Emails are sent from
//! a background task so that a request's timing never depends on the mail server, and the token
//! is never written anywhere but the email itself.

use std::sync::Arc;

use error_handling::ApiResult;
use smtp_mailer::{Mailer, Outgoing};
use sqlx::{PgConnection, PgPool};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use crate::{crypto, store::Account};

/// No new link within this long of the last one, per account and purpose.
pub const MIN_INTERVAL: Duration = Duration::seconds(60);
/// At most this many links per account and purpose in an hour.
pub const MAX_PER_HOUR: i64 = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    VerifyEmail,
    ResetPassword,
}

impl Purpose {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::VerifyEmail => "verify_email",
            Self::ResetPassword => "reset_password",
        }
    }

    pub fn lifetime(self) -> Duration {
        match self {
            Self::VerifyEmail => Duration::hours(24),
            Self::ResetPassword => Duration::hours(1),
        }
    }

    /// The web page that takes the token.
    fn page(self) -> &'static str {
        match self {
            Self::VerifyEmail => "verify-email",
            Self::ResetPassword => "reset-password",
        }
    }
}

/// Where emails go out from. Absent when no mail server is configured.
pub struct AccountMail {
    pub mailer: Arc<dyn Mailer>,
    /// Base URL of the web app for links, without a trailing slash.
    pub app_url: String,
}

/// A new link for `account`, or `Err(retry_after)` when one was sent too recently.
pub async fn issue(pool: &PgPool, account: &Account, purpose: Purpose) -> ApiResult<Result<String, Duration>> {
    let mut tx = pool.begin().await?;
    // Serialise issuing per account so two requests cannot both pass the rate check.
    sqlx::query("SELECT 1 FROM accounts WHERE id = $1 FOR UPDATE").bind(account.id).execute(&mut *tx).await?;
    let (recent, last): (i64, Option<OffsetDateTime>) = sqlx::query_as(
        "SELECT count(*), max(created_at) FROM email_tokens \
         WHERE account_id = $1 AND purpose = $2 AND created_at > now() - interval '1 hour'",
    )
    .bind(account.id)
    .bind(purpose.as_str())
    .fetch_one(&mut *tx)
    .await?;
    let now = OffsetDateTime::now_utc();
    if let Some(last) = last {
        let wait = MIN_INTERVAL - (now - last);
        if wait > Duration::ZERO {
            return Ok(Err(wait));
        }
    }
    if recent >= MAX_PER_HOUR {
        return Ok(Err(Duration::hours(1)));
    }
    // Old links are kept a while for the rate count, then dropped.
    sqlx::query("DELETE FROM email_tokens WHERE account_id = $1 AND expires_at < now() - interval '7 days'")
        .bind(account.id)
        .execute(&mut *tx)
        .await?;
    let token = crypto::random_token();
    sqlx::query(
        "INSERT INTO email_tokens (id, account_id, purpose, token_hash, email, expires_at) \
         VALUES ($1, $2, $3, $4, $5, now() + make_interval(secs => $6))",
    )
    .bind(platform_common::new_id())
    .bind(account.id)
    .bind(purpose.as_str())
    .bind(crypto::sha256(token.as_bytes()))
    .bind(&account.email)
    .bind(purpose.lifetime().whole_seconds() as f64)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Ok(token))
}

/// Use up a link: the account it was for, if it is genuine, unused, unexpired and still for the
/// account's current address. Every other open link of that purpose for the account is spent too.
pub async fn consume(conn: &mut PgConnection, token: &str, purpose: Purpose) -> ApiResult<Option<Uuid>> {
    if token.is_empty() || token.len() > 100 {
        return Ok(None);
    }
    let account: Option<(Uuid,)> = sqlx::query_as(
        "UPDATE email_tokens t SET used_at = now() FROM accounts a \
         WHERE t.token_hash = $1 AND t.purpose = $2 AND t.used_at IS NULL AND t.expires_at > now() \
           AND a.id = t.account_id AND a.email = t.email \
         RETURNING t.account_id",
    )
    .bind(crypto::sha256(token.as_bytes()))
    .bind(purpose.as_str())
    .fetch_optional(&mut *conn)
    .await?;
    if let Some((id,)) = account {
        sqlx::query("UPDATE email_tokens SET used_at = now() WHERE account_id = $1 AND purpose = $2 AND used_at IS NULL")
            .bind(id)
            .bind(purpose.as_str())
            .execute(&mut *conn)
            .await?;
    }
    Ok(account.map(|(id,)| id))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rendered {
    pub subject: String,
    pub text: String,
    pub html: String,
}

fn escape_html(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

pub fn render(purpose: Purpose, display_name: &str, app_url: &str, token: &str) -> Rendered {
    let link = format!("{app_url}/{}?token={token}", purpose.page());
    let name: String = display_name.chars().filter(|c| !c.is_control()).take(80).collect();
    let (subject, intro, button, outro) = match purpose {
        Purpose::VerifyEmail => (
            "Verify your email address",
            "Confirm that this is your email address to finish setting up your Meridian account.",
            "Verify email address",
            "The link works once and expires in 24 hours. If you didn't create an account, ignore this email.",
        ),
        Purpose::ResetPassword => (
            "Reset your password",
            "Someone asked to reset the password of your Meridian account. Choose a new one with this link.",
            "Choose a new password",
            "The link works once and expires in an hour; it signs you out everywhere. If you didn't ask for this, ignore this email: your password stays as it is.",
        ),
    };
    let text = format!("Hi {name},\n\n{intro}\n\n{link}\n\n{outro}\n");
    let html = format!(
        r#"<!doctype html><html><body style="margin:0;padding:24px;background:#f7f8f9;font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',Roboto,sans-serif;color:#172b4d">
<div style="max-width:560px;margin:0 auto;background:#ffffff;border:1px solid #dcdfe4;border-radius:6px;padding:24px">
<p style="margin:0 0 16px;font-size:14px">Hi {name},</p>
<p style="margin:0 0 16px;font-size:14px">{intro}</p>
<p style="margin:24px 0"><a href="{link}" style="background:#0c66e4;color:#ffffff;padding:8px 14px;border-radius:3px;text-decoration:none;font-weight:600">{button}</a></p>
<p style="margin:0;font-size:12px;color:#626f86">{outro}</p>
</div></body></html>"#,
        name = escape_html(&name),
        link = escape_html(&link),
    );
    Rendered { subject: subject.to_owned(), text, html }
}

/// Send the link in the background. Failures are logged without the address or the token.
pub fn send(mail: &Arc<AccountMail>, account: &Account, purpose: Purpose, token: String) {
    let rendered = render(purpose, &account.display_name, &mail.app_url, &token);
    let (mail, to, account_id) = (Arc::clone(mail), account.email.clone(), account.id);
    tokio::spawn(async move {
        let email =
            Outgoing { to: &to, subject: &rendered.subject, text: &rendered.text, html: &rendered.html, unsubscribe_url: None };
        match mail.mailer.send(&email).await {
            Ok(()) => tracing::info!(user_id = %account_id, purpose = purpose.as_str(), "account email sent"),
            // The server's reply can quote the address, so only the kind of failure is logged.
            Err(error) => {
                let kind = if matches!(error, smtp_mailer::SendError::Transient(_)) { "temporary" } else { "permanent" };
                tracing::warn!(user_id = %account_id, purpose = purpose.as_str(), failure = kind, "account email not sent");
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_point_at_the_right_page_and_names_are_escaped() {
        let r = render(Purpose::VerifyEmail, "Ada <b>\r\nBcc: x", "https://app.example.com", "tok_en");
        assert_eq!(r.subject, "Verify your email address");
        assert!(r.text.contains("https://app.example.com/verify-email?token=tok_en"));
        assert!(r.text.starts_with("Hi Ada <b>Bcc: x,"), "{}", r.text);
        assert!(r.html.contains("Ada &lt;b&gt;") && !r.html.contains("<b>"));
        let reset = render(Purpose::ResetPassword, "Ada", "https://app.example.com", "t");
        assert!(reset.text.contains("https://app.example.com/reset-password?token=t") && reset.text.contains("an hour"));
    }
}
