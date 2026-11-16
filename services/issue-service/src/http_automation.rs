//! Managing a project's automation rules. Project administrators only.

use auth_middleware::{OrgContext, ProjectPermission};
use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    routing::get,
    Json, Router,
};
use error_handling::{require_if_match, ApiError, ApiResult};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::{
    automation::{self, Action, Assignee, RuleDefinition, MAX_RULES_PER_PROJECT},
    http::{ensure_assignable, etag},
    AppState,
};

const RULE_NOT_FOUND: ApiError = ApiError::NotFound("automation rule");

#[derive(Debug, Serialize, ToSchema)]
pub struct AutomationRuleDto {
    pub id: Uuid,
    pub project_id: Uuid,
    pub name: String,
    pub enabled: bool,
    pub definition: RuleDefinition,
    pub version: i64,
    pub created_by: Uuid,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AutomationRuleList {
    pub items: Vec<AutomationRuleDto>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateAutomationRuleRequest {
    pub project_key: String,
    pub name: String,
    #[serde(default = "yes")]
    pub enabled: bool,
    pub definition: RuleDefinition,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateAutomationRuleRequest {
    pub name: String,
    pub enabled: bool,
    pub definition: RuleDefinition,
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct RuleQuery {
    /// Project key.
    pub project: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AutomationRunDto {
    pub id: Uuid,
    pub issue_id: Uuid,
    pub issue_key: String,
    pub actor_id: Uuid,
    /// `applied`, `no_change` or `failed`.
    pub outcome: String,
    pub detail: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AutomationRunList {
    pub items: Vec<AutomationRunDto>,
}

fn dto(rule: automation::Rule) -> ApiResult<AutomationRuleDto> {
    let definition = serde_json::from_value(rule.definition).map_err(ApiError::internal)?;
    Ok(AutomationRuleDto {
        id: rule.id,
        project_id: rule.project_id,
        name: rule.name,
        enabled: rule.enabled,
        definition,
        version: rule.version,
        created_by: rule.created_by,
        created_at: rule.created_at,
        updated_at: rule.updated_at,
    })
}

fn rule_name(name: &str) -> ApiResult<String> {
    let name = name.trim();
    if !(1..=100).contains(&name.chars().count()) {
        return Err(ApiError::invalid("name", "length", "Give the rule a name of up to 100 characters."));
    }
    Ok(name.to_owned())
}

/// Validate a definition, including that anyone it assigns to is in the workspace.
async fn checked(state: &AppState, ctx: &OrgContext, def: RuleDefinition) -> ApiResult<RuleDefinition> {
    let def = automation::validate(def)?;
    for action in &def.actions {
        if let Action::Assign { assignee: Assignee::User { user_id } } = action {
            ensure_assignable(state, ctx, *user_id).await.map_err(|_| {
                ApiError::invalid(
                    "definition.actions",
                    "not_a_member",
                    "The person to assign must be a member of this workspace.",
                )
            })?;
        }
    }
    Ok(def)
}

async fn require_admin(state: &AppState, ctx: &OrgContext, project_id: Uuid) -> ApiResult<()> {
    state.policy.snapshot(ctx).await?.require(ctx, project_id, ProjectPermission::ManageProject)?;
    Ok(())
}

/// Load a rule the caller administers; others' rules are indistinguishable from missing ones.
async fn load(
    state: &AppState,
    ctx: &OrgContext,
    tx: &mut tenant_context::TenantTx,
    id: Uuid,
    lock: bool,
) -> ApiResult<automation::Rule> {
    let rule = automation::rule_by_id(tx.conn(), ctx.tenant_id, id, lock).await?.ok_or(RULE_NOT_FOUND)?;
    require_admin(state, ctx, rule.project_id).await.map_err(|_| RULE_NOT_FOUND)?;
    Ok(rule)
}

#[utoipa::path(get, path = "/api/v1/automation-rules", security(("bearer" = [])), params(RuleQuery),
    responses((status = 200, body = AutomationRuleList), (status = 403), (status = 404)))]
pub async fn list(
    State(state): State<AppState>,
    ctx: OrgContext,
    Query(q): Query<RuleQuery>,
) -> ApiResult<Json<AutomationRuleList>> {
    let project = state.directory.project(ctx.tenant_id, &q.project.trim().to_uppercase(), false).await?;
    require_admin(&state, &ctx, project.id).await?;
    let mut tx = state.db.begin(ctx.tenant_id).await?;
    let rules = automation::rules_for_project(tx.conn(), ctx.tenant_id, project.id, false).await?;
    tx.commit().await?;
    Ok(Json(AutomationRuleList { items: rules.into_iter().map(dto).collect::<ApiResult<_>>()? }))
}

#[utoipa::path(post, path = "/api/v1/automation-rules", request_body = CreateAutomationRuleRequest, security(("bearer" = [])),
    responses((status = 201, body = AutomationRuleDto), (status = 403), (status = 404), (status = 409, description = "Too many rules"), (status = 422)))]
pub async fn create(
    State(state): State<AppState>,
    ctx: OrgContext,
    Json(body): Json<CreateAutomationRuleRequest>,
) -> ApiResult<(StatusCode, HeaderMap, Json<AutomationRuleDto>)> {
    let project = state.directory.project(ctx.tenant_id, &body.project_key.trim().to_uppercase(), false).await?;
    require_admin(&state, &ctx, project.id).await?;
    let name = rule_name(&body.name)?;
    let def = checked(&state, &ctx, body.definition).await?;
    let mut tx = state.db.begin(ctx.tenant_id).await?;
    if automation::count_rules(tx.conn(), ctx.tenant_id, project.id).await? >= MAX_RULES_PER_PROJECT {
        return Err(ApiError::Conflict(format!("A project can have at most {MAX_RULES_PER_PROJECT} automation rules.")));
    }
    let rule = automation::insert_rule(tx.conn(), ctx.tenant_id, project.id, &name, body.enabled, &def, ctx.user_id).await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, etag(rule.version), Json(dto(rule)?)))
}

