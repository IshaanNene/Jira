//! Project automation: declarative rules that react to issues being created or moved.
//!
//! A rule is data from a closed set of types — a trigger, conditions and up to five actions —
//! validated on save and interpreted here. Nothing in a rule is ever executed as code.
//!
//! Rules run inside the transaction of the change that triggers them, as the person who made
//! it: their effects commit or roll back together with it, and a failed action (say, assigning
//! someone who has left the workspace) skips that rule and is recorded, without failing the
//! person's own change. Rules cannot change status, so a rule's changes never trigger rules.

use auth_middleware::OrgContext;
use error_handling::{ApiError, ApiResult};
use event_contracts::payloads::{IssueAssignedV1, IssueUpdatedV1};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{FromRow, PgConnection};
use tenant_context::TenantTx;
use time::OffsetDateTime;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{
    domain::{issue_type, normalize_labels, validate_priority},
    http::{emit, ensure_assignable, Changes},
    store::{self, Issue},
    AppState,
};

pub const MAX_RULES_PER_PROJECT: i64 = 50;
pub const MAX_ACTIONS: usize = 5;
const RUNS_KEPT: i64 = 100;

// --- the rule model ------------------------------------------------------------

/// What starts a rule.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Trigger {
    /// A new issue (including a clone).
    IssueCreated,
    /// An issue moved to another status; to one status only when `to_status` is set.
    IssueTransitioned {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        to_status: Option<String>,
    },
}

