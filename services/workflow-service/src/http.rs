//! Public REST API: designing, publishing and assigning workflows.

use auth_middleware::{OrgContext, OrgPermission, ProjectPermission};
use axum::{
    extract::{Path, Query, State},
    http::{
        header::{self, ETAG},
        HeaderMap, HeaderValue, Method, StatusCode,
    },
    routing::{get, post, put},
    Json, Router,
};
use error_handling::{require_if_match, ApiError, ApiResult, FieldError};
use event_contracts::{
    outbox,
    payloads::{WorkflowPublishedV1, WorkflowUpdatedV1},
    Actor, EventEnvelope,
};
use serde::{Deserialize, Serialize};
use tenant_context::TenantTx;
use time::OffsetDateTime;
use tower_http::cors::{AllowOrigin, CorsLayer};
use utoipa::{OpenApi, ToSchema};
use uuid::Uuid;

use crate::{
    engine::{default_definition, Condition, Definition, PostFunction, Problem, StatusDef, Transition, Validator},
    resolve, store, AppState,
};

const ISSUE_TYPES: [&str; 7] = ["epic", "story", "task", "bug", "feature", "improvement", "subtask"];
const NOT_FOUND: ApiError = ApiError::NotFound("workflow");

#[derive(Debug, Serialize, ToSchema)]
pub struct WorkflowSummary {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    /// Latest published version, if any.
    pub published_version: Option<i32>,
    pub has_draft: bool,
    /// Row version; send it back in `If-Match` when saving the draft.
    pub version: i64,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

impl From<&store::Workflow> for WorkflowSummary {
    fn from(w: &store::Workflow) -> Self {
        Self {
            id: w.id,
            name: w.name.clone(),
            description: w.description.clone(),
            published_version: w.published_version,
            has_draft: w.draft.is_some(),
            version: w.version,
            updated_at: w.updated_at,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct WorkflowList {
    pub items: Vec<WorkflowSummary>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct WorkflowDetail {
    #[serde(flatten)]
    pub summary: WorkflowSummary,
    /// Unpublished edits, if any.
    pub draft: Option<Definition>,
    /// The definition currently governing issues, if published.
    pub published: Option<Definition>,
    /// Why the draft (or, without one, nothing) cannot be published yet.
    pub problems: Vec<Problem>,
    /// How many project / issue-type assignments use this workflow.
    pub assignment_count: i64,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateWorkflowRequest {
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// Start from a copy of this workflow; omitted = the default workflow.
    pub copy_from: Option<Uuid>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RenameWorkflowRequest {
    pub name: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SaveDraftRequest {
    pub definition: Definition,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct VersionDto {
    pub version: i32,
    pub published_by: Uuid,
    #[serde(with = "time::serde::rfc3339")]
    pub published_at: OffsetDateTime,
    pub status_count: usize,
    pub transition_count: usize,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct VersionDetail {
    pub version: i32,
    pub published_by: Uuid,
    #[serde(with = "time::serde::rfc3339")]
    pub published_at: OffsetDateTime,
    pub definition: Definition,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct VersionList {
    pub items: Vec<VersionDto>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AssignmentDto {
    /// null = applies to every issue type without its own assignment
    pub issue_type: Option<String>,
    pub workflow_id: Uuid,
    pub workflow_name: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AssignmentList {
    pub items: Vec<AssignmentDto>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct AssignmentQuery {
    /// Omit for the workspace-wide defaults.
    pub project_id: Option<Uuid>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AssignRequest {
    /// Omit for a workspace-wide default.
    pub project_id: Option<Uuid>,
    /// Omit to cover every issue type.
    pub issue_type: Option<String>,
    /// null removes the assignment (falling back to the next more general one).
    pub workflow_id: Option<Uuid>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct StatusQuery {
    pub project_id: Uuid,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct StatusDto {
    pub key: String,
    pub name: String,
    pub category: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct StatusList {
    pub items: Vec<StatusDto>,
}

fn etag(version: i64) -> HeaderMap {
    let mut headers = HeaderMap::new();
    if let Ok(v) = HeaderValue::from_str(&format!("\"{version}\"")) {
        headers.insert(ETAG, v);
    }
    headers
}

fn validate_name(name: &str) -> ApiResult<String> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 80 {
        return Err(ApiError::invalid("name", "invalid_name", "The name must be between 1 and 80 characters."));
    }
    Ok(name.to_owned())
}

fn duplicate_name(err: sqlx::Error) -> ApiError {
    match &err {
        sqlx::Error::Database(db) if db.is_unique_violation() => {
            ApiError::Conflict("A workflow with that name already exists.".into())
        }
        _ => err.into(),
    }
}

fn parse(value: &serde_json::Value) -> ApiResult<Definition> {
    serde_json::from_value(value.clone())
        .map_err(|e| ApiError::internal(anyhow::anyhow!("stored workflow definition is invalid: {e}")))
}

async fn emit_updated(tx: &mut TenantTx, ctx: &OrgContext, workflow: &store::Workflow, change: &str) -> ApiResult<()> {
    outbox::enqueue(
        tx.conn(),
        &EventEnvelope::new(
            Some(ctx.tenant_id),
            workflow.id,
            workflow.version,
            Actor::User(ctx.user_id),
            WorkflowUpdatedV1 { workflow_id: workflow.id, change: change.to_owned() },
        ),
    )
    .await?;
    Ok(())
}

async fn detail(tx: &mut TenantTx, ctx: &OrgContext, workflow: store::Workflow) -> ApiResult<WorkflowDetail> {
    let draft = workflow.draft.as_ref().map(parse).transpose()?;
    let published = match workflow.published_version {
        Some(v) => {
            store::version(tx.conn(), ctx.tenant_id, workflow.id, v).await?.map(|row| parse(&row.definition)).transpose()?
        }
        None => None,
    };
    let problems = draft.as_ref().map(Definition::problems).unwrap_or_default();
    let assignment_count = store::assignment_count(tx.conn(), ctx.tenant_id, workflow.id).await?;
    Ok(WorkflowDetail { summary: WorkflowSummary::from(&workflow), draft, published, problems, assignment_count })
}

// --- workflows ---------------------------------------------------------------

/// Readable by everyone in the organization: project administrators choose from this list.
#[utoipa::path(get, path = "/api/v1/workflows", security(("bearer" = [])), responses((status = 200, body = WorkflowList)))]
pub async fn list(State(state): State<AppState>, ctx: OrgContext) -> ApiResult<Json<WorkflowList>> {
    let mut tx = state.db.begin(ctx.tenant_id).await?;
    let rows = store::list(tx.conn(), ctx.tenant_id).await?;
    tx.commit().await?;
    Ok(Json(WorkflowList { items: rows.iter().map(WorkflowSummary::from).collect() }))
}

#[utoipa::path(post, path = "/api/v1/workflows", request_body = CreateWorkflowRequest, security(("bearer" = [])),
    responses((status = 201, body = WorkflowDetail), (status = 403), (status = 409), (status = 422)))]
pub async fn create(
    State(state): State<AppState>,
    ctx: OrgContext,
    Json(body): Json<CreateWorkflowRequest>,
) -> ApiResult<(StatusCode, HeaderMap, Json<WorkflowDetail>)> {
    ctx.require(OrgPermission::ManageOrganization)?;
    let name = validate_name(&body.name)?;
    if body.description.chars().count() > 500 {
        return Err(ApiError::invalid("description", "too_long", "The description must be 500 characters or fewer."));
    }
    let mut tx = state.db.begin(ctx.tenant_id).await?;
    let definition = match body.copy_from {
        None => default_definition(),
        Some(source) => {
            let source = store::get(tx.conn(), ctx.tenant_id, source, false)
                .await?
                .ok_or_else(|| ApiError::invalid("copy_from", "unknown_workflow", "The workflow to copy was not found."))?;
            // Copy what the source's author is looking at: the draft if there is one.
            match (&source.draft, source.published_version) {
                (Some(draft), _) => parse(draft)?,
                (None, Some(v)) => {
                    parse(&store::version(tx.conn(), ctx.tenant_id, source.id, v).await?.ok_or(NOT_FOUND)?.definition)?
                }
                (None, None) => default_definition(),
            }
        }
    };
    let draft = serde_json::to_value(&definition).map_err(ApiError::internal)?;
    let workflow =
        store::insert(tx.conn(), ctx.tenant_id, platform_common::new_id(), &name, body.description.trim(), &draft, ctx.user_id)
            .await
            .map_err(duplicate_name)?;
    emit_updated(&mut tx, &ctx, &workflow, "created").await?;
    let version = workflow.version;
    let out = detail(&mut tx, &ctx, workflow).await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, etag(version), Json(out)))
}

#[utoipa::path(get, path = "/api/v1/workflows/{id}", security(("bearer" = [])), params(("id" = Uuid, Path)),
    responses((status = 200, body = WorkflowDetail), (status = 404)))]
pub async fn get_one(
    State(state): State<AppState>,
    ctx: OrgContext,
    Path(id): Path<Uuid>,
) -> ApiResult<(HeaderMap, Json<WorkflowDetail>)> {
    let mut tx = state.db.begin(ctx.tenant_id).await?;
    let workflow = store::get(tx.conn(), ctx.tenant_id, id, false).await?.ok_or(NOT_FOUND)?;
    let version = workflow.version;
    let out = detail(&mut tx, &ctx, workflow).await?;
    tx.commit().await?;
    Ok((etag(version), Json(out)))
}

#[utoipa::path(patch, path = "/api/v1/workflows/{id}", request_body = RenameWorkflowRequest, security(("bearer" = [])), params(("id" = Uuid, Path)),
    responses((status = 200, body = WorkflowDetail), (status = 403), (status = 404), (status = 409)))]
pub async fn rename(
    State(state): State<AppState>,
    ctx: OrgContext,
    Path(id): Path<Uuid>,
    Json(body): Json<RenameWorkflowRequest>,
) -> ApiResult<(HeaderMap, Json<WorkflowDetail>)> {
    ctx.require(OrgPermission::ManageOrganization)?;
    let name = validate_name(&body.name)?;
    let mut tx = state.db.begin(ctx.tenant_id).await?;
    let workflow = store::rename(tx.conn(), ctx.tenant_id, id, &name, body.description.trim())
        .await
        .map_err(duplicate_name)?
        .ok_or(NOT_FOUND)?;
    emit_updated(&mut tx, &ctx, &workflow, "renamed").await?;
    let version = workflow.version;
    let out = detail(&mut tx, &ctx, workflow).await?;
    tx.commit().await?;
    Ok((etag(version), Json(out)))
}

/// Save work in progress. A draft may be incomplete; the response lists what still
/// prevents publishing. Drafts never affect issues.
#[utoipa::path(put, path = "/api/v1/workflows/{id}/draft", request_body = SaveDraftRequest, security(("bearer" = [])),
    params(("id" = Uuid, Path), ("If-Match" = String, Header)),
    responses((status = 200, body = WorkflowDetail), (status = 403), (status = 409, description = "Edited by someone else"), (status = 428)))]
pub async fn save_draft(
    State(state): State<AppState>,
    ctx: OrgContext,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Json(body): Json<SaveDraftRequest>,
) -> ApiResult<(HeaderMap, Json<WorkflowDetail>)> {
    ctx.require(OrgPermission::ManageOrganization)?;
    let expected = require_if_match(&headers)?;
    let draft = serde_json::to_value(&body.definition).map_err(ApiError::internal)?;
    if draft.to_string().len() > 300_000 {
        return Err(ApiError::invalid("definition", "too_large", "The workflow is too large."));
    }
    let mut tx = state.db.begin(ctx.tenant_id).await?;
    let Some(workflow) = store::set_draft(tx.conn(), ctx.tenant_id, id, expected, Some(&draft)).await? else {
        let current = store::get(tx.conn(), ctx.tenant_id, id, false).await?.ok_or(NOT_FOUND)?;
        let current = detail(&mut tx, &ctx, current).await?;
        return Err(ApiError::VersionConflict { current: serde_json::to_value(current).ok() });
    };
    let version = workflow.version;
    let out = detail(&mut tx, &ctx, workflow).await?;
    tx.commit().await?;
    Ok((etag(version), Json(out)))
}

#[utoipa::path(delete, path = "/api/v1/workflows/{id}/draft", security(("bearer" = [])), params(("id" = Uuid, Path), ("If-Match" = String, Header)),
    responses((status = 200, body = WorkflowDetail), (status = 409), (status = 428)))]
pub async fn discard_draft(
    State(state): State<AppState>,
    ctx: OrgContext,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> ApiResult<(HeaderMap, Json<WorkflowDetail>)> {
    ctx.require(OrgPermission::ManageOrganization)?;
    let expected = require_if_match(&headers)?;
    let mut tx = state.db.begin(ctx.tenant_id).await?;
    let current = store::get(tx.conn(), ctx.tenant_id, id, true).await?.ok_or(NOT_FOUND)?;
    if current.published_version.is_none() {
        return Err(ApiError::Conflict(
            "This workflow has never been published, so its draft is all there is. Delete the workflow instead.".into(),
        ));
    }
    let Some(workflow) = store::set_draft(tx.conn(), ctx.tenant_id, id, expected, None).await? else {
        let current = detail(&mut tx, &ctx, current).await?;
        return Err(ApiError::VersionConflict { current: serde_json::to_value(current).ok() });
    };
    let version = workflow.version;
    let out = detail(&mut tx, &ctx, workflow).await?;
    tx.commit().await?;
    Ok((etag(version), Json(out)))
}

/// Make the draft the definition that governs issues. Creates a new immutable version.
#[utoipa::path(post, path = "/api/v1/workflows/{id}/publish", security(("bearer" = [])), params(("id" = Uuid, Path), ("If-Match" = String, Header)),
    responses((status = 200, body = WorkflowDetail), (status = 409), (status = 422, description = "The draft has problems"), (status = 428)))]
pub async fn publish(
    State(state): State<AppState>,
    ctx: OrgContext,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> ApiResult<(HeaderMap, Json<WorkflowDetail>)> {
    ctx.require(OrgPermission::ManageOrganization)?;
    let expected = require_if_match(&headers)?;
    let mut tx = state.db.begin(ctx.tenant_id).await?;
    let workflow = store::get(tx.conn(), ctx.tenant_id, id, true).await?.ok_or(NOT_FOUND)?;
    if workflow.version != expected {
        let current = detail(&mut tx, &ctx, workflow).await?;
        return Err(ApiError::VersionConflict { current: serde_json::to_value(current).ok() });
    }
    let Some(draft) = &workflow.draft else {
        return Err(ApiError::Conflict("There are no unpublished changes.".into()));
    };
    let definition = parse(draft)?;
    let problems = definition.problems();
    if !problems.is_empty() {
        return Err(ApiError::Validation(
            problems.into_iter().map(|p| FieldError::new(p.path, "invalid_workflow", p.message)).collect(),
        ));
    }
    let published = store::publish(tx.conn(), ctx.tenant_id, &workflow, draft, ctx.user_id).await?;
    let version_number = published.published_version.unwrap_or(1);
    outbox::enqueue(
        tx.conn(),
        &EventEnvelope::new(
            Some(ctx.tenant_id),
            published.id,
            published.version,
            Actor::User(ctx.user_id),
            WorkflowPublishedV1 { workflow_id: published.id, name: published.name.clone(), version: version_number },
        ),
    )
    .await?;
    let version = published.version;
    let out = detail(&mut tx, &ctx, published).await?;
    tx.commit().await?;
    tracing::info!(workflow_id = %id, version = version_number, "workflow published");
    Ok((etag(version), Json(out)))
}

#[utoipa::path(get, path = "/api/v1/workflows/{id}/versions", security(("bearer" = [])), params(("id" = Uuid, Path)),
    responses((status = 200, body = VersionList)))]
pub async fn versions(State(state): State<AppState>, ctx: OrgContext, Path(id): Path<Uuid>) -> ApiResult<Json<VersionList>> {
    let mut tx = state.db.begin(ctx.tenant_id).await?;
    store::get(tx.conn(), ctx.tenant_id, id, false).await?.ok_or(NOT_FOUND)?;
    let rows = store::versions(tx.conn(), ctx.tenant_id, id).await?;
    tx.commit().await?;
    let items = rows
        .into_iter()
        .map(|row| {
            let definition = parse(&row.definition)?;
            Ok(VersionDto {
                version: row.version,
                published_by: row.published_by,
                published_at: row.published_at,
                status_count: definition.statuses.len(),
                transition_count: definition.transitions.len(),
            })
        })
        .collect::<ApiResult<Vec<_>>>()?;
    Ok(Json(VersionList { items }))
}

/// One published version as it was, for looking back before restoring it.
#[utoipa::path(get, path = "/api/v1/workflows/{id}/versions/{version}", security(("bearer" = [])),
    params(("id" = Uuid, Path), ("version" = i32, Path)), responses((status = 200, body = VersionDetail), (status = 404)))]
pub async fn get_version(
    State(state): State<AppState>,
    ctx: OrgContext,
    Path((id, number)): Path<(Uuid, i32)>,
) -> ApiResult<Json<VersionDetail>> {
    let mut tx = state.db.begin(ctx.tenant_id).await?;
    store::get(tx.conn(), ctx.tenant_id, id, false).await?.ok_or(NOT_FOUND)?;
    let row = store::version(tx.conn(), ctx.tenant_id, id, number).await?.ok_or(ApiError::NotFound("version"))?;
    tx.commit().await?;
    Ok(Json(VersionDetail {
        version: row.version,
        published_by: row.published_by,
        published_at: row.published_at,
        definition: parse(&row.definition)?,
    }))
}

/// Make an earlier version the draft, replacing any unpublished changes. Nothing changes for
/// issues until the draft is published, which checks it like any other draft.
#[utoipa::path(post, path = "/api/v1/workflows/{id}/versions/{version}/restore", security(("bearer" = [])),
    params(("id" = Uuid, Path), ("version" = i32, Path), ("If-Match" = String, Header)),
    responses((status = 200, body = WorkflowDetail), (status = 403), (status = 404), (status = 409), (status = 428)))]
