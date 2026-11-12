//! Organization-level authorization, decided from the signed `roles` claim.
//!
//! Project- and issue-level permissions are evaluated from policy snapshots owned by
//! the permission service (ADR-0007); this module only covers what an organization
//! role alone decides.

use serde::{Deserialize, Serialize};

/// Ordered from least to most privileged so `max()` picks the strongest role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrgRole {
    Guest,
    Member,
    Admin,
    Owner,
}

impl OrgRole {
    pub const ALL: [OrgRole; 4] = [OrgRole::Guest, OrgRole::Member, OrgRole::Admin, OrgRole::Owner];

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "guest" => Some(OrgRole::Guest),
            "member" => Some(OrgRole::Member),
            "admin" => Some(OrgRole::Admin),
            "owner" => Some(OrgRole::Owner),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            OrgRole::Guest => "guest",
            OrgRole::Member => "member",
            OrgRole::Admin => "admin",
            OrgRole::Owner => "owner",
        }
    }

    pub fn allows(self, permission: OrgPermission) -> bool {
        self >= permission.minimum_role()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OrgPermission {
    ViewOrganization,
    ViewMembers,
    InviteMembers,
    ManageMembers,
    ManageTeams,
    ManageOrganization,
    ManageSecurity,
    ViewAuditLog,
    /// Granting or revoking the owner role, deleting the organization.
    ManageOwnership,
}

impl OrgPermission {
    pub const fn minimum_role(self) -> OrgRole {
        match self {
            OrgPermission::ViewOrganization => OrgRole::Guest,
            OrgPermission::ViewMembers => OrgRole::Member,
            OrgPermission::InviteMembers
            | OrgPermission::ManageMembers
            | OrgPermission::ManageTeams
            | OrgPermission::ManageOrganization
            | OrgPermission::ManageSecurity
            | OrgPermission::ViewAuditLog => OrgRole::Admin,
            OrgPermission::ManageOwnership => OrgRole::Owner,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roles_are_ordered_by_privilege() {
        assert!(OrgRole::Owner > OrgRole::Admin && OrgRole::Admin > OrgRole::Member && OrgRole::Member > OrgRole::Guest);
    }

    #[test]
    fn members_cannot_administer() {
        assert!(OrgRole::Member.allows(OrgPermission::ViewMembers));
        assert!(!OrgRole::Member.allows(OrgPermission::InviteMembers));
        assert!(!OrgRole::Member.allows(OrgPermission::ManageMembers));
        assert!(!OrgRole::Guest.allows(OrgPermission::ViewMembers));
    }

    #[test]
    fn only_owners_manage_ownership() {
        assert!(!OrgRole::Admin.allows(OrgPermission::ManageOwnership));
        assert!(OrgRole::Owner.allows(OrgPermission::ManageOwnership));
    }

    #[test]
    fn unknown_role_strings_parse_to_nothing() {
        assert_eq!(OrgRole::parse("superuser"), None);
        for role in OrgRole::ALL {
            assert_eq!(OrgRole::parse(role.as_str()), Some(role));
        }
    }
}
