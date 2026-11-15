//! The invitation email. The link's token is only ever in this email and in the response to the
//! person who sent the invitation; it is never stored or logged.

use std::sync::Arc;

use smtp_mailer::{Mailer, Outgoing, SendError};
use time::OffsetDateTime;
use uuid::Uuid;

pub struct InviteMail {
    pub mailer: Arc<dyn Mailer>,
}

#[derive(Debug, Clone)]
pub struct InviteEmail<'a> {
    pub inviter_name: &'a str,
    pub organization_name: &'a str,
    pub role_label: &'a str,
    pub accept_url: &'a str,
    pub expires_at: OffsetDateTime,
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

/// One line, no control characters, bounded length.
fn one_line(value: &str, max: usize) -> String {
    let flat: String = value.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    let flat = flat.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() > max {
        format!("{}…", flat.chars().take(max - 1).collect::<String>())
    } else {
        flat
    }
}

pub fn role_label(role: &str) -> &'static str {
    match role {
        "admin" => "an admin",
        "guest" => "a guest",
        _ => "a member",
    }
}

pub fn render(input: &InviteEmail<'_>) -> Rendered {
    let who = match one_line(input.inviter_name, 80) {
        name if name.is_empty() => "A workspace admin".to_owned(),
        name => name,
    };
    let org = one_line(input.organization_name, 100);
    let expires = format!("{} {} {}", input.expires_at.day(), input.expires_at.month(), input.expires_at.year());
    let subject = format!("{who} invited you to {org} on Meridian");
    let intro = format!("{who} invited you to join {org} on Meridian as {}.", input.role_label);
    let outro = format!(
        "The link works once, only for this email address, and expires on {expires} (UTC). If you weren't expecting this, ignore this email."
    );
    let text = format!("{intro}\n\nAccept the invitation: {}\n\n{outro}\n", input.accept_url);
    let html = format!(
        r#"<!doctype html><html><body style="margin:0;padding:24px;background:#f7f8f9;font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',Roboto,sans-serif;color:#172b4d">
<div style="max-width:560px;margin:0 auto;background:#ffffff;border:1px solid #dcdfe4;border-radius:6px;padding:24px">
<p style="margin:0 0 16px;font-size:14px">{intro}</p>
<p style="margin:24px 0"><a href="{link}" style="background:#0c66e4;color:#ffffff;padding:8px 14px;border-radius:3px;text-decoration:none;font-weight:600">Accept invitation</a></p>
<p style="margin:0;font-size:12px;color:#626f86">{outro}</p>
</div></body></html>"#,
        intro = escape_html(&intro),
        link = escape_html(input.accept_url),
        outro = escape_html(&outro),
    );
    Rendered { subject, text, html }
}

/// Send in the background. Failures are logged by invitation id and kind only: the server's reply
/// can quote the address.
pub fn send(mail: &Arc<InviteMail>, invitation_id: Uuid, to: String, rendered: Rendered) {
    let mail = Arc::clone(mail);
    tokio::spawn(async move {
        let email =
            Outgoing { to: &to, subject: &rendered.subject, text: &rendered.text, html: &rendered.html, unsubscribe_url: None };
        match mail.mailer.send(&email).await {
            Ok(()) => tracing::info!(%invitation_id, "invitation email sent"),
            Err(error) => {
                let kind = if matches!(error, SendError::Transient(_)) { "temporary" } else { "permanent" };
                tracing::warn!(%invitation_id, failure = kind, "invitation email not sent");
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_cannot_inject_headers_or_markup() {
        let r = render(&InviteEmail {
            inviter_name: "Olivia <b>\r\nBcc: x@example.com",
            organization_name: "Acme & <Co>",
            role_label: role_label("member"),
            accept_url: "https://app.example.com/invite/abc",
            expires_at: time::Date::from_calendar_date(2026, time::Month::November, 22)
                .unwrap()
                .with_hms(10, 0, 0)
                .unwrap()
                .assume_utc(),
        });
        assert_eq!(r.subject, "Olivia <b> Bcc: x@example.com invited you to Acme & <Co> on Meridian");
        assert!(!r.subject.contains('\n'));
        assert!(r.text.contains("as a member.") && r.text.contains("Accept the invitation: https://app.example.com/invite/abc"));
        assert!(r.text.contains("expires on 22 November 2026"));
        assert!(r.html.contains("Acme &amp; &lt;Co&gt;") && !r.html.contains("<b>"));
        let anonymous = render(&InviteEmail {
            inviter_name: "  ",
            organization_name: "A",
            role_label: "a guest",
            accept_url: "u",
            expires_at: OffsetDateTime::UNIX_EPOCH,
        });
        assert!(anonymous.subject.starts_with("A workspace admin invited you"));
    }
}