pub async fn restore_version(
    State(state): State<AppState>,
    ctx: OrgContext,
    Path((id, number)): Path<(Uuid, i32)>,
    headers: HeaderMap,
) -> ApiResult<(HeaderMap, Json<WorkflowDetail>)> {
    ctx.require(OrgPermission::ManageOrganization)?;
    let expected = require_if_match(&headers)?;
    let mut tx = state.db.begin(ctx.tenant_id).await?;
    let current = store::get(tx.conn(), ctx.tenant_id, id, true).await?.ok_or(NOT_FOUND)?;
    let row = store::version(tx.conn(), ctx.tenant_id, id, number).await?.ok_or(ApiError::NotFound("version"))?;
    if current.version == expected && current.draft.is_none() && current.published_version == Some(number) {
        return Err(ApiError::Conflict(format!("Version {number} is already the published version.")));
    }
    let Some(workflow) = store::set_draft(tx.conn(), ctx.tenant_id, id, expected, Some(&row.definition)).await? else {
        let current = detail(&mut tx, &ctx, current).await?;
        return Err(ApiError::VersionConflict { current: serde_json::to_value(current).ok() });
    };
    let version = workflow.version;
    let out = detail(&mut tx, &ctx, workflow).await?;
    tx.commit().await?;
    tracing::info!(workflow_id = %id, version = number, "earlier workflow version restored as draft");
    Ok((etag(version), Json(out)))
}

