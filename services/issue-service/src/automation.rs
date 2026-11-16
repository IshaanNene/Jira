//! Project automation: declarative rules that react to issues being created or moved, or
//! that run on a schedule.
//!
//! A rule is data from a closed set of types — a trigger, conditions and up to five actions —
//! validated on save and interpreted here. Nothing in a rule is ever executed as code.
//!
//! Rules triggered by a change run inside its transaction, as the person who made it: their
//! effects commit or roll back together with it, and a failed action (say, assigning someone
//! who has left the workspace) skips that rule and is recorded, without failing the person's
//! own change. Scheduled rules run as the administrator who last saved them, with that
//! person's access at run time, one issue per transaction. A rule may move an issue through
//! its workflow, but changes made by rules never trigger rules, so rules cannot chain or loop.

use std::time::Duration as StdDuration;

use auth_middleware::{OrgContext, ProjectPermission, ProjectRole};
use error_handling::{ApiError, ApiResult};
use event_contracts::payloads::{CommentCreatedV1, IssueAssignedV1, IssueTransitionedV1, IssueUpdatedV1};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{FromRow, PgConnection, PgPool};
use tenant_context::TenantTx;
use time::{Duration, OffsetDateTime, Time};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{
    domain::{issue_type, normalize_labels, validate_priority},
    http::{emit, ensure_assignable, enter_status, facts, status_display, Changes},
    richtext,
    store::{self, Issue},
    workflows::{Evaluation, Scope},
    AppState,
};

pub const MAX_RULES_PER_PROJECT: i64 = 50;
pub const MAX_ACTIONS: usize = 5;
pub const MAX_COMMENT_CHARS: usize = 2000;
/// Issues one scheduled run looks at, least recently updated first.
pub const MAX_SCHEDULED_ISSUES: i64 = 100;
const RUNS_KEPT: i64 = 100;

// --- the rule model ------------------------------------------------------------

