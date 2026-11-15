use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use utoipa::ToSchema;
use uuid::Uuid;
use validator::Validate;

use crate::store::{Invitation, MemberRow, Organization, Profile};

#[derive(Debug, Serialize, ToSchema)]
pub struct ProfileDto {
    pub user_id: Uuid,
    pub email: String,
    pub display_name: String,
    pub timezone: String,
    pub locale: String,
    pub version: i64,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

impl From<Profile> for ProfileDto {
    fn from(p: Profile) -> Self {
        Self {
            user_id: p.user_id,
            email: p.email,
            display_name: p.display_name,
            timezone: p.timezone,
            locale: p.locale,
            version: p.version,
            created_at: p.created_at,
        }
    }
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateProfileRequest {
    #[validate(length(min = 1, max = 80, message = "Name must be between 1 and 80 characters."))]
    pub display_name: Option<String>,
    /// IANA time zone name, e.g. `Europe/Berlin`.
    #[validate(length(min = 1, max = 64, message = "Time zone is not valid."))]
    pub timezone: Option<String>,
    /// BCP 47 language tag, e.g. `en` or `pt-BR`.
    #[validate(length(min = 2, max = 16, message = "Locale is not valid."))]
    pub locale: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct OrganizationDto {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
    /// The caller's role in this organization.
    pub role: String,
    /// Members must have two-step verification on to work in it.
    pub require_two_step: bool,
    pub version: i64,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

impl OrganizationDto {
    pub fn new(org: Organization, role: &str) -> Self {
        Self {
            id: org.id,
            name: org.name,
            slug: org.slug,
            role: role.to_owned(),
            require_two_step: org.require_two_step,
            version: org.version,
            created_at: org.created_at,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct OrganizationList {
    pub items: Vec<OrganizationDto>,
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateOrganizationRequest {
    #[validate(length(min = 2, max = 80, message = "Name must be between 2 and 80 characters."))]
    pub name: String,
    /// URL name. Derived from `name` when omitted.
    pub slug: Option<String>,
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateOrganizationRequest {
    #[validate(length(min = 2, max = 80, message = "Name must be between 2 and 80 characters."))]
    pub name: Option<String>,
    /// Require two-step verification of every member.
    pub require_two_step: Option<bool>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct MemberDto {
    pub user_id: Uuid,
    pub display_name: String,
    pub email: String,
    pub org_role: String,
    pub status: String,
    #[serde(with = "time::serde::rfc3339")]
    pub joined_at: OffsetDateTime,
}

impl From<MemberRow> for MemberDto {
    fn from(m: MemberRow) -> Self {
        Self {
            user_id: m.user_id,
            display_name: m.display_name,
            email: m.email,
            org_role: m.org_role,
            status: m.status,
            joined_at: m.joined_at,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct MemberPage {
    pub items: Vec<MemberDto>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct MemberQuery {
    pub limit: Option<u32>,
    pub cursor: Option<String>,
    /// Filter by name or email (substring, case-insensitive).
    pub query: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateMemberRequest {
    /// owner | admin | member | guest
    pub org_role: Option<String>,
    /// active | suspended
    pub status: Option<String>,
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateInvitationRequest {
    #[validate(email(message = "Enter a valid email address."), length(max = 254, message = "Email is too long."))]
    pub email: String,
    /// admin | member | guest
    pub org_role: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct InvitationDto {
    pub id: Uuid,
    pub email: String,
    pub org_role: String,
    pub invited_by: Uuid,
    pub invited_by_name: Option<String>,
    /// pending | expired
    pub state: String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: OffsetDateTime,
    /// Only present in the response to the request that created the invitation; the
    /// token inside it is not stored and cannot be shown again.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accept_url: Option<String>,
}

impl InvitationDto {
    pub fn new(i: Invitation, accept_url: Option<String>) -> Self {
        Self {
            state: i.state().to_owned(),
            id: i.id,
            email: i.email,
            org_role: i.org_role,
            invited_by: i.invited_by,
            invited_by_name: i.invited_by_name,
            created_at: i.created_at,
            expires_at: i.expires_at,
            accept_url,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct InvitationList {
    pub items: Vec<InvitationDto>,
}

/// What an invitee may see before signing in: enough to decide, nothing more.
#[derive(Debug, Serialize, ToSchema)]
pub struct InvitationPreview {
    pub organization_name: String,
    pub email: String,
    pub org_role: String,
    pub invited_by_name: Option<String>,
    /// pending | expired
    pub state: String,
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: OffsetDateTime,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AcceptInvitationResponse {
    pub organization_id: Uuid,
    pub organization_name: String,
}