#[utoipa::path(delete, path = "/api/v1/workflows/{id}", security(("bearer" = [])), params(("id" = Uuid, Path)),
    responses((status = 204), (status = 403), (status = 404), (status = 409, description = "Still assigned")))]
pub async fn remove(State(state): State<AppState>, ctx: OrgContext, Path(id): Path<Uuid>) -> ApiResult<StatusCode> {
    ctx.require(OrgPermission::ManageOrganization)?;
    let mut tx = state.db.begin(ctx.tenant_id).await?;
    let workflow = store::get(tx.conn(), ctx.tenant_id, id, true).await?.ok_or(NOT_FOUND)?;
    if store::assignment_count(tx.conn(), ctx.tenant_id, id).await? > 0 {
        return Err(ApiError::Conflict("This workflow is in use. Assign another workflow to its projects first.".into()));
    }
    store::soft_delete(tx.conn(), ctx.tenant_id, id).await?;
    emit_updated(&mut tx, &ctx, &workflow, "deleted").await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

// --- assignments -------------------------------------------------------------

/// Project assignments are managed by that project's administrators; workspace-wide
/// defaults by organization admins.
async fn require_scope_admin(state: &AppState, ctx: &OrgContext, project_id: Option<Uuid>) -> ApiResult<()> {
    match project_id {
        None => ctx.require(OrgPermission::ManageOrganization),
        Some(project) => state.policy.snapshot(ctx).await?.require(ctx, project, ProjectPermission::ManageProject).map(|_| ()),
    }
}

#[utoipa::path(get, path = "/api/v1/workflow-assignments", security(("bearer" = [])), params(("project_id" = Option<Uuid>, Query)),
    responses((status = 200, body = AssignmentList)))]
