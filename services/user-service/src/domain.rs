//! Organization and membership rules. Pure functions: no I/O, exhaustively unit-tested.

use auth_middleware::{OrgPermission, OrgRole};
use error_handling::{ApiError, ApiResult};

const RESERVED_SLUGS: &[&str] = &[
    "admin",
    "api",
    "app",
    "auth",
    "billing",
    "docs",
    "help",
    "invite",
    "login",
    "logout",
    "new",
    "onboarding",
    "register",
    "settings",
    "signup",
    "static",
    "status",
    "support",
    "www",
];

/// Lowercase letters, digits and single hyphens; 3–40 characters; not reserved.
pub fn validate_slug(slug: &str) -> ApiResult<()> {
    let ok = (3..=40).contains(&slug.len())
        && slug.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !slug.starts_with('-')
        && !slug.ends_with('-')
        && !slug.contains("--");
    if !ok {
        return Err(ApiError::invalid("slug", "invalid_slug", "Use 3–40 lowercase letters, numbers and single hyphens."));
    }
    if RESERVED_SLUGS.contains(&slug) {
        return Err(ApiError::invalid("slug", "reserved_slug", "This URL name is reserved. Choose another."));
    }
    Ok(())
}

/// Derive a URL slug from an organization name ("Acme Rockets, Inc." -> "acme-rockets-inc").
pub fn slugify(name: &str) -> String {
    let mut slug = String::with_capacity(name.len());
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c.to_ascii_lowercase());
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let mut slug: String = slug.trim_matches('-').chars().take(40).collect();
    slug = slug.trim_matches('-').to_owned();
    while slug.len() < 3 {
        slug.push('0');
    }
    slug
}

/// Who may invite whom: administrators and owners, never above their own role, and never
/// directly as owner (ownership is granted to an existing member by an owner).
pub fn check_can_invite(actor: OrgRole, invited_as: OrgRole) -> ApiResult<()> {
    if !actor.allows(OrgPermission::InviteMembers) {
        return Err(ApiError::Forbidden);
    }
    if invited_as == OrgRole::Owner {
        return Err(ApiError::invalid("org_role", "owner_not_invitable", "Invite as admin, then transfer ownership."));
    }
    if invited_as > actor {
        return Err(ApiError::Forbidden);
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemberStatus {
    Active,
    Suspended,
}

impl MemberStatus {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "active" => Some(Self::Active),
            "suspended" => Some(Self::Suspended),
            _ => None,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Suspended => "suspended",
        }
    }
}

