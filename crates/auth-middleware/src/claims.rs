use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const API_AUDIENCE: &str = "platform-api";
pub const ACCESS_TOKEN_TTL_SECS: i64 = 600;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TokenKind {
    /// Authenticated person, no organization selected. Cannot touch tenant data.
    Account,
    /// Authenticated person acting inside one organization (tenant).
    Org,
}

/// Access-token claims. `tid`, `org` and `roles` are only present on organization tokens
/// and are set exclusively by the identity service after a membership check.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    pub iss: String,
    pub aud: String,
    pub sub: Uuid,
    /// Session id; used for revocation.
    pub sid: Uuid,
    pub jti: Uuid,
    pub iat: i64,
    pub exp: i64,
    pub typ: TokenKind,
    pub email: String,
    /// The person proved they receive mail at `email`. Tokens from before this claim existed
    /// read as unverified.
    #[serde(default)]
    pub email_verified: bool,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tid: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub roles: Vec<String>,
    /// Policy version the roles were read at.
    #[serde(default)]
    pub pv: i64,
    /// Issued in exchange for an API token rather than from a signed-in session: `sid` is then
    /// the API token's id. Such tokens cannot manage credentials (API tokens, sessions,
    /// two-step verification) or switch organization.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub api: bool,
}