pub async fn list_assignments(
    State(state): State<AppState>,
    ctx: OrgContext,
    Query(q): Query<AssignmentQuery>,
) -> ApiResult<Json<AssignmentList>> {
    if let Some(project) = q.project_id {
        state.policy.snapshot(&ctx).await?.require(&ctx, project, ProjectPermission::Browse)?;
    }
    let mut tx = state.db.begin(ctx.tenant_id).await?;
    let rows = store::assignments(tx.conn(), ctx.tenant_id, q.project_id).await?;
    tx.commit().await?;
    Ok(Json(AssignmentList {
        items: rows
            .into_iter()
            .map(|a| AssignmentDto { issue_type: a.issue_type, workflow_id: a.workflow_id, workflow_name: a.workflow_name })
            .collect(),
    }))
}

#[utoipa::path(put, path = "/api/v1/workflow-assignments", request_body = AssignRequest, security(("bearer" = [])),
    responses((status = 204), (status = 403), (status = 422)))]
pub async fn assign(State(state): State<AppState>, ctx: OrgContext, Json(body): Json<AssignRequest>) -> ApiResult<StatusCode> {
    require_scope_admin(&state, &ctx, body.project_id).await?;
    if let Some(issue_type) = &body.issue_type {
        if !ISSUE_TYPES.contains(&issue_type.as_str()) {
            return Err(ApiError::invalid("issue_type", "unknown_type", "Unknown issue type."));
        }
    }
    let mut tx = state.db.begin(ctx.tenant_id).await?;
    match body.workflow_id {
        Some(workflow_id) => {
            let workflow = store::get(tx.conn(), ctx.tenant_id, workflow_id, false)
                .await?
                .ok_or_else(|| ApiError::invalid("workflow_id", "unknown_workflow", "That workflow was not found."))?;
            if workflow.published_version.is_none() {
                return Err(ApiError::invalid("workflow_id", "not_published", "Publish the workflow before using it."));
            }
            store::assign(tx.conn(), ctx.tenant_id, body.project_id, body.issue_type.as_deref(), workflow_id, ctx.user_id)
                .await?;
            emit_updated(&mut tx, &ctx, &workflow, "assigned").await?;
        }
        None => {
            store::unassign(tx.conn(), ctx.tenant_id, body.project_id, body.issue_type.as_deref()).await?;
        }
    }
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Every status an issue of the project can be in (for filters).
#[utoipa::path(get, path = "/api/v1/workflow-statuses", security(("bearer" = [])), params(("project_id" = Uuid, Query)),
    responses((status = 200, body = StatusList)))]