/// The state needed to judge a change to one membership.
#[derive(Debug, Clone, Copy)]
pub struct MemberChangeContext {
    pub actor_role: OrgRole,
    pub actor_is_target: bool,
    pub target_role: OrgRole,
    pub target_status: MemberStatus,
    /// Number of *active* owners in the organization right now (read under lock).
    pub active_owner_count: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemberChange {
    SetRole(OrgRole),
    SetStatus(MemberStatus),
    Remove,
}

/// Decide whether `change` is allowed.
///
/// * Managing members needs the admin role; anyone may remove *themselves* (leave).
/// * Only owners may touch an owner or grant ownership.
/// * Nobody may suspend themselves.
/// * The organization must always keep at least one active owner.
pub fn check_member_change(ctx: MemberChangeContext, change: MemberChange) -> ApiResult<()> {
    let leaving = ctx.actor_is_target && change == MemberChange::Remove;
    if !leaving && !ctx.actor_role.allows(OrgPermission::ManageMembers) {
        return Err(ApiError::Forbidden);
    }
    let touches_ownership = ctx.target_role == OrgRole::Owner || change == MemberChange::SetRole(OrgRole::Owner);
    if touches_ownership && !leaving && !ctx.actor_role.allows(OrgPermission::ManageOwnership) {
        return Err(ApiError::Forbidden);
    }
    if ctx.actor_is_target && matches!(change, MemberChange::SetStatus(MemberStatus::Suspended)) {
        return Err(ApiError::Conflict("You cannot suspend yourself.".into()));
    }

    let target_is_active_owner = ctx.target_role == OrgRole::Owner && ctx.target_status == MemberStatus::Active;
    let removes_an_active_owner = target_is_active_owner
        && match change {
            MemberChange::SetRole(role) => role != OrgRole::Owner,
            MemberChange::SetStatus(status) => status == MemberStatus::Suspended,
            MemberChange::Remove => true,
        };
    if removes_an_active_owner && ctx.active_owner_count <= 1 {
        return Err(ApiError::Conflict("An organization must keep at least one active owner.".into()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(actor: OrgRole, target: OrgRole) -> MemberChangeContext {
        MemberChangeContext {
            actor_role: actor,
            actor_is_target: false,
            target_role: target,
            target_status: MemberStatus::Active,
            active_owner_count: 2,
        }
    }

    #[test]
    fn slugify_and_validate() {
        assert_eq!(slugify("Acme Rockets, Inc."), "acme-rockets-inc");
        assert_eq!(slugify("  --Ünïcode & Co  "), "n-code-co");
        assert_eq!(slugify("A"), "a00");
        assert!(validate_slug(&slugify("Acme Rockets, Inc.")).is_ok());
        assert!(validate_slug(&slugify(&"x".repeat(200))).is_ok());
        for bad in ["ab", "Has-Caps", "double--hyphen", "-lead", "trail-", "with space", "admin", "api"] {
            assert!(validate_slug(bad).is_err(), "{bad} should be rejected");
        }
    }

    #[test]
    fn members_and_guests_cannot_manage_anyone() {
        for actor in [OrgRole::Member, OrgRole::Guest] {
            for change in
                [MemberChange::SetRole(OrgRole::Admin), MemberChange::SetStatus(MemberStatus::Suspended), MemberChange::Remove]
            {
                assert!(matches!(check_member_change(ctx(actor, OrgRole::Member), change), Err(ApiError::Forbidden)));
            }
        }
    }

    #[test]
    fn admins_manage_non_owners_only() {
        assert!(check_member_change(ctx(OrgRole::Admin, OrgRole::Member), MemberChange::SetRole(OrgRole::Admin)).is_ok());
        assert!(check_member_change(ctx(OrgRole::Admin, OrgRole::Member), MemberChange::Remove).is_ok());
        // Privilege escalation attempts:
        assert!(check_member_change(ctx(OrgRole::Admin, OrgRole::Member), MemberChange::SetRole(OrgRole::Owner)).is_err());
        assert!(check_member_change(ctx(OrgRole::Admin, OrgRole::Owner), MemberChange::SetRole(OrgRole::Member)).is_err());
        assert!(check_member_change(ctx(OrgRole::Admin, OrgRole::Owner), MemberChange::Remove).is_err());
        let mut self_promote = ctx(OrgRole::Admin, OrgRole::Admin);
        self_promote.actor_is_target = true;
        assert!(check_member_change(self_promote, MemberChange::SetRole(OrgRole::Owner)).is_err());
    }

    #[test]
    fn last_active_owner_is_protected() {
        let mut last = ctx(OrgRole::Owner, OrgRole::Owner);
        last.active_owner_count = 1;
        last.actor_is_target = true;
        assert!(check_member_change(last, MemberChange::SetRole(OrgRole::Admin)).is_err());
        assert!(check_member_change(last, MemberChange::Remove).is_err());
        last.active_owner_count = 2;
        assert!(check_member_change(last, MemberChange::SetRole(OrgRole::Admin)).is_ok());
        assert!(check_member_change(last, MemberChange::Remove).is_ok());
        // A suspended owner does not count, and removing one is fine.
        let mut suspended = ctx(OrgRole::Owner, OrgRole::Owner);
        suspended.target_status = MemberStatus::Suspended;
        suspended.active_owner_count = 1;
        assert!(check_member_change(suspended, MemberChange::Remove).is_ok());
    }

    #[test]
    fn anyone_can_leave_but_nobody_suspends_themselves() {
        let mut me = ctx(OrgRole::Guest, OrgRole::Guest);
        me.actor_is_target = true;
        assert!(check_member_change(me, MemberChange::Remove).is_ok());
        let mut admin = ctx(OrgRole::Admin, OrgRole::Admin);
        admin.actor_is_target = true;
        assert!(check_member_change(admin, MemberChange::SetStatus(MemberStatus::Suspended)).is_err());
    }

    #[test]
    fn invitation_rules() {
        assert!(check_can_invite(OrgRole::Admin, OrgRole::Admin).is_ok());
        assert!(check_can_invite(OrgRole::Admin, OrgRole::Guest).is_ok());
        assert!(check_can_invite(OrgRole::Owner, OrgRole::Admin).is_ok());
        assert!(check_can_invite(OrgRole::Owner, OrgRole::Owner).is_err());
        assert!(check_can_invite(OrgRole::Member, OrgRole::Guest).is_err());
    }
}
