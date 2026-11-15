pub const IDENTITY: &str = "platform.identity.v1";
pub const TENANT: &str = "platform.tenant.v1";
pub const ORGANIZATION: &str = "platform.organization.v1";
pub const PROJECT: &str = "platform.project.v1";
pub const ISSUE: &str = "platform.issue.v1";
pub const WORKFLOW: &str = "platform.workflow.v1";
pub const AGILE: &str = "platform.agile.v1";
pub const PERMISSION: &str = "platform.permission.v1";
pub const AUTOMATION: &str = "platform.automation.v1";
pub const NOTIFICATION: &str = "platform.notification.v1";
pub const AUDIT: &str = "platform.audit.v1";

pub fn dead_letter(topic: &str) -> String {
    format!("{topic}.dlq")
}
pub const SEARCH: &str = "platform.search.v1";