/// All must hold; an empty list places no restriction.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Conditions {
    #[serde(default)]
    pub issue_types: Vec<String>,
    #[serde(default)]
    pub priorities: Vec<String>,
    /// The issue has at least one of these labels.
    #[serde(default)]
    pub labels_any: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Assignee {
    Reporter,
    Unassigned,
    User { user_id: Uuid },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Action {
    Assign { assignee: Assignee },
    SetPriority { priority: String },
    AddLabels { labels: Vec<String> },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RuleDefinition {
    pub trigger: Trigger,
    #[serde(default)]
    pub conditions: Conditions,
    pub actions: Vec<Action>,
}

fn status_key_ok(key: &str) -> bool {
    (1..=64).contains(&key.len()) && key.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

fn bad_labels(field: &str) -> ApiError {
    ApiError::invalid(field, "invalid_label", "Labels use letters, numbers, hyphens, underscores and dots, at most 20.")
}

/// Check and normalise a definition; the stored form is always the normalised one.
pub fn validate(mut def: RuleDefinition) -> ApiResult<RuleDefinition> {
    if let Trigger::IssueTransitioned { to_status: Some(s) } = &mut def.trigger {
        *s = s.trim().to_owned();
        if !status_key_ok(s) {
            return Err(ApiError::invalid(
                "definition.trigger.to_status",
                "invalid_status",
                "Choose a status from the workflow.",
            ));
        }
    }
    for t in &def.conditions.issue_types {
        issue_type(t)
            .map_err(|_| ApiError::invalid("definition.conditions.issue_types", "unknown_type", "Unknown issue type."))?;
    }
    def.conditions.issue_types.dedup();
    for p in &def.conditions.priorities {
        validate_priority(p)
            .map_err(|_| ApiError::invalid("definition.conditions.priorities", "unknown_priority", "Unknown priority."))?;
    }
    def.conditions.priorities.dedup();
    def.conditions.labels_any = normalize_labels(std::mem::take(&mut def.conditions.labels_any))
        .map_err(|_| bad_labels("definition.conditions.labels_any"))?;
    if def.actions.is_empty() || def.actions.len() > MAX_ACTIONS {
        return Err(ApiError::invalid("definition.actions", "action_count", "A rule needs between 1 and 5 actions."));
    }
    for action in &mut def.actions {
        match action {
            Action::Assign { .. } => {}
            Action::SetPriority { priority } => validate_priority(priority)
                .map_err(|_| ApiError::invalid("definition.actions", "unknown_priority", "Unknown priority."))?,
            Action::AddLabels { labels } => {
                *labels = normalize_labels(std::mem::take(labels)).map_err(|_| bad_labels("definition.actions"))?;
                if labels.is_empty() {
                    return Err(ApiError::invalid("definition.actions", "no_labels", "Add at least one label."));
                }
            }
        }
    }
    Ok(def)
}

/// What happened to an issue, as a rule sees it.
#[derive(Debug, Clone, Copy)]
pub enum Event<'a> {
    Created,
    Transitioned { to_status: &'a str },
}

pub fn triggered(trigger: &Trigger, event: Event<'_>) -> bool {
    match (trigger, event) {
        (Trigger::IssueCreated, Event::Created) => true,
        (Trigger::IssueTransitioned { to_status }, Event::Transitioned { to_status: to }) => {
            to_status.as_deref().is_none_or(|want| want == to)
        }
        _ => false,
    }
}

pub fn conditions_hold(c: &Conditions, issue: &Issue) -> bool {
    (c.issue_types.is_empty() || c.issue_types.contains(&issue.type_key))
        && (c.priorities.is_empty() || c.priorities.contains(&issue.priority))
        && (c.labels_any.is_empty() || c.labels_any.iter().any(|l| issue.labels.contains(l)))
}

// --- storage -----------------------------------------------------------------

#[derive(Debug, Clone, FromRow)]
pub struct Rule {
    pub id: Uuid,
    pub project_id: Uuid,
    pub name: String,
    pub enabled: bool,
    pub definition: Value,
    pub version: i64,
    pub created_by: Uuid,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

const RULE: &str = "id, project_id, name, enabled, definition, version, created_by, created_at, updated_at";

pub async fn rules_for_project(
    conn: &mut PgConnection,
    tenant_id: Uuid,
    project_id: Uuid,
    only_enabled: bool,
) -> ApiResult<Vec<Rule>> {
    Ok(sqlx::query_as(&format!(
        "SELECT {RULE} FROM automation_rules WHERE tenant_id = $1 AND project_id = $2 AND (enabled OR NOT $3) ORDER BY created_at, id"
    ))
    .bind(tenant_id)
    .bind(project_id)
    .bind(only_enabled)
    .fetch_all(conn)
    .await?)
}

pub async fn rule_by_id(conn: &mut PgConnection, tenant_id: Uuid, id: Uuid, lock: bool) -> ApiResult<Option<Rule>> {
    let lock = if lock { " FOR UPDATE" } else { "" };
    Ok(sqlx::query_as(&format!("SELECT {RULE} FROM automation_rules WHERE tenant_id = $1 AND id = $2{lock}"))
        .bind(tenant_id)
        .bind(id)
        .fetch_optional(conn)
        .await?)
}

pub async fn count_rules(conn: &mut PgConnection, tenant_id: Uuid, project_id: Uuid) -> ApiResult<i64> {
    Ok(sqlx::query_scalar("SELECT count(*) FROM automation_rules WHERE tenant_id = $1 AND project_id = $2")
        .bind(tenant_id)
        .bind(project_id)
        .fetch_one(conn)
        .await?)
}

pub async fn insert_rule(
    conn: &mut PgConnection,
    tenant_id: Uuid,
    project_id: Uuid,
    name: &str,
    enabled: bool,
    definition: &RuleDefinition,
    created_by: Uuid,
) -> ApiResult<Rule> {
    Ok(sqlx::query_as(&format!(
        "INSERT INTO automation_rules (id, tenant_id, project_id, name, enabled, definition, created_by) \
         VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING {RULE}"
    ))
    .bind(platform_common::new_id())
    .bind(tenant_id)
    .bind(project_id)
    .bind(name)
    .bind(enabled)
    .bind(json!(definition))
    .bind(created_by)
    .fetch_one(conn)
    .await?)
}

pub async fn update_rule(
    conn: &mut PgConnection,
    tenant_id: Uuid,
    id: Uuid,
    name: &str,
    enabled: bool,
    definition: &RuleDefinition,
) -> ApiResult<Rule> {
    Ok(sqlx::query_as(&format!(
        "UPDATE automation_rules SET name = $3, enabled = $4, definition = $5, version = version + 1, updated_at = now() \
         WHERE tenant_id = $1 AND id = $2 RETURNING {RULE}"
    ))
    .bind(tenant_id)
    .bind(id)
    .bind(name)
    .bind(enabled)
    .bind(json!(definition))
    .fetch_one(conn)
    .await?)
}

pub async fn delete_rule(conn: &mut PgConnection, tenant_id: Uuid, id: Uuid) -> ApiResult<()> {
    sqlx::query("DELETE FROM automation_rules WHERE tenant_id = $1 AND id = $2").bind(tenant_id).bind(id).execute(conn).await?;
    Ok(())
}

#[derive(Debug, Clone, FromRow)]
pub struct Run {
    pub id: Uuid,
    pub issue_id: Uuid,
    pub issue_key: String,
    pub actor_id: Uuid,
    pub outcome: String,
    pub detail: Option<String>,
    pub created_at: OffsetDateTime,
}

pub async fn runs(conn: &mut PgConnection, tenant_id: Uuid, rule_id: Uuid) -> ApiResult<Vec<Run>> {
    Ok(sqlx::query_as(
        "SELECT id, issue_id, issue_key, actor_id, outcome, detail, created_at FROM automation_runs \
         WHERE tenant_id = $1 AND rule_id = $2 ORDER BY created_at DESC, id DESC LIMIT 100",
    )
    .bind(tenant_id)
    .bind(rule_id)
    .fetch_all(conn)
    .await?)
}

async fn record_run(
    conn: &mut PgConnection,
    tenant_id: Uuid,
    rule_id: Uuid,
    issue: &Issue,
    actor_id: Uuid,
    outcome: &str,
    detail: Option<&str>,
) -> ApiResult<()> {
    sqlx::query(
        "INSERT INTO automation_runs (id, tenant_id, rule_id, issue_id, issue_key, actor_id, outcome, detail) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(platform_common::new_id())
    .bind(tenant_id)
    .bind(rule_id)
    .bind(issue.id)
    .bind(issue.key())
    .bind(actor_id)
    .bind(outcome)
    .bind(detail)
    .execute(&mut *conn)
    .await?;
    sqlx::query(
        "DELETE FROM automation_runs WHERE tenant_id = $1 AND rule_id = $2 AND id NOT IN \
         (SELECT id FROM automation_runs WHERE tenant_id = $1 AND rule_id = $2 ORDER BY created_at DESC, id DESC LIMIT $3)",
    )
    .bind(tenant_id)
    .bind(rule_id)
    .bind(RUNS_KEPT)
    .execute(conn)
    .await?;
    Ok(())
}

// --- running rules -------------------------------------------------------------

/// Apply one rule's actions to `issue`, recording each change against the rule's name.
/// Returns why the rule could not run, if it could not.
async fn apply_actions(
    state: &AppState,
    ctx: &OrgContext,
    rule_name: &str,
    actions: &[Action],
    issue: &mut Issue,
    changes: &mut Changes,
) -> Result<(), String> {
    for action in actions {
        match action {
            Action::Assign { assignee } => {
                let to = match assignee {
                    Assignee::Reporter => Some(issue.reporter_id),
                    Assignee::Unassigned => None,
                    Assignee::User { user_id } => Some(*user_id),
                };
                if let Some(user) = to {
                    if ensure_assignable(state, ctx, user).await.is_err() {
                        return Err("The person to assign is no longer a member of this workspace.".into());
                    }
                }
                let from = issue.assignee_id;
                issue.assignee_id = to;
                changes.record_by_rule("assignee", &from, &to, rule_name);
            }
            Action::SetPriority { priority } => {
                let from = std::mem::replace(&mut issue.priority, priority.clone());
                changes.record_by_rule("priority", &from, &issue.priority, rule_name);
            }
            Action::AddLabels { labels } => {
                let from = issue.labels.clone();
                for l in labels {
                    if !issue.labels.contains(l) {
                        issue.labels.push(l.clone());
                    }
                }
                if issue.labels.len() > 20 {
                    return Err("The issue would have more than 20 labels.".into());
                }
                changes.record_by_rule("labels", &from, &issue.labels, rule_name);
            }
        }
    }
    Ok(())
}

/// Run the project's enabled rules for `event` on `issue` (already saved in `tx`), in the order
/// they were created; each sees the changes of those before it. Everything the rules change is
/// saved as one new version with one history entry, and announced like any other edit.
/// Returns the issue as it now is.
pub async fn run(state: &AppState, ctx: &OrgContext, tx: &mut TenantTx, issue: Issue, event: Event<'_>) -> ApiResult<Issue> {
    let rules = rules_for_project(tx.conn(), ctx.tenant_id, issue.project_id, true).await?;
    if rules.is_empty() {
        return Ok(issue);
    }
    let mut next = issue.clone();
    let mut changes = Changes::default();
    for rule in rules {
        let Ok(def) = serde_json::from_value::<RuleDefinition>(rule.definition.clone()) else {
            tracing::warn!(rule_id = %rule.id, "stored automation rule does not parse; skipped");
            continue;
        };
        if !triggered(&def.trigger, event) || !conditions_hold(&def.conditions, &next) {
            continue;
        }
        // A rule applies all its actions or none of them.
        let mut attempt = next.clone();
        let mut rule_changes = Changes::default();
        match apply_actions(state, ctx, &rule.name, &def.actions, &mut attempt, &mut rule_changes).await {
            Ok(()) => {
                let outcome = if rule_changes.is_empty() { "no_change" } else { "applied" };
                record_run(tx.conn(), ctx.tenant_id, rule.id, &next, ctx.user_id, outcome, None).await?;
                next = attempt;
                changes.extend(rule_changes);
            }
            Err(why) => record_run(tx.conn(), ctx.tenant_id, rule.id, &next, ctx.user_id, "failed", Some(&why)).await?,
        }
    }
    if changes.is_empty() {
        return Ok(issue);
    }
    store::save_issue(tx.conn(), ctx.tenant_id, &next).await?;
    let version = issue.version + 1;
    store::add_history(tx.conn(), ctx.tenant_id, issue.id, ctx.user_id, version, &Value::Array(changes.0.clone())).await?;
    let (issue_id, key, project_id) = (issue.id, issue.key(), issue.project_id);
    let fields = changes.fields();
    emit(tx, ctx, issue_id, version, IssueUpdatedV1 { issue_id, key: key.clone(), project_id, changed_fields: fields }).await?;
    if next.assignee_id != issue.assignee_id {
        let assigned =
            IssueAssignedV1 { issue_id, key, project_id, assignee_id: next.assignee_id, previous_assignee_id: issue.assignee_id };
        emit(tx, ctx, issue_id, version, assigned).await?;
    }
    store::issue_by_id(tx.conn(), ctx.tenant_id, issue_id).await?.ok_or(ApiError::NotFound("issue"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn def(v: Value) -> Result<RuleDefinition, String> {
        serde_json::from_value::<RuleDefinition>(v)
            .map_err(|e| e.to_string())
            .and_then(|d| validate(d).map_err(|e| format!("{e:?}")))
    }

    #[test]
    fn definitions_are_closed_and_normalised() {
        let d = def(json!({
            "trigger": { "type": "issue_transitioned", "to_status": "in_review" },
            "conditions": { "issue_types": ["bug"], "labels_any": ["UI"] },
            "actions": [{ "type": "add_labels", "labels": ["Needs-QA", "needs-qa"] }, { "type": "assign", "assignee": { "kind": "reporter" } }]
        }))
        .unwrap();
        assert_eq!(d.conditions.labels_any, ["ui"]);
        assert_eq!(d.actions[0], Action::AddLabels { labels: vec!["needs-qa".into()] });

        for bad in [
            json!({ "trigger": { "type": "run_script" }, "actions": [{ "type": "set_priority", "priority": "high" }] }),
            json!({ "trigger": { "type": "issue_created" }, "actions": [] }),
            json!({ "trigger": { "type": "issue_created" }, "actions": [{ "type": "set_priority", "priority": "urgent" }] }),
            json!({ "trigger": { "type": "issue_created" }, "conditions": { "issue_types": ["saga"] }, "actions": [{ "type": "set_priority", "priority": "high" }] }),
            json!({ "trigger": { "type": "issue_transitioned", "to_status": "In Review; DROP" }, "actions": [{ "type": "set_priority", "priority": "high" }] }),
            json!({ "trigger": { "type": "issue_created" }, "conditions": { "script": "x" }, "actions": [{ "type": "set_priority", "priority": "high" }] }),
            json!({ "trigger": { "type": "issue_created" }, "actions": [{ "type": "add_labels", "labels": [] }] }),
        ] {
            assert!(def(bad.clone()).is_err(), "accepted {bad}");
        }
        let six = vec![json!({ "type": "set_priority", "priority": "high" }); 6];
        assert!(def(json!({ "trigger": { "type": "issue_created" }, "actions": six })).is_err());
    }

    #[test]
    fn triggers_match_their_event() {
        let any = Trigger::IssueTransitioned { to_status: None };
        let review = Trigger::IssueTransitioned { to_status: Some("in_review".into()) };
        assert!(triggered(&Trigger::IssueCreated, Event::Created));
        assert!(!triggered(&Trigger::IssueCreated, Event::Transitioned { to_status: "done" }));
        assert!(triggered(&any, Event::Transitioned { to_status: "done" }));
        assert!(triggered(&review, Event::Transitioned { to_status: "in_review" }));
        assert!(!triggered(&review, Event::Transitioned { to_status: "done" }));
        assert!(!triggered(&review, Event::Created));
    }
}
