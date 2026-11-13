//! Sending one email over SMTP.

use async_trait::async_trait;
use lettre::{
    message::{header::ContentType, Mailbox, MultiPart, SinglePart},
    AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
};

#[derive(Debug, Clone)]
pub struct Outgoing<'a> {
    pub to: &'a str,
    pub subject: &'a str,
    pub text: &'a str,
    pub html: &'a str,
}

#[derive(Debug, thiserror::Error)]
pub enum SendError {
    /// Worth trying again later (connection refused, 4xx reply, timeout).
    #[error("temporary: {0}")]
    Transient(String),
    /// Will never work (invalid address, 5xx reply).
    #[error("permanent: {0}")]
    Permanent(String),
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
        let to: Mailbox = email.to.parse().map_err(|_| SendError::Permanent("invalid recipient address".into()))?;
        let message = Message::builder()
            .from(self.from.clone())
            .to(to)
            .subject(email.subject)
            .multipart(
                MultiPart::alternative()
                    .singlepart(SinglePart::builder().header(ContentType::TEXT_PLAIN).body(email.text.to_owned()))
                    .singlepart(SinglePart::builder().header(ContentType::TEXT_HTML).body(email.html.to_owned())),
            )
            .map_err(|e| SendError::Permanent(e.to_string()))?;
        match self.transport.send(message).await {
            Ok(_) => Ok(()),
            Err(e) if e.is_permanent() => Err(SendError::Permanent(e.to_string())),
            Err(e) => Err(SendError::Transient(e.to_string())),
        }
    }
}
