//! Versioned event payloads. Adding an optional field is compatible; anything else is a
//! new struct with a new `SCHEMA_VERSION`. Golden fixtures in `tests/` pin the wire format.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{envelope::EventPayload, topics};

macro_rules! payload {
    ($ty:ty, $event:literal, $version:literal, $topic:expr, $aggregate:literal) => {
        impl EventPayload for $ty {
            const EVENT_TYPE: &'static str = $event;
            const SCHEMA_VERSION: u32 = $version;
            const TOPIC: &'static str = $topic;
            const AGGREGATE_TYPE: &'static str = $aggregate;
        }
    };
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UserRegisteredV1 {
    pub user_id: Uuid,
    pub email: String,
    pub display_name: String,
}
payload!(UserRegisteredV1, "identity.user_registered", 1, topics::IDENTITY, "account");

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LoginSucceededV1 {
    pub user_id: Uuid,
    pub session_id: Uuid,
}
payload!(LoginSucceededV1, "identity.login_succeeded", 1, topics::IDENTITY, "account");

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionRevokedV1 {
    pub user_id: Uuid,
    pub session_id: Uuid,
    pub reason: String,
}
payload!(SessionRevokedV1, "identity.session_revoked", 1, topics::IDENTITY, "account");

/// An account moved to a new, verified address.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EmailChangedV1 {
    pub user_id: Uuid,
    pub email: String,
    /// Consumers apply the change only over this address, so a replayed or stale event is a no-op.
    pub previous_email: String,
}
payload!(EmailChangedV1, "identity.email_changed", 1, topics::IDENTITY, "account");

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TenantProvisionedV1 {
    pub tenant_id: Uuid,
    pub organization_id: Uuid,
    pub deployment_tier: String,
    pub region: String,
    pub database_destination: String,
}
payload!(TenantProvisionedV1, "tenant.provisioned", 1, topics::TENANT, "tenant");

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OrganizationCreatedV1 {
    pub organization_id: Uuid,
    pub name: String,
    pub slug: String,
    pub owner_user_id: Uuid,
}
payload!(OrganizationCreatedV1, "organization.created", 1, topics::ORGANIZATION, "organization");

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OrganizationUpdatedV1 {
    pub organization_id: Uuid,
    pub name: String,
}
payload!(OrganizationUpdatedV1, "organization.updated", 1, topics::ORGANIZATION, "organization");