#[utoipa::path(put, path = "/api/v1/automation-rules/{id}", request_body = UpdateAutomationRuleRequest, security(("bearer" = [])),
    params(("id" = Uuid, Path), ("If-Match" = String, Header, description = "Version the edit is based on")),
    responses((status = 200, body = AutomationRuleDto), (status = 404), (status = 409, description = "Edited by someone else"), (status = 422), (status = 428)))]
pub async fn update(
    State(state): State<AppState>,
    ctx: OrgContext,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Json(body): Json<UpdateAutomationRuleRequest>,
) -> ApiResult<(HeaderMap, Json<AutomationRuleDto>)> {
    let expected = require_if_match(&headers)?;
    let name = rule_name(&body.name)?;
    let def = checked(&state, &ctx, body.definition).await?;
    let mut tx = state.db.begin(ctx.tenant_id).await?;
    let current = load(&state, &ctx, &mut tx, id, true).await?;
    if current.version != expected {
        return Err(ApiError::VersionConflict { current: serde_json::to_value(dto(current)?).ok() });
    }
    let rule = automation::update_rule(tx.conn(), ctx.tenant_id, id, &name, body.enabled, &def).await?;
    tx.commit().await?;
    Ok((etag(rule.version), Json(dto(rule)?)))
}

#[utoipa::path(delete, path = "/api/v1/automation-rules/{id}", security(("bearer" = [])), params(("id" = Uuid, Path)),
    responses((status = 204), (status = 404)))]
pub async fn delete(State(state): State<AppState>, ctx: OrgContext, Path(id): Path<Uuid>) -> ApiResult<StatusCode> {
    let mut tx = state.db.begin(ctx.tenant_id).await?;
    load(&state, &ctx, &mut tx, id, true).await?;
    automation::delete_rule(tx.conn(), ctx.tenant_id, id).await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

/// What the rule did recently, newest first (the latest 100).
#[utoipa::path(get, path = "/api/v1/automation-rules/{id}/runs", security(("bearer" = [])), params(("id" = Uuid, Path)),
    responses((status = 200, body = AutomationRunList), (status = 404)))]
pub async fn runs(State(state): State<AppState>, ctx: OrgContext, Path(id): Path<Uuid>) -> ApiResult<Json<AutomationRunList>> {
    let mut tx = state.db.begin(ctx.tenant_id).await?;
    load(&state, &ctx, &mut tx, id, false).await?;
    let rows = automation::runs(tx.conn(), ctx.tenant_id, id).await?;
    tx.commit().await?;
    Ok(Json(AutomationRunList {
        items: rows
            .into_iter()
            .map(|r| AutomationRunDto {
                id: r.id,
                issue_id: r.issue_id,
                issue_key: r.issue_key,
                actor_id: r.actor_id,
                outcome: r.outcome,
                detail: r.detail,
                created_at: r.created_at,
            })
            .collect(),
    }))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/automation-rules", get(list).post(create))
        .route("/automation-rules/{id}", axum::routing::put(update).delete(delete))
        .route("/automation-rules/{id}/runs", get(runs))
}
