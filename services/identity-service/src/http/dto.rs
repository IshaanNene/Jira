use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use utoipa::ToSchema;
use uuid::Uuid;
use validator::Validate;

use crate::store::{Account, Session};

#[derive(Debug, Deserialize, Validate, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RegisterRequest {
    #[validate(email(message = "Enter a valid email address."), length(max = 254, message = "Email is too long."))]
    pub email: String,
    #[validate(length(min = 10, max = 128, message = "Password must be between 10 and 128 characters."))]
    pub password: String,
    #[validate(length(min = 1, max = 80, message = "Name must be between 1 and 80 characters."))]
    pub display_name: String,
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct LoginRequest {
    #[validate(length(min = 1, max = 254, message = "Email is required."))]
    pub email: String,
    #[validate(length(min = 1, max = 128, message = "Password is required."))]
    pub password: String,
}

#[derive(Debug, Default, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RefreshRequest {
    /// When set, the new access token is scoped to this organization (if the caller is a member).
    pub organization_id: Option<Uuid>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ExchangeRequest {
    pub organization_id: Uuid,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AccountDto {
    pub id: Uuid,
    pub email: String,
    pub display_name: String,
    pub email_verified: bool,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

impl From<&Account> for AccountDto {
    fn from(a: &Account) -> Self {
        Self {
            id: a.id,
            email: a.email.clone(),
            display_name: a.display_name.clone(),
            email_verified: a.email_verified_at.is_some(),
            created_at: a.created_at,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TokenOrganization {
    pub id: Uuid,
    pub role: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TokenResponse {
    pub access_token: String,
    pub token_type: &'static str,
    /// Seconds until the access token expires.
    pub expires_in: i64,
    pub account: AccountDto,
    /// Present when the token is scoped to an organization.
    pub organization: Option<TokenOrganization>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SessionDto {
    pub id: Uuid,
    pub current: bool,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub last_seen_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: OffsetDateTime,
    pub ip: Option<String>,
    pub user_agent: Option<String>,
    /// How it was signed in to: `password`, `two_step` or `passkey`.
    pub sign_in_method: String,
}

impl SessionDto {
    pub fn from_session(s: Session, current: Uuid) -> Self {
        Self {
            current: s.id == current,
            id: s.id,
            created_at: s.created_at,
            last_seen_at: s.last_seen_at,
            expires_at: s.expires_at,
            ip: s.ip,
            sign_in_method: s.auth_method,
            user_agent: s.user_agent,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SessionList {
    pub items: Vec<SessionDto>,
}