/// The workspace started or stopped requiring two-step verification.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OrganizationTwoStepChangedV1 {
    pub organization_id: Uuid,
    pub name: String,
    pub required: bool,
}
payload!(OrganizationTwoStepChangedV1, "organization.two_step_changed", 1, topics::ORGANIZATION, "organization");

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UserInvitedV1 {
    pub invitation_id: Uuid,
    pub organization_id: Uuid,
    pub email: String,
    pub org_role: String,
    pub invited_by: Uuid,
}
payload!(UserInvitedV1, "user.invited", 1, topics::ORGANIZATION, "organization_membership");

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MembershipChangedV1 {
    pub organization_id: Uuid,
    pub user_id: Uuid,
    /// joined | role_changed | suspended | reactivated | removed
    pub change: String,
    pub org_role: Option<String>,
}
payload!(MembershipChangedV1, "membership.changed", 1, topics::ORGANIZATION, "organization_membership");

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PermissionUpdatedV1 {
    pub version: i64,
    pub reason: String,
}
payload!(PermissionUpdatedV1, "permission.updated", 1, topics::PERMISSION, "policy");

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectCreatedV1 {
    pub project_id: Uuid,
    pub key: String,
    pub name: String,
    pub project_type: String,
    pub created_by: Uuid,
}
payload!(ProjectCreatedV1, "project.created", 1, topics::PROJECT, "project");

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectUpdatedV1 {
    pub project_id: Uuid,
    pub key: String,
    pub name: String,
    /// active | archived
    pub status: String,
}
payload!(ProjectUpdatedV1, "project.updated", 1, topics::PROJECT, "project");

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectDeletedV1 {
    pub project_id: Uuid,
    pub key: String,
}
payload!(ProjectDeletedV1, "project.deleted", 1, topics::PROJECT, "project");

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VersionReleasedV1 {
    pub project_id: Uuid,
    pub version_id: Uuid,
    pub name: String,
}
payload!(VersionReleasedV1, "version.released", 1, topics::PROJECT, "project");

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IssueCreatedV1 {
    pub issue_id: Uuid,
    pub key: String,
    pub project_id: Uuid,
    pub issue_type: String,
    pub summary: String,
    pub reporter_id: Uuid,
    pub assignee_id: Option<Uuid>,
}
payload!(IssueCreatedV1, "issue.created", 1, topics::ISSUE, "issue");

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IssueUpdatedV1 {
    pub issue_id: Uuid,
    pub key: String,
    pub project_id: Uuid,
    pub changed_fields: Vec<String>,
}
payload!(IssueUpdatedV1, "issue.updated", 1, topics::ISSUE, "issue");

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IssueTransitionedV1 {
    pub issue_id: Uuid,
    pub key: String,
    pub project_id: Uuid,
    pub from_status: String,
    pub to_status: String,
    /// `todo`, `in_progress` or `done`. Absent on events written before analytics needed them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_category: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_category: Option<String>,
}
payload!(IssueTransitionedV1, "issue.transitioned", 1, topics::ISSUE, "issue");

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IssueAssignedV1 {
    pub issue_id: Uuid,
    pub key: String,
    pub project_id: Uuid,
    pub assignee_id: Option<Uuid>,
    pub previous_assignee_id: Option<Uuid>,
}
payload!(IssueAssignedV1, "issue.assigned", 1, topics::ISSUE, "issue");

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IssueDeletedV1 {
    pub issue_id: Uuid,
    pub key: String,
    pub project_id: Uuid,
}
payload!(IssueDeletedV1, "issue.deleted", 1, topics::ISSUE, "issue");

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CommentCreatedV1 {
    pub comment_id: Uuid,
    pub issue_id: Uuid,
    pub key: String,
    pub project_id: Uuid,
    pub author_id: Uuid,
    pub mentioned_user_ids: Vec<Uuid>,
}
payload!(CommentCreatedV1, "comment.created", 1, topics::ISSUE, "issue");

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CommentDeletedV1 {
    pub comment_id: Uuid,
    pub issue_id: Uuid,
    pub key: String,
    /// Added after v1 shipped; absent in older events.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<Uuid>,
}
payload!(CommentDeletedV1, "comment.deleted", 1, topics::ISSUE, "issue");

/// People newly mentioned in a description (on create or edit) or in an edited comment.
/// A new comment's mentions travel in `comment.created` instead.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IssueMentionedV1 {
    pub issue_id: Uuid,
    pub key: String,
    pub project_id: Uuid,
    /// The edited comment; absent for a description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment_id: Option<Uuid>,
    pub mentioned_user_ids: Vec<Uuid>,
}
payload!(IssueMentionedV1, "issue.mentioned", 1, topics::ISSUE, "issue");

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkflowPublishedV1 {
    pub workflow_id: Uuid,
    pub name: String,
    pub version: i32,
}
payload!(WorkflowPublishedV1, "workflow.published", 1, topics::WORKFLOW, "workflow");

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkflowUpdatedV1 {
    pub workflow_id: Uuid,
    /// created | renamed | assigned | unassigned | deleted
    pub change: String,
}
payload!(WorkflowUpdatedV1, "workflow.updated", 1, topics::WORKFLOW, "workflow");

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BoardUpdatedV1 {
    pub board_id: Uuid,
    pub project_id: Uuid,
    /// created | renamed | columns_changed | deleted
    pub change: String,
}
payload!(BoardUpdatedV1, "board.updated", 1, topics::AGILE, "board");

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SprintCreatedV1 {
    pub sprint_id: Uuid,
    pub board_id: Uuid,
    pub project_id: Uuid,
    pub name: String,
}
payload!(SprintCreatedV1, "sprint.created", 1, topics::AGILE, "board");

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SprintStartedV1 {
    pub sprint_id: Uuid,
    pub board_id: Uuid,
    pub project_id: Uuid,
    pub name: String,
    pub issue_count: u32,
}
payload!(SprintStartedV1, "sprint.started", 1, topics::AGILE, "board");

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SprintCompletedV1 {
    pub sprint_id: Uuid,
    pub board_id: Uuid,
    pub project_id: Uuid,
    pub name: String,
    pub completed_issues: u32,
    pub incomplete_issues: u32,
    /// Where unfinished issues went; `None` = the backlog.
    pub moved_to_sprint_id: Option<Uuid>,
}
payload!(SprintCompletedV1, "sprint.completed", 1, topics::AGILE, "board");

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IssueRankedV1 {
    pub board_id: Uuid,
    pub project_id: Uuid,
    pub issue_ids: Vec<Uuid>,
    /// The sprint the issues are now planned into; `None` = the backlog.
    pub sprint_id: Option<Uuid>,
}
payload!(IssueRankedV1, "issue.ranked", 1, topics::AGILE, "board");

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NotificationCreatedV1 {
    pub notification_id: Uuid,
    pub recipient_id: Uuid,
    /// assigned | commented | mentioned | status_changed
    pub kind: String,
    pub issue_key: String,
}
payload!(NotificationCreatedV1, "notification.created", 1, topics::NOTIFICATION, "notification");