/// What starts a rule.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Trigger {
    /// A new issue (including a clone).
    IssueCreated,
    /// An issue moved to another status; to one status only when `to_status` is set.
    IssueTransitioned {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        to_status: Option<String>,
    },
    /// Every day, or every week on `weekday` (ISO, 1 = Monday), at `hour`:00 UTC: the rule
    /// runs on the project's issues that meet its conditions.
    Scheduled {
        frequency: Frequency,
        hour: u8,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        weekday: Option<u8>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Frequency {
    Daily,
    Weekly,
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
    /// The issue is in one of these statuses (keys).
    #[serde(default)]
    pub statuses: Vec<String>,
    /// Nobody has changed the issue for at least this many days. Scheduled rules only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub untouched_days: Option<u16>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Assignee {
    Reporter,
    Unassigned,
    User { user_id: Uuid },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    Assign {
        assignee: Assignee,
    },
    SetPriority {
        priority: String,
    },
    AddLabels {
        labels: Vec<String>,
    },
    /// Move the issue to this status, if its workflow allows the move for the person the
    /// rule acts as.
    Transition {
        to_status: String,
    },
    /// Post a plain-text comment, named as the rule's.
    AddComment {
        body: String,
    },
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

fn bad_status(field: &str) -> ApiError {
    ApiError::invalid(field, "invalid_status", "Choose a status from the workflow.")
}

/// Check and normalise a definition; the stored form is always the normalised one.
pub fn validate(mut def: RuleDefinition) -> ApiResult<RuleDefinition> {
    match &mut def.trigger {
        Trigger::IssueTransitioned { to_status: Some(s) } => {
            *s = s.trim().to_owned();
            if !status_key_ok(s) {
                return Err(bad_status("definition.trigger.to_status"));
            }
        }
        Trigger::Scheduled { frequency, hour, weekday } => {
            if *hour > 23 {
                return Err(ApiError::invalid("definition.trigger.hour", "out_of_range", "Choose an hour from 0 to 23 (UTC)."));
            }
            match (*frequency, *weekday) {
                (Frequency::Weekly, Some(1..=7)) | (Frequency::Daily, None) => {}
                (Frequency::Weekly, _) => {
                    return Err(ApiError::invalid(
                        "definition.trigger.weekday",
                        "required",
                        "Choose a day of the week (1 = Monday).",
                    ))
                }
                (Frequency::Daily, Some(_)) => {
                    return Err(ApiError::invalid("definition.trigger.weekday", "not_allowed", "A daily rule runs every day."))
                }
            }
        }
        _ => {}
    }
    for s in &mut def.conditions.statuses {
        *s = s.trim().to_owned();
        if !status_key_ok(s) {
            return Err(bad_status("definition.conditions.statuses"));
        }
    }
    def.conditions.statuses.sort();
    def.conditions.statuses.dedup();
    if def.conditions.statuses.len() > 20 {
        return Err(ApiError::invalid("definition.conditions.statuses", "too_many", "Choose at most 20 statuses."));
    }
    if let Some(days) = def.conditions.untouched_days {
        if !matches!(def.trigger, Trigger::Scheduled { .. }) {
            return Err(ApiError::invalid(
                "definition.conditions.untouched_days",
                "scheduled_only",
                "Only a scheduled rule can look for issues nobody has changed.",
            ));
        }
        if !(1..=365).contains(&days) {
            return Err(ApiError::invalid("definition.conditions.untouched_days", "out_of_range", "Choose from 1 to 365 days."));
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
    let count = |f: fn(&Action) -> bool| def.actions.iter().filter(|a| f(a)).count();
    if count(|a| matches!(a, Action::Transition { .. })) > 1 {
        return Err(ApiError::invalid("definition.actions", "one_transition", "A rule can move an issue once."));
    }
    if count(|a| matches!(a, Action::AddComment { .. })) > 1 {
        return Err(ApiError::invalid("definition.actions", "one_comment", "A rule can post one comment."));
    }
    for action in &mut def.actions {
        match action {
            Action::Assign { .. } => {}
            Action::Transition { to_status } => {
                *to_status = to_status.trim().to_owned();
                if !status_key_ok(to_status) {
                    return Err(bad_status("definition.actions"));
                }
            }
            Action::AddComment { body } => {
                *body = body.trim().to_owned();
                let chars = body.chars().count();
                if chars == 0 || chars > MAX_COMMENT_CHARS {
                    return Err(ApiError::invalid(
                        "definition.actions",
                        "comment_length",
                        "Write a comment of up to 2,000 characters.",
                    ));
                }
                if body.chars().any(|c| c.is_control() && c != '\n') {
                    return Err(ApiError::invalid("definition.actions", "comment_text", "Use plain text in the comment."));
                }
            }
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

/// A comment's text as a document: one paragraph per line, text only.
pub fn comment_document(body: &str) -> Value {
    let paragraphs: Vec<Value> = body
        .lines()
        .map(|line| match line.trim_end() {
            "" => json!({ "type": "paragraph" }),
            text => json!({ "type": "paragraph", "content": [{ "type": "text", "text": text }] }),
        })
        .collect();
    json!({ "type": "doc", "content": paragraphs })
}

/// The first run strictly after `now`: at `hour`:00 UTC, every day or on `weekday`.
pub fn next_run(now: OffsetDateTime, frequency: Frequency, hour: u8, weekday: Option<u8>) -> OffsetDateTime {
    let at = Time::from_hms(hour.min(23), 0, 0).expect("a valid hour");
    let mut run = now.date().with_time(at).assume_utc();
    match (frequency, weekday) {
        (Frequency::Weekly, Some(day)) => {
            let today = now.weekday().number_from_monday();
            run += Duration::days(i64::from((day + 7 - today) % 7));
            if run <= now {
                run += Duration::weeks(1);
            }
        }
        _ => {
            if run <= now {
                run += Duration::days(1);
            }
        }
    }
    run
}

/// What happened to an issue, as a rule sees it.
#[derive(Debug, Clone, Copy)]
pub enum Event<'a> {
    Created,
    Transitioned {
        to_status: &'a str,
    },
    /// A scheduled rule's time came.
    Scheduled,
}

pub fn triggered(trigger: &Trigger, event: Event<'_>) -> bool {
    match (trigger, event) {
        (Trigger::IssueCreated, Event::Created) => true,
        (Trigger::Scheduled { .. }, Event::Scheduled) => true,
        (Trigger::IssueTransitioned { to_status }, Event::Transitioned { to_status: to }) => {
            to_status.as_deref().is_none_or(|want| want == to)
        }
        _ => false,
    }
}

pub fn conditions_hold(c: &Conditions, issue: &Issue, now: OffsetDateTime) -> bool {
    (c.issue_types.is_empty() || c.issue_types.contains(&issue.type_key))
        && (c.priorities.is_empty() || c.priorities.contains(&issue.priority))
        && (c.labels_any.is_empty() || c.labels_any.iter().any(|l| issue.labels.contains(l)))
        && (c.statuses.is_empty() || c.statuses.contains(&issue.status))
        && c.untouched_days.is_none_or(|days| issue.updated_at <= now - Duration::days(i64::from(days)))
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
    /// Who a scheduled run acts as: the administrator who last saved the rule.
    pub actor_id: Uuid,
    pub organization_id: Option<Uuid>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

const RULE: &str =
    "id, project_id, name, enabled, definition, version, created_by, actor_id, organization_id, created_at, updated_at";

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
    by: &OrgContext,
) -> ApiResult<Rule> {
    Ok(sqlx::query_as(&format!(
        "INSERT INTO automation_rules (id, tenant_id, project_id, name, enabled, definition, created_by, actor_id, organization_id) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $7, $8) RETURNING {RULE}"
    ))
    .bind(platform_common::new_id())
    .bind(tenant_id)
    .bind(project_id)
    .bind(name)
    .bind(enabled)
    .bind(json!(definition))
    .bind(by.user_id)
    .bind(by.organization_id)
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
    by: &OrgContext,
) -> ApiResult<Rule> {
    Ok(sqlx::query_as(&format!(
        "UPDATE automation_rules SET name = $3, enabled = $4, definition = $5, actor_id = $6, organization_id = $7, \
             version = version + 1, updated_at = now() \
         WHERE tenant_id = $1 AND id = $2 RETURNING {RULE}"
    ))
    .bind(tenant_id)
    .bind(id)
    .bind(name)
    .bind(enabled)
    .bind(json!(definition))
    .bind(by.user_id)
    .bind(by.organization_id)
    .fetch_one(conn)
    .await?)
}

/// Keep a rule's place in the schedule in step with the rule: an enabled scheduled rule has
/// its next run; any other rule has none.
pub async fn sync_schedule(conn: &mut PgConnection, tenant_id: Uuid, rule: &Rule, def: &RuleDefinition) -> ApiResult<()> {
    match (&def.trigger, rule.enabled) {
        (Trigger::Scheduled { frequency, hour, weekday }, true) => {
            let next = next_run(OffsetDateTime::now_utc(), *frequency, *hour, *weekday);
            sqlx::query(
                "INSERT INTO automation_schedule (rule_id, tenant_id, next_run_at) VALUES ($1, $2, $3) \
                 ON CONFLICT (rule_id) DO UPDATE SET next_run_at = EXCLUDED.next_run_at, leased_until = NULL",
            )
            .bind(rule.id)
            .bind(tenant_id)
            .bind(next)
            .execute(conn)
            .await?;
        }
        _ => {
            sqlx::query("DELETE FROM automation_schedule WHERE rule_id = $1").bind(rule.id).execute(conn).await?;
        }
    }
    Ok(())
}

/// When a scheduled rule runs next, if it is scheduled.
pub async fn next_run_at(conn: &mut PgConnection, rule_id: Uuid) -> ApiResult<Option<OffsetDateTime>> {
    Ok(sqlx::query_scalar("SELECT next_run_at FROM automation_schedule WHERE rule_id = $1")
        .bind(rule_id)
        .fetch_optional(conn)
        .await?)
}

pub async fn delete_rule(conn: &mut PgConnection, tenant_id: Uuid, id: Uuid) -> ApiResult<()> {
    sqlx::query("DELETE FROM automation_rules WHERE tenant_id = $1 AND id = $2").bind(tenant_id).bind(id).execute(conn).await?;
    Ok(())
}

#[derive(Debug, Clone, FromRow)]
pub struct Run {
    pub id: Uuid,
    pub issue_id: Option<Uuid>,
    pub issue_key: Option<String>,
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
    issue: Option<&Issue>,
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
    .bind(issue.map(|i| i.id))
    .bind(issue.map(Issue::key))
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

/// Who a rule's changes are made as, and their role in the issue's project.
#[derive(Clone, Copy)]
pub struct Acting<'a> {
    pub ctx: &'a OrgContext,
    pub role: ProjectRole,
}

/// Everything the rules that ran on one issue changed, saved together at the end.
struct Batch {
    next: Issue,
    changes: Changes,
    /// (rule name, document) for each comment to post.
    comments: Vec<(String, Value)>,
}

/// Apply one rule's actions to `out.next`, recording each change against the rule's name.
/// Returns why the rule could not run, if it could not.
async fn apply_actions(
    state: &AppState,
    acting: Acting<'_>,
    tx: &mut TenantTx,
    rule_name: &str,
    actions: &[Action],
    out: &mut Batch,
) -> ApiResult<Result<(), String>> {
    let ctx = acting.ctx;
    let Batch { next: issue, changes, comments } = out;
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
                        return Ok(Err("The person to assign is no longer a member of this workspace.".into()));
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
                    return Ok(Err("The issue would have more than 20 labels.".into()));
                }
                changes.record_by_rule("labels", &from, &issue.labels, rule_name);
            }
            Action::Transition { to_status } => {
                if *to_status == issue.status {
                    continue;
                }
                if !acting.role.allows(ProjectPermission::TransitionIssue) {
                    return Ok(Err("The person this rule acts as cannot move issues in this project.".into()));
                }
                let children_done = store::children_done(tx.conn(), ctx.tenant_id, issue.id).await?;
                let scope = Scope { tenant_id: ctx.tenant_id, project_id: issue.project_id, issue_type: &issue.type_key };
                let evaluation = state
                    .workflows
                    .evaluate(scope, &issue.status, to_status, &facts(ctx, acting.role, issue, children_done))
                    .await;
                let (target, post_functions) = match evaluation {
                    Ok(Evaluation::Allowed { to, post_functions }) => (to, post_functions),
                    Ok(Evaluation::Denied { message }) => return Ok(Err(message)),
                    Err(error) => {
                        tracing::warn!(%error, "workflow check for an automation rule failed");
                        return Ok(Err("The workflow could not be checked. The rule will run again next time.".into()));
                    }
                };
                let before = issue.clone();
                enter_status(issue, &before, &target, post_functions, ctx.user_id);
                let (from_name, _) =
                    status_display(state, ctx, before.project_id, &before.type_key, &before.status, &before.status_category)
                        .await;
                changes.record_by_rule("status", &from_name, &target.name, rule_name);
                changes.record_by_rule("assignee", &before.assignee_id, &issue.assignee_id, rule_name);
                changes.record_by_rule("resolution", &before.resolution, &issue.resolution, rule_name);
            }
            Action::AddComment { body } => {
                if !acting.role.allows(ProjectPermission::Comment) {
                    return Ok(Err("The person this rule acts as cannot comment in this project.".into()));
                }
                let doc = comment_document(body);
                if richtext::validate("body", &doc).is_err() {
                    return Ok(Err("The comment could not be posted.".into()));
                }
                comments.push((rule_name.to_owned(), doc));
            }
        }
    }
    Ok(Ok(()))
}

/// Try one rule against the batch: all of its actions apply, or none do. Every attempt is
/// recorded, except a scheduled run that changed nothing (`quiet`), which would otherwise
/// crowd out the runs that did something.
async fn try_rule(
    state: &AppState,
    acting: Acting<'_>,
    tx: &mut TenantTx,
    rule: &Rule,
    def: &RuleDefinition,
    batch: &mut Batch,
    quiet: bool,
) -> ApiResult<()> {
    let mut attempt = Batch { next: batch.next.clone(), changes: Changes::default(), comments: Vec::new() };
    let tenant_id = acting.ctx.tenant_id;
    let actor = acting.ctx.user_id;
    match apply_actions(state, acting, tx, &rule.name, &def.actions, &mut attempt).await? {
        Ok(()) => {
            let changed = !attempt.changes.is_empty() || !attempt.comments.is_empty();
            if changed || !quiet {
                let outcome = if changed { "applied" } else { "no_change" };
                record_run(tx.conn(), tenant_id, rule.id, Some(&batch.next), actor, outcome, None).await?;
            }
            batch.next = attempt.next;
            batch.changes.extend(attempt.changes);
            batch.comments.extend(attempt.comments);
        }
        Err(why) => record_run(tx.conn(), tenant_id, rule.id, Some(&batch.next), actor, "failed", Some(&why)).await?,
    }
    Ok(())
}

/// Save what the rules changed as one new version with one history entry, post their
/// comments, and announce it all like any other edit. Returns the issue as it now is.
async fn finish(ctx: &OrgContext, tx: &mut TenantTx, issue: Issue, batch: Batch) -> ApiResult<Issue> {
    let Batch { next, changes, comments } = batch;
    if changes.is_empty() && comments.is_empty() {
        return Ok(issue);
    }
    let (issue_id, key, project_id) = (issue.id, issue.key(), issue.project_id);
    let mut version = issue.version;
    if !changes.is_empty() {
        store::save_issue(tx.conn(), ctx.tenant_id, &next).await?;
        version += 1;
        store::add_history(tx.conn(), ctx.tenant_id, issue_id, ctx.user_id, version, &Value::Array(changes.0.clone())).await?;
        let fields = changes.fields();
        emit(tx, ctx, issue_id, version, IssueUpdatedV1 { issue_id, key: key.clone(), project_id, changed_fields: fields })
            .await?;
        if next.status != issue.status {
            let moved = IssueTransitionedV1 {
                issue_id,
                key: key.clone(),
                project_id,
                from_status: issue.status.clone(),
                to_status: next.status.clone(),
                from_category: Some(issue.status_category.clone()),
                to_category: Some(next.status_category.clone()),
            };
            emit(tx, ctx, issue_id, version, moved).await?;
        }
        if next.assignee_id != issue.assignee_id {
            let assigned = IssueAssignedV1 {
                issue_id,
                key: key.clone(),
                project_id,
                assignee_id: next.assignee_id,
                previous_assignee_id: issue.assignee_id,
            };
            emit(tx, ctx, issue_id, version, assigned).await?;
        }
    }
    for (rule_name, doc) in comments {
        let comment = store::insert_comment(tx.conn(), ctx.tenant_id, issue_id, ctx.user_id, &doc, Some(&rule_name)).await?;
        let created = CommentCreatedV1 {
            comment_id: comment.id,
            issue_id,
            key: key.clone(),
            project_id,
            author_id: ctx.user_id,
            mentioned_user_ids: Vec::new(),
        };
        emit(tx, ctx, issue_id, version, created).await?;
    }
    store::issue_by_id(tx.conn(), ctx.tenant_id, issue_id).await?.ok_or(ApiError::NotFound("issue"))
}

fn parsed(rule: &Rule) -> Option<RuleDefinition> {
    let def = serde_json::from_value::<RuleDefinition>(rule.definition.clone()).ok();
    if def.is_none() {
        tracing::warn!(rule_id = %rule.id, "stored automation rule does not parse; skipped");
    }
    def
}

/// Run the project's enabled rules for `event` on `issue` (already saved in `tx`), as the
/// person who made the change, in the order the rules were created; each sees the changes
/// of those before it. Returns the issue as it now is.
pub async fn run(state: &AppState, ctx: &OrgContext, tx: &mut TenantTx, issue: Issue, event: Event<'_>) -> ApiResult<Issue> {
    let rules = rules_for_project(tx.conn(), ctx.tenant_id, issue.project_id, true).await?;
    if rules.is_empty() {
        return Ok(issue);
    }
    let Some(role) = state.policy.snapshot(ctx).await?.project_role(ctx, issue.project_id) else {
        return Ok(issue);
    };
    let acting = Acting { ctx, role };
    let now = OffsetDateTime::now_utc();
    let mut batch = Batch { next: issue.clone(), changes: Changes::default(), comments: Vec::new() };
    for rule in rules {
        let Some(def) = parsed(&rule) else { continue };
        if !triggered(&def.trigger, event) || !conditions_hold(&def.conditions, &batch.next, now) {
            continue;
        }
        try_rule(state, acting, tx, &rule, &def, &mut batch, false).await?;
    }
    finish(ctx, tx, issue, batch).await
}

// --- the schedule ----------------------------------------------------------------

/// The project's issues a scheduled rule applies to now, least recently updated first.
async fn matching_issues(conn: &mut PgConnection, tenant_id: Uuid, project_id: Uuid, c: &Conditions) -> ApiResult<Vec<Uuid>> {
    Ok(sqlx::query_scalar(
        "SELECT id FROM issues WHERE tenant_id = $1 AND project_id = $2 AND deleted_at IS NULL \
           AND (cardinality($3::text[]) = 0 OR type_key = ANY($3)) \
           AND (cardinality($4::text[]) = 0 OR priority = ANY($4)) \
           AND (cardinality($5::text[]) = 0 OR labels && $5) \
           AND (cardinality($6::text[]) = 0 OR status = ANY($6)) \
           AND ($7::int IS NULL OR updated_at <= now() - make_interval(days => $7)) \
         ORDER BY updated_at, id LIMIT $8",
    )
    .bind(tenant_id)
    .bind(project_id)
    .bind(&c.issue_types)
    .bind(&c.priorities)
    .bind(&c.labels_any)
    .bind(&c.statuses)
    .bind(c.untouched_days.map(i32::from))
    .bind(MAX_SCHEDULED_ISSUES)
    .fetch_all(conn)
    .await?)
}

/// What one scheduled run did.
#[derive(Debug, PartialEq, Eq)]
pub enum ScheduledRun {
    /// It looked at this many issues.
    Ran(usize),
    /// It could not act (recorded on the rule, with why).
    Blocked,
    /// The rule was removed, disabled or is no longer scheduled; it left the schedule.
    Gone,
}

/// Run one scheduled rule now.
///
/// The next run is booked and committed first, so a run that fails part-way never repeats
/// changes already made: the issues it did not reach wait for the next run.
pub async fn run_scheduled(state: &AppState, tenant_id: Uuid, rule_id: Uuid) -> ApiResult<ScheduledRun> {
    let mut tx = state.db.begin(tenant_id).await?;
    let rule = rule_by_id(tx.conn(), tenant_id, rule_id, true).await?;
    let def = rule.as_ref().filter(|r| r.enabled).and_then(parsed);
    let (rule, def) = match (rule, def) {
        (Some(rule), Some(def)) if matches!(def.trigger, Trigger::Scheduled { .. }) => (rule, def),
        _ => {
            sqlx::query("DELETE FROM automation_schedule WHERE rule_id = $1").bind(rule_id).execute(tx.conn()).await?;
            tx.commit().await?;
            return Ok(ScheduledRun::Gone);
        }
    };
    sync_schedule(tx.conn(), tenant_id, &rule, &def).await?;
    tx.commit().await?;

    let blocked = |why: &'static str| async move {
        let mut tx = state.db.begin(tenant_id).await?;
        record_run(tx.conn(), tenant_id, rule_id, None, rule.actor_id, "failed", Some(why)).await?;
        tx.commit().await?;
        Ok::<_, ApiError>(ScheduledRun::Blocked)
    };
    // The person it runs as, with their access as it is now.
    let role = state.directory.active_role(tenant_id, rule.actor_id).await?;
    let (Some(role), Some(organization_id)) = (role, rule.organization_id) else {
        return blocked(
            "The person this rule runs as is no longer an active member of the workspace. Save the rule to run it as yourself.",
        )
        .await;
    };
    let ctx = OrgContext {
        user_id: rule.actor_id,
        session_id: Uuid::nil(),
        tenant_id,
        organization_id,
        role,
        email: String::new(),
        name: String::new(),
    };
    let project_role = state.policy.snapshot(&ctx).await?.project_role(&ctx, rule.project_id);
    let Some(project_role) = project_role.filter(|r| r.allows(ProjectPermission::ManageProject)) else {
        return blocked("The person this rule runs as no longer administers this project. Save the rule to run it as yourself.")
            .await;
    };
    let acting = Acting { ctx: &ctx, role: project_role };

    let mut tx = state.db.begin(tenant_id).await?;
    let ids = matching_issues(tx.conn(), tenant_id, rule.project_id, &def.conditions).await?;
    tx.commit().await?;
    let now = OffsetDateTime::now_utc();
    for id in &ids {
        let mut tx = state.db.begin(tenant_id).await?;
        let Some(issue) = store::issue_by_id_for(tx.conn(), tenant_id, *id, true).await? else { continue };
        // It may have changed since it was found.
        if !conditions_hold(&def.conditions, &issue, now) {
            continue;
        }
        let mut batch = Batch { next: issue.clone(), changes: Changes::default(), comments: Vec::new() };
        try_rule(state, acting, &mut tx, &rule, &def, &mut batch, true).await?;
        finish(&ctx, &mut tx, issue, batch).await?;
        tx.commit().await?;
    }
    Ok(ScheduledRun::Ran(ids.len()))
}

/// Due rules claimed per pass.
const BATCH: i64 = 20;
/// How long a claimed rule is held before another runner may take it.
const LEASE_SECONDS: f64 = 600.0;
const IDLE: StdDuration = StdDuration::from_secs(30);

/// Claim the scheduled rules due on this database and run them; returns how many were claimed.
pub async fn run_due(state: &AppState, pool: &PgPool) -> ApiResult<usize> {
    let claimed: Vec<(Uuid, Uuid)> = sqlx::query_as(
        "UPDATE automation_schedule SET leased_until = now() + make_interval(secs => $2) \
         WHERE rule_id IN ( \
           SELECT rule_id FROM automation_schedule \
           WHERE next_run_at <= now() AND (leased_until IS NULL OR leased_until < now()) \
           ORDER BY next_run_at LIMIT $1 FOR UPDATE SKIP LOCKED) \
         RETURNING rule_id, tenant_id",
    )
    .bind(BATCH)
    .bind(LEASE_SECONDS)
    .fetch_all(pool)
    .await?;
    for (rule_id, tenant_id) in &claimed {
        match run_scheduled(state, *tenant_id, *rule_id).await {
            Ok(outcome) => tracing::info!(%tenant_id, %rule_id, ?outcome, "scheduled automation rule ran"),
            // Its lease runs out and it is tried again.
            Err(error) => tracing::warn!(%tenant_id, %rule_id, %error, "scheduled automation rule failed; will retry"),
        }
    }
    Ok(claimed.len())
}

/// One schedule runner per database.
pub fn spawn_scheduler(state: AppState, pools: Vec<PgPool>) {
    for pool in pools {
        let state = state.clone();
        tokio::spawn(async move {
            loop {
                match run_due(&state, &pool).await {
                    Ok(n) if n as i64 == BATCH => continue,
                    Ok(_) => tokio::time::sleep(IDLE).await,
                    Err(error) => {
                        tracing::warn!(%error, "automation schedule pass failed");
                        tokio::time::sleep(IDLE).await;
                    }
                }
            }
        });
    }
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

    #[test]
    fn schedules_comments_and_moves_are_validated() {
        let ok = def(json!({
            "trigger": { "type": "scheduled", "frequency": "weekly", "hour": 9, "weekday": 1 },
            "conditions": { "statuses": ["in_review", " todo", "todo"], "untouched_days": 14 },
            "actions": [{ "type": "transition", "to_status": " done " }, { "type": "add_comment", "body": "  Still needed?\nSay so.  " }]
        }))
        .unwrap();
        assert_eq!(ok.conditions.statuses, ["in_review", "todo"]);
        assert_eq!(ok.actions[0], Action::Transition { to_status: "done".into() });
        assert_eq!(ok.actions[1], Action::AddComment { body: "Still needed?\nSay so.".into() });

        let label = json!({ "type": "add_labels", "labels": ["x"] });
        for bad in [
            json!({ "trigger": { "type": "scheduled", "frequency": "daily", "hour": 9, "weekday": 2 }, "actions": [label] }),
            json!({ "trigger": { "type": "scheduled", "frequency": "weekly", "hour": 9, "weekday": 8 }, "actions": [label] }),
            json!({ "trigger": { "type": "scheduled", "frequency": "hourly", "hour": 9 }, "actions": [label] }),
            json!({ "trigger": { "type": "scheduled", "frequency": "daily", "hour": 9, "cron": "* * * * *" }, "actions": [label] }),
            json!({ "trigger": { "type": "scheduled", "frequency": "daily", "hour": 9 }, "conditions": { "untouched_days": 0 }, "actions": [label] }),
            json!({ "trigger": { "type": "issue_created" }, "conditions": { "statuses": ["Done!"] }, "actions": [label] }),
            json!({ "trigger": { "type": "issue_created" }, "actions": [{ "type": "transition", "to_status": "../done" }] }),
            json!({ "trigger": { "type": "issue_created" }, "actions": [{ "type": "add_comment", "body": "a\u{7}b" }] }),
            json!({ "trigger": { "type": "issue_created" }, "actions": [{ "type": "add_comment", "body": "a" }, { "type": "add_comment", "body": "b" }] }),
        ] {
            assert!(def(bad.clone()).is_err(), "accepted {bad}");
        }
    }

    #[test]
    fn comments_become_plain_documents() {
        let doc = comment_document("First line\n\n<b>not markup</b>");
        assert_eq!(doc["content"].as_array().unwrap().len(), 3);
        assert_eq!(doc["content"][1], json!({ "type": "paragraph" }));
        assert_eq!(doc["content"][2]["content"][0]["text"], "<b>not markup</b>");
        assert!(richtext::validate("body", &doc).is_ok());
    }

    #[test]
    fn scheduled_runs_are_always_ahead() {
        use time::macros::datetime;
        // A Wednesday.
        let now = datetime!(2026-11-18 10:30 UTC);
        assert_eq!(next_run(now, Frequency::Daily, 11, None), datetime!(2026-11-18 11:00 UTC));
        assert_eq!(next_run(now, Frequency::Daily, 10, None), datetime!(2026-11-19 10:00 UTC));
        assert_eq!(next_run(now, Frequency::Weekly, 9, Some(3)), datetime!(2026-11-25 09:00 UTC));
        assert_eq!(next_run(now, Frequency::Weekly, 9, Some(5)), datetime!(2026-11-20 09:00 UTC));
        assert!(triggered(&Trigger::Scheduled { frequency: Frequency::Daily, hour: 1, weekday: None }, Event::Scheduled));
        assert!(!triggered(&Trigger::IssueCreated, Event::Scheduled));
    }
}