pub async fn statuses(
    State(state): State<AppState>,
    ctx: OrgContext,
    Query(q): Query<StatusQuery>,
) -> ApiResult<Json<StatusList>> {
    state.policy.snapshot(&ctx).await?.require(&ctx, q.project_id, ProjectPermission::Browse)?;
    let mut tx = state.db.begin(ctx.tenant_id).await?;
    let items = resolve::project_statuses(tx.conn(), ctx.tenant_id, q.project_id).await?;
    tx.commit().await?;
    Ok(Json(StatusList {
        items: items.into_iter().map(|s| StatusDto { key: s.key, name: s.name, category: s.category }).collect(),
    }))
}

#[derive(OpenApi)]
#[openapi(
    info(title = "Workflows API", version = "1.0.0", description = "Workflow definitions, publishing and assignment."),
    paths(
        list,
        create,
        get_one,
        rename,
        save_draft,
        discard_draft,
        publish,
        versions,
        get_version,
        restore_version,
        remove,
        list_assignments,
        assign,
        statuses
    ),
    components(schemas(
        WorkflowSummary,
        WorkflowList,
        WorkflowDetail,
        CreateWorkflowRequest,
        RenameWorkflowRequest,
        SaveDraftRequest,
        VersionDto,
        VersionList,
        VersionDetail,
        AssignmentDto,
        AssignmentList,
        AssignRequest,
        StatusDto,
        StatusList,
        Definition,
        StatusDef,
        Transition,
        Condition,
        Validator,
        PostFunction,
        Problem
    ))
)]
pub struct ApiDoc;

pub fn router(state: AppState) -> Router {
    let api = Router::new()
        .route("/workflows", get(list).post(create))
        .route("/workflows/{id}", get(get_one).patch(rename).delete(remove))
        .route("/workflows/{id}/draft", put(save_draft).delete(discard_draft))
        .route("/workflows/{id}/publish", post(publish))
        .route("/workflows/{id}/versions", get(versions))
        .route("/workflows/{id}/versions/{version}", get(get_version))
        .route("/workflows/{id}/versions/{version}/restore", post(restore_version))
        .route("/workflow-assignments", get(list_assignments).put(assign))
        .route("/workflow-statuses", get(statuses));

    let origins: Vec<_> = state.allowed_origins.iter().filter_map(|o| o.parse().ok()).collect();
    let cors = CorsLayer::new()
        .allow_origin(AllowOrigin::list(origins))
        .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE, Method::PATCH])
        .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE, header::IF_MATCH])
        .expose_headers([header::ETAG]);

    Router::new()
        .nest("/api/v1", api)
        .route("/openapi.json", get(|| async { Json(ApiDoc::openapi()) }))
        .layer(cors)
        .with_state(state)
}