/// An administrative action that no domain event describes well enough for the audit
/// log (for example a change of someone's project role, where the permission service
/// does not know who asked). The aggregate id is the resource's id when it has one.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuditRecordedV1 {
    /// Dotted verb, e.g. `project.access_granted`.
    pub action: String,
    pub resource_type: String,
    pub resource_id: String,
    /// What a person would call the resource (a project key, a name); may be empty.
    pub resource_label: String,
    /// Short string facts about the change. No secrets, no free-form user content.
    #[serde(default)]
    pub details: std::collections::BTreeMap<String, String>,
}
payload!(AuditRecordedV1, "audit.recorded", 1, topics::AUDIT, "audit");

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Actor, EventEnvelope};

    #[test]
    fn envelope_carries_contract_metadata() {
        let org = Uuid::now_v7();
        let tenant = Uuid::now_v7();
        let user = Uuid::now_v7();
        let env = EventEnvelope::new(
            Some(tenant),
            org,
            1,
            Actor::User(user),
            OrganizationCreatedV1 { organization_id: org, name: "Acme".into(), slug: "acme".into(), owner_user_id: user },
        );
        assert_eq!(env.event_type, "organization.created");
        assert_eq!(env.topic(), topics::ORGANIZATION);
        assert_eq!(env.partition_key(), format!("{tenant}:{org}"));
        let json = serde_json::to_value(&env).unwrap();
        assert_eq!(json["schema_version"], 1);
        assert_eq!(json["actor"]["type"], "user");
        assert_eq!(json["payload"]["slug"], "acme");
    }

    /// Golden fixture: the published v1 wire format must keep deserializing.
    #[test]
    fn membership_changed_v1_wire_format_is_stable() {
        let raw = r#"{
            "event_id":"0195c1e2-0000-7000-8000-000000000001","event_type":"membership.changed","schema_version":1,
            "tenant_id":"0195c1e2-0000-7000-8000-000000000002","aggregate_type":"organization_membership",
            "aggregate_id":"0195c1e2-0000-7000-8000-000000000003","aggregate_version":4,
            "occurred_at":"2026-11-11T09:00:00Z","correlation_id":"req-1","causation_id":null,
            "actor":{"type":"system","id":"scim"},
            "payload":{"organization_id":"0195c1e2-0000-7000-8000-000000000003",
                       "user_id":"0195c1e2-0000-7000-8000-000000000004","change":"joined","org_role":"member",
                       "field_added_later":"ignored"}
        }"#;
        let env: EventEnvelope<MembershipChangedV1> = serde_json::from_str(raw).unwrap();
        assert_eq!(env.payload.change, "joined");
        assert_eq!(env.actor, Actor::System("scim".into()));
        assert_eq!(env.aggregate_version, 4);
    }
}

/// One issue in a filter digest, as the recipient could see it when the filter ran.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DigestIssueV1 {
    pub issue_id: Uuid,
    pub key: String,
    pub summary: String,
    /// Status key.
    pub status: String,
}

/// A subscribed saved filter ran for its subscriber; notification-service emails the result.
/// The issues are only those the recipient could see, and at most `MAX_DIGEST_ISSUES`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FilterDigestV1 {
    pub subscription_id: Uuid,
    pub filter_id: Uuid,
    pub recipient_id: Uuid,
    pub filter_name: String,
    pub query: String,
    pub total: u64,
    /// The real total is at least `total`.
    pub total_is_lower_bound: bool,
    pub issues: Vec<DigestIssueV1>,
}
payload!(FilterDigestV1, "filter.digest", 1, topics::SEARCH, "filter_subscription");

/// Most issues listed in one digest.
pub const MAX_DIGEST_ISSUES: usize = 20;
