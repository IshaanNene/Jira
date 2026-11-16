//! Sending one email over SMTP, for every service that sends mail.

use async_trait::async_trait;
use lettre::{
    message::{
        header::{ContentType, HeaderName, HeaderValue},
        Mailbox, MultiPart, SinglePart,
    },
    AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
};

#[derive(Debug, Clone)]
pub struct Outgoing<'a> {
    pub to: &'a str,
    pub subject: &'a str,
    pub text: &'a str,
    pub html: &'a str,
    /// One-click unsubscribe endpoint (RFC 8058).
    pub unsubscribe_url: Option<&'a str>,
}

#[derive(Debug, thiserror::Error)]
pub enum SendError {
    /// Worth trying again later (connection refused, 4xx reply, timeout).
    #[error("temporary: {0}")]
    Transient(String),
    /// Will never work (5xx reply about the message or the server).
    #[error("permanent: {0}")]
    Permanent(String),
    /// The recipient address itself will never work: it is not an address, or the receiving
    /// server says the mailbox does not exist (550, 551, 553). Worth not emailing it again.
    #[error("recipient rejected: {0}")]
    Rejected(String),
}

/// Whether a permanent SMTP reply code is about the recipient's mailbox rather than the
/// message: 550 (no such mailbox), 551 (user not local), 553 (mailbox name not allowed).
pub fn rejects_recipient(code: &str) -> bool {
    matches!(code, "550" | "551" | "553")
}

#[async_trait]
pub trait Mailer: Send + Sync {
    async fn send(&self, email: &Outgoing<'_>) -> Result<(), SendError>;
}

pub struct SmtpMailer {
    transport: AsyncSmtpTransport<Tokio1Executor>,
    from: Mailbox,
}

impl SmtpMailer {
    /// `url`: `smtp://host:port` (plain, local only), `smtp://host?tls=required`, or
    /// `smtps://user:password@host:465`.
    pub fn new(url: &str, from: &str) -> anyhow::Result<Self> {
        let transport = AsyncSmtpTransport::<Tokio1Executor>::from_url(url)
            .map_err(|_| anyhow::anyhow!("SMTP_URL is not a valid SMTP URL"))?
            .timeout(Some(std::time::Duration::from_secs(20)))
            .build();
        Ok(Self { transport, from: from.parse().map_err(|e| anyhow::anyhow!("EMAIL_FROM is not a valid address: {e}"))? })
    }
}

#[async_trait]
impl Mailer for SmtpMailer {
    async fn send(&self, email: &Outgoing<'_>) -> Result<(), SendError> {
        let to: Mailbox = email.to.parse().map_err(|_| SendError::Rejected("invalid recipient address".into()))?;
        let mut message = Message::builder()
            .from(self.from.clone())
            .to(to)
            .subject(email.subject)
            .multipart(
                MultiPart::alternative()
                    .singlepart(SinglePart::builder().header(ContentType::TEXT_PLAIN).body(email.text.to_owned()))
                    .singlepart(SinglePart::builder().header(ContentType::TEXT_HTML).body(email.html.to_owned())),
            )
            .map_err(|e| SendError::Permanent(e.to_string()))?;
        if let Some(url) = email.unsubscribe_url {
            let headers = message.headers_mut();
            headers.insert_raw(HeaderValue::new(HeaderName::new_from_ascii_str("List-Unsubscribe"), format!("<{url}>")));
            headers.insert_raw(HeaderValue::new(
                HeaderName::new_from_ascii_str("List-Unsubscribe-Post"),
                "List-Unsubscribe=One-Click".to_owned(),
            ));
        }
        match self.transport.send(message).await {
            Ok(_) => Ok(()),
            Err(e) if e.is_permanent() && e.status().is_some_and(|c| rejects_recipient(&c.to_string())) => {
                Err(SendError::Rejected(e.to_string()))
            }
            Err(e) if e.is_permanent() => Err(SendError::Permanent(e.to_string())),
            Err(e) => Err(SendError::Transient(e.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_mailbox_replies_reject_the_recipient() {
        for code in ["550", "551", "553"] {
            assert!(rejects_recipient(code), "{code}");
        }
        for code in ["552", "554", "500", "421", "450"] {
            assert!(!rejects_recipient(code), "{code}");
        }
    }
}
