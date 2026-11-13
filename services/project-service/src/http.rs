//! Public REST API for projects.

use auth_middleware::{OrgContext, OrgPermission, OrgRole, ProjectPermission, ProjectRole};
use axum::{
    extract::{Path, Query, State},
    http::{
        header::{self, ETAG},
        HeaderMap, HeaderValue, Method, StatusCode,
    },
    routing::{delete, get, post, put},
    Json, Router,
};
use error_handling::{require_if_match, ApiError, ApiResult};
use event_contracts::{
    outbox,
    payloads::{AuditRecordedV1, ProjectCreatedV1, ProjectDeletedV1, ProjectUpdatedV1, VersionReleasedV1},
    Actor, EventEnvelope,
};
use serde::{Deserialize, Serialize};
use tenant_context::TenantTx;
use time::{Date, OffsetDateTime};
use tower_http::cors::{AllowOrigin, CorsLayer};
use utoipa::{OpenApi, ToSchema};
use uuid::Uuid;
use validator::Validate;

use crate::{
    domain::{validate_key, validate_type},
    store::{self, Component, NewProject, Project, ProjectPatch, Version},
    AppState,
};

time::serde::format_description!(iso_date, Date, "[year]-[month]-[day]");

// --- DTOs --------------------------------------------------------------------

#[derive(Debug, Serialize, ToSchema)]
pub struct ProjectDto {
    pub id: Uuid,
    pub key: String,
    pub name: String,
    pub description: String,
    /// scrum | kanban | general
    pub project_type: String,
    pub lead_user_id: Option<Uuid>,
    /// active | archived
    pub status: String,
    /// Role every organization member holds (null = private project).
    pub default_role: Option<String>,
    /// The caller's effective role: administrator | member | viewer
    pub my_role: String,
    pub version: i64,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

impl ProjectDto {
    fn new(p: Project, role: ProjectRole) -> Self {
        Self {
            id: p.id,
            key: p.key,
            name: p.name,
            description: p.description,
            project_type: p.project_type,
            lead_user_id: p.lead_user_id,
            status: p.status,
            default_role: p.default_role,
            my_role: role.as_str().to_owned(),
            version: p.version,
            created_at: p.created_at,
            updated_at: p.updated_at,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ProjectList {
    pub items: Vec<ProjectDto>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct ProjectListQuery {
    /// active (default) | archived
    pub status: Option<String>,
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateProjectRequest {
    #[validate(length(min = 2, max = 80, message = "Name must be between 2 and 80 characters."))]
    pub name: String,
    pub key: String,
    #[validate(length(max = 2000, message = "Description must be 2000 characters or fewer."))]
    #[serde(default)]
    pub description: String,
    /// scrum | kanban | general
    pub project_type: String,
    /// open: every organization member can work in it. private: only people added to it.
    pub access: String,
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateProjectRequest {
    #[validate(length(min = 2, max = 80, message = "Name must be between 2 and 80 characters."))]
    pub name: Option<String>,
    #[validate(length(max = 2000, message = "Description must be 2000 characters or fewer."))]
    pub description: Option<String>,
    pub project_type: Option<String>,
    /// Send `null` to clear the lead; omit to leave it unchanged.
    #[serde(default, deserialize_with = "double_option")]
    #[schema(value_type = Option<Uuid>)]
    pub lead_user_id: Option<Option<Uuid>>,
}

fn double_option<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Option<Uuid>>, D::Error> {
    Option::<Uuid>::deserialize(d).map(Some)
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ComponentDto {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub lead_user_id: Option<Uuid>,
}

impl From<Component> for ComponentDto {
    fn from(c: Component) -> Self {
        Self { id: c.id, name: c.name, description: c.description, lead_user_id: c.lead_user_id }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ComponentList {
    pub items: Vec<ComponentDto>,
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateComponentRequest {
    #[validate(length(min = 1, max = 80, message = "Name must be between 1 and 80 characters."))]
    pub name: String,
    #[validate(length(max = 500, message = "Description must be 500 characters or fewer."))]
    #[serde(default)]
    pub description: String,
    pub lead_user_id: Option<Uuid>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct VersionDto {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    #[serde(with = "iso_date::option")]
    #[schema(value_type = Option<String>, format = Date)]
    pub start_date: Option<Date>,
    #[serde(with = "iso_date::option")]
    #[schema(value_type = Option<String>, format = Date)]
    pub release_date: Option<Date>,
    /// unreleased | released | archived
    pub status: String,
    #[serde(with = "time::serde::rfc3339::option")]
    pub released_at: Option<OffsetDateTime>,
}

impl From<Version> for VersionDto {
    fn from(v: Version) -> Self {
        Self {
            id: v.id,
            name: v.name,
            description: v.description,
            start_date: v.start_date,
            release_date: v.release_date,
            status: v.status,
            released_at: v.released_at,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct VersionList {
    pub items: Vec<VersionDto>,
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateVersionRequest {
    #[validate(length(min = 1, max = 80, message = "Name must be between 1 and 80 characters."))]
    pub name: String,
    #[validate(length(max = 500, message = "Description must be 500 characters or fewer."))]
    #[serde(default)]
    pub description: String,
    #[serde(default, with = "iso_date::option")]
    #[schema(value_type = Option<String>, format = Date)]
    pub start_date: Option<Date>,
    #[serde(default, with = "iso_date::option")]
    #[schema(value_type = Option<String>, format = Date)]
    pub release_date: Option<Date>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ProjectMemberDto {
    pub user_id: Uuid,
    /// administrator | member | viewer
    pub role: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ProjectAccessDto {
    pub members: Vec<ProjectMemberDto>,
    /// Role held by every organization member; null when the project is private.
    pub default_role: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SetMemberRequest {
    pub role: String,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SetAccessRequest {
    /// member | viewer, or null to make the project private.
    pub default_role: Option<String>,
}

// --- helpers -----------------------------------------------------------------

fn etag(version: i64) -> HeaderMap {
    let mut headers = HeaderMap::new();
    if let Ok(v) = HeaderValue::from_str(&format!("\"{version}\"")) {
        headers.insert(ETAG, v);
    }
    headers
}

fn parse_role(value: &str) -> ApiResult<ProjectRole> {
    ProjectRole::parse(value)
        .ok_or_else(|| ApiError::invalid("role", "unknown_role", "Role must be administrator, member or viewer."))
}

/// Open a tenant transaction, load the project and check the caller's permission on it.
/// A project the caller cannot see is indistinguishable from one that does not exist.
async fn open(
    state: &AppState,
    ctx: &OrgContext,
    key: &str,
    permission: ProjectPermission,
) -> ApiResult<(TenantTx, Project, ProjectRole)> {
    let snapshot = state.policy.snapshot(ctx).await?;
    let mut tx = state.db.begin(ctx.tenant_id).await?;
    let project = store::project_by_key(tx.conn(), ctx.tenant_id, key).await?.ok_or(ApiError::NotFound("project"))?;
    let role = snapshot.require(ctx, project.id, permission)?;
    Ok((tx, project, role))
}

async fn emit_updated(tx: &mut TenantTx, ctx: &OrgContext, p: &Project) -> ApiResult<()> {
    outbox::enqueue(
        tx.conn(),
        &EventEnvelope::new(
            Some(ctx.tenant_id),
            p.id,
            p.version,
            Actor::User(ctx.user_id),
            ProjectUpdatedV1 { project_id: p.id, key: p.key.clone(), name: p.name.clone(), status: p.status.clone() },
        ),
    )
    .await?;
    Ok(())
}

/// Record an access change for the audit log. The permission service applies the change
/// but does not know who asked, so the record is written here, after it succeeded.
async fn audit_access(
    tx: &mut TenantTx,
    ctx: &OrgContext,
    project: &Project,
    action: &str,
    details: &[(&str, String)],
) -> ApiResult<()> {
    outbox::enqueue(
        tx.conn(),
        &EventEnvelope::new(
            Some(ctx.tenant_id),
            project.id,
            0,
            Actor::User(ctx.user_id),
            AuditRecordedV1 {
                action: action.to_owned(),
                resource_type: "project".to_owned(),
                resource_id: project.id.to_string(),
                resource_label: project.key.clone(),
                details: details.iter().map(|(k, v)| ((*k).to_owned(), v.clone())).collect(),
            },
        ),
    )
    .await?;
    Ok(())
}

fn unique(what: &'static str) -> impl Fn(sqlx::Error) -> ApiError {
    move |err| match &err {
        sqlx::Error::Database(db) if db.is_unique_violation() => {
            ApiError::Conflict(format!("A {what} with that name already exists."))
        }
        _ => err.into(),
    }
}

// --- projects ----------------------------------------------------------------

#[utoipa::path(get, path = "/api/v1/projects", security(("bearer" = [])), params(("status" = Option<String>, Query)),
    responses((status = 200, body = ProjectList)))]
pub async fn list(
    State(state): State<AppState>,
    ctx: OrgContext,
    Query(q): Query<ProjectListQuery>,
) -> ApiResult<Json<ProjectList>> {
    let status = match q.status.as_deref() {
        None | Some("active") => "active",
        Some("archived") => "archived",
        Some(_) => return Err(ApiError::invalid("status", "unknown_status", "Status must be active or archived.")),
    };
    let snapshot = state.policy.snapshot(&ctx).await?;
    let visible = match snapshot.visible_projects(&ctx) {
        auth_middleware::policy::VisibleProjects::All => None,
        auth_middleware::policy::VisibleProjects::Only(ids) => Some(ids),
    };
    let mut tx = state.db.begin(ctx.tenant_id).await?;
    let rows = store::list_projects(tx.conn(), ctx.tenant_id, status, visible.as_deref()).await?;
    tx.commit().await?;
    let items = rows.into_iter().filter_map(|p| snapshot.project_role(&ctx, p.id).map(|role| ProjectDto::new(p, role))).collect();
    Ok(Json(ProjectList { items }))
}

#[utoipa::path(post, path = "/api/v1/projects", request_body = CreateProjectRequest, security(("bearer" = [])),
    responses((status = 201, body = ProjectDto), (status = 409, description = "Key already used"), (status = 422)))]
pub async fn create(
    State(state): State<AppState>,
    ctx: OrgContext,
    Json(mut body): Json<CreateProjectRequest>,
) -> ApiResult<(StatusCode, HeaderMap, Json<ProjectDto>)> {
    // Guests work in projects they are invited to; they do not create them.
    if ctx.role < OrgRole::Member {
        return Err(ApiError::Forbidden);
    }
    body.name = body.name.trim().to_owned();
    body.key = body.key.trim().to_uppercase();
    body.validate()?;
    validate_key(&body.key)?;
    validate_type(&body.project_type)?;
    let default_role = match body.access.as_str() {
        "open" => Some(ProjectRole::Member),
        "private" => None,
        _ => return Err(ApiError::invalid("access", "unknown_access", "Access must be open or private.")),
    };

    let id = platform_common::new_id();
    let mut tx = state.db.begin(ctx.tenant_id).await?;
    let project = store::insert_project(
        tx.conn(),
        ctx.tenant_id,
        &NewProject {
            id,
            key: &body.key,
            name: &body.name,
            description: &body.description,
            project_type: &body.project_type,
            default_role: default_role.map(ProjectRole::as_str),
            created_by: ctx.user_id,
        },
    )
    .await
    .map_err(|err| match &err {
        sqlx::Error::Database(db) if db.is_unique_violation() => {
            ApiError::Conflict(format!("The key {} is already used by another project.", body.key))
        }
        _ => err.into(),
    })?;
    outbox::enqueue(
        tx.conn(),
        &EventEnvelope::new(
            Some(ctx.tenant_id),
            id,
            project.version,
            Actor::User(ctx.user_id),
            ProjectCreatedV1 {
                project_id: id,
                key: project.key.clone(),
                name: project.name.clone(),
                project_type: project.project_type.clone(),
                created_by: ctx.user_id,
            },
        ),
    )
    .await?;

    // Grant access before committing: if the permission service is down the project is
    // not created at all. If the commit then fails, the bindings are compensated.
    let granted = async {
        state.access.set_role(ctx.tenant_id, id, ctx.user_id, ProjectRole::Administrator).await?;
        if let Some(role) = default_role {
            state.access.set_default_role(ctx.tenant_id, id, Some(role)).await?;
        }
        Ok::<_, ApiError>(())
    }
    .await;
    let committed = match granted {
        Ok(()) => tx.commit().await,
        Err(err) => Err(err),
    };
    if let Err(err) = committed {
        if let Err(cleanup) = state.access.delete_project(ctx.tenant_id, id).await {
            tracing::error!(project_id = %id, error = %cleanup, "could not remove bindings of a project that failed to create");
        }
        return Err(err);
    }
    state.policy.forget(ctx.tenant_id, ctx.user_id);
    tracing::info!(project_id = %id, key = %project.key, "project created");
    Ok((StatusCode::CREATED, etag(project.version), Json(ProjectDto::new(project, ProjectRole::Administrator))))
}

#[utoipa::path(get, path = "/api/v1/projects/{key}", security(("bearer" = [])), params(("key" = String, Path)),
    responses((status = 200, body = ProjectDto), (status = 404)))]
pub async fn get_one(
    State(state): State<AppState>,
    ctx: OrgContext,
    Path(key): Path<String>,
) -> ApiResult<(HeaderMap, Json<ProjectDto>)> {
    let (tx, project, role) = open(&state, &ctx, &key, ProjectPermission::Browse).await?;
    tx.commit().await?;
    Ok((etag(project.version), Json(ProjectDto::new(project, role))))
}

#[utoipa::path(patch, path = "/api/v1/projects/{key}", request_body = UpdateProjectRequest, security(("bearer" = [])),
    params(("key" = String, Path), ("If-Match" = String, Header)),
    responses((status = 200, body = ProjectDto), (status = 409), (status = 428)))]
pub async fn update(
    State(state): State<AppState>,
    ctx: OrgContext,
    Path(key): Path<String>,
    headers: HeaderMap,
    Json(mut body): Json<UpdateProjectRequest>,
) -> ApiResult<(HeaderMap, Json<ProjectDto>)> {
    let expected = require_if_match(&headers)?;
    body.name = body.name.map(|n| n.trim().to_owned());
    body.validate()?;
    if let Some(t) = &body.project_type {
        validate_type(t)?;
    }
    let (mut tx, project, role) = open(&state, &ctx, &key, ProjectPermission::ManageProject).await?;
    if let Some(Some(lead)) = body.lead_user_id {
        if !state.access.is_org_member(lead, ctx.organization_id).await? {
            return Err(ApiError::invalid(
                "lead_user_id",
                "not_a_member",
                "The project lead must be a member of this workspace.",
            ));
        }
    }
    let patch = ProjectPatch {
        name: body.name.as_deref(),
        description: body.description.as_deref(),
        project_type: body.project_type.as_deref(),
        lead_user_id: body.lead_user_id,
    };
    let Some(updated) = store::update_project(tx.conn(), ctx.tenant_id, project.id, expected, &patch).await? else {
        return Err(ApiError::VersionConflict { current: serde_json::to_value(ProjectDto::new(project, role)).ok() });
    };
    emit_updated(&mut tx, &ctx, &updated).await?;
    tx.commit().await?;
    Ok((etag(updated.version), Json(ProjectDto::new(updated, role))))
}

async fn change_status(
    state: AppState,
    ctx: OrgContext,
    key: String,
    status: &'static str,
) -> ApiResult<(HeaderMap, Json<ProjectDto>)> {
    let (mut tx, project, role) = open(&state, &ctx, &key, ProjectPermission::ManageProject).await?;
    if project.status == status {
        return Ok((etag(project.version), Json(ProjectDto::new(project, role))));
    }
    let updated = store::set_status(tx.conn(), ctx.tenant_id, project.id, status).await?;
    emit_updated(&mut tx, &ctx, &updated).await?;
    tx.commit().await?;
    Ok((etag(updated.version), Json(ProjectDto::new(updated, role))))
}

#[utoipa::path(post, path = "/api/v1/projects/{key}/archive", security(("bearer" = [])), params(("key" = String, Path)),
    responses((status = 200, body = ProjectDto)))]
pub async fn archive(
    State(state): State<AppState>,
    ctx: OrgContext,
    Path(key): Path<String>,
) -> ApiResult<(HeaderMap, Json<ProjectDto>)> {
    change_status(state, ctx, key, "archived").await
}

#[utoipa::path(post, path = "/api/v1/projects/{key}/restore", security(("bearer" = [])), params(("key" = String, Path)),
    responses((status = 200, body = ProjectDto)))]
pub async fn restore(
    State(state): State<AppState>,
    ctx: OrgContext,
    Path(key): Path<String>,
) -> ApiResult<(HeaderMap, Json<ProjectDto>)> {
    change_status(state, ctx, key, "active").await
}

#[utoipa::path(delete, path = "/api/v1/projects/{key}", security(("bearer" = [])), params(("key" = String, Path)),
    responses((status = 204), (status = 403), (status = 409, description = "Archive the project first")))]
pub async fn remove(State(state): State<AppState>, ctx: OrgContext, Path(key): Path<String>) -> ApiResult<StatusCode> {
    // Deleting is an organization-level decision, and deliberately two steps.
    ctx.require(OrgPermission::ManageOrganization)?;
    let (mut tx, project, _) = open(&state, &ctx, &key, ProjectPermission::ManageProject).await?;
    if project.status != "archived" {
        return Err(ApiError::Conflict("Archive the project before deleting it.".into()));
    }
    store::soft_delete(tx.conn(), ctx.tenant_id, project.id).await?;
    outbox::enqueue(
        tx.conn(),
        &EventEnvelope::new(
            Some(ctx.tenant_id),
            project.id,
            project.version + 1,
            Actor::User(ctx.user_id),
            ProjectDeletedV1 { project_id: project.id, key: project.key.clone() },
        ),
    )
    .await?;
    tx.commit().await?;
    if let Err(err) = state.access.delete_project(ctx.tenant_id, project.id).await {
        // The project is gone for users either way; stale bindings grant nothing.
        tracing::warn!(project_id = %project.id, error = %err, "could not remove bindings of deleted project");
    }
    state.policy.forget_tenant(ctx.tenant_id);
    Ok(StatusCode::NO_CONTENT)
}

// --- components --------------------------------------------------------------

#[utoipa::path(get, path = "/api/v1/projects/{key}/components", security(("bearer" = [])), params(("key" = String, Path)),
    responses((status = 200, body = ComponentList)))]
pub async fn list_components(
    State(state): State<AppState>,
    ctx: OrgContext,
    Path(key): Path<String>,
) -> ApiResult<Json<ComponentList>> {
    let (mut tx, project, _) = open(&state, &ctx, &key, ProjectPermission::Browse).await?;
    let rows = store::components(tx.conn(), ctx.tenant_id, project.id).await?;
    tx.commit().await?;
    Ok(Json(ComponentList { items: rows.into_iter().map(Into::into).collect() }))
}

#[utoipa::path(post, path = "/api/v1/projects/{key}/components", request_body = CreateComponentRequest, security(("bearer" = [])),
    params(("key" = String, Path)), responses((status = 201, body = ComponentDto), (status = 409)))]
pub async fn create_component(
    State(state): State<AppState>,
    ctx: OrgContext,
    Path(key): Path<String>,
    Json(mut body): Json<CreateComponentRequest>,
) -> ApiResult<(StatusCode, Json<ComponentDto>)> {
    body.name = body.name.trim().to_owned();
    body.validate()?;
    let (mut tx, project, _) = open(&state, &ctx, &key, ProjectPermission::ManageProject).await?;
    let row = store::insert_component(tx.conn(), ctx.tenant_id, project.id, &body.name, &body.description, body.lead_user_id)
        .await
        .map_err(unique("component"))?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(row.into())))
}

#[utoipa::path(delete, path = "/api/v1/projects/{key}/components/{id}", security(("bearer" = [])),
    params(("key" = String, Path), ("id" = Uuid, Path)), responses((status = 204), (status = 404)))]
pub async fn delete_component(
    State(state): State<AppState>,
    ctx: OrgContext,
    Path((key, id)): Path<(String, Uuid)>,
) -> ApiResult<StatusCode> {
    let (mut tx, project, _) = open(&state, &ctx, &key, ProjectPermission::ManageProject).await?;
    let done = store::delete_component(tx.conn(), ctx.tenant_id, project.id, id).await?;
    tx.commit().await?;
    if done {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound("component"))
    }
}

// --- versions ----------------------------------------------------------------

#[utoipa::path(get, path = "/api/v1/projects/{key}/versions", security(("bearer" = [])), params(("key" = String, Path)),
    responses((status = 200, body = VersionList)))]
pub async fn list_versions(
    State(state): State<AppState>,
    ctx: OrgContext,
    Path(key): Path<String>,
) -> ApiResult<Json<VersionList>> {
    let (mut tx, project, _) = open(&state, &ctx, &key, ProjectPermission::Browse).await?;
    let rows = store::versions(tx.conn(), ctx.tenant_id, project.id).await?;
    tx.commit().await?;
    Ok(Json(VersionList { items: rows.into_iter().map(Into::into).collect() }))
}

#[utoipa::path(post, path = "/api/v1/projects/{key}/versions", request_body = CreateVersionRequest, security(("bearer" = [])),
    params(("key" = String, Path)), responses((status = 201, body = VersionDto), (status = 409), (status = 422)))]
pub async fn create_version(
    State(state): State<AppState>,
    ctx: OrgContext,
    Path(key): Path<String>,
    Json(mut body): Json<CreateVersionRequest>,
) -> ApiResult<(StatusCode, Json<VersionDto>)> {
    body.name = body.name.trim().to_owned();
    body.validate()?;
    if let (Some(start), Some(release)) = (body.start_date, body.release_date) {
        if release < start {
            return Err(ApiError::invalid("release_date", "before_start", "The release date cannot be before the start date."));
        }
    }
    let (mut tx, project, _) = open(&state, &ctx, &key, ProjectPermission::ManageProject).await?;
    let row = store::insert_version(
        tx.conn(),
        ctx.tenant_id,
        project.id,
        &body.name,
        &body.description,
        body.start_date,
        body.release_date,
    )
    .await
    .map_err(unique("version"))?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(row.into())))
}

async fn set_released(state: AppState, ctx: OrgContext, key: String, id: Uuid, released: bool) -> ApiResult<Json<VersionDto>> {
    let (mut tx, project, _) = open(&state, &ctx, &key, ProjectPermission::ManageProject).await?;
    let version = store::set_version_released(tx.conn(), ctx.tenant_id, project.id, id, released)
        .await?
        .ok_or(ApiError::NotFound("version"))?;
    if released {
        outbox::enqueue(
            tx.conn(),
            &EventEnvelope::new(
                Some(ctx.tenant_id),
                project.id,
                project.version,
                Actor::User(ctx.user_id),
                VersionReleasedV1 { project_id: project.id, version_id: version.id, name: version.name.clone() },
            ),
        )
        .await?;
    }
    tx.commit().await?;
    Ok(Json(version.into()))
}

#[utoipa::path(post, path = "/api/v1/projects/{key}/versions/{id}/release", security(("bearer" = [])),
    params(("key" = String, Path), ("id" = Uuid, Path)), responses((status = 200, body = VersionDto)))]
pub async fn release_version(
    State(state): State<AppState>,
    ctx: OrgContext,
    Path((key, id)): Path<(String, Uuid)>,
) -> ApiResult<Json<VersionDto>> {
    set_released(state, ctx, key, id, true).await
}

#[utoipa::path(post, path = "/api/v1/projects/{key}/versions/{id}/unrelease", security(("bearer" = [])),
    params(("key" = String, Path), ("id" = Uuid, Path)), responses((status = 200, body = VersionDto)))]
pub async fn unrelease_version(
    State(state): State<AppState>,
    ctx: OrgContext,
    Path((key, id)): Path<(String, Uuid)>,
) -> ApiResult<Json<VersionDto>> {
    set_released(state, ctx, key, id, false).await
}

#[utoipa::path(delete, path = "/api/v1/projects/{key}/versions/{id}", security(("bearer" = [])),
    params(("key" = String, Path), ("id" = Uuid, Path)), responses((status = 204), (status = 404)))]
pub async fn delete_version(
    State(state): State<AppState>,
    ctx: OrgContext,
    Path((key, id)): Path<(String, Uuid)>,
) -> ApiResult<StatusCode> {
    let (mut tx, project, _) = open(&state, &ctx, &key, ProjectPermission::ManageProject).await?;
    let done = store::delete_version(tx.conn(), ctx.tenant_id, project.id, id).await?;
    tx.commit().await?;
    if done {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound("version"))
    }
}

// --- access ------------------------------------------------------------------

#[utoipa::path(get, path = "/api/v1/projects/{key}/members", security(("bearer" = [])), params(("key" = String, Path)),
    responses((status = 200, body = ProjectAccessDto)))]
pub async fn list_members(
    State(state): State<AppState>,
    ctx: OrgContext,
    Path(key): Path<String>,
) -> ApiResult<Json<ProjectAccessDto>> {
    let (tx, project, _) = open(&state, &ctx, &key, ProjectPermission::Browse).await?;
    tx.commit().await?;
    let list = state.access.members(ctx.tenant_id, project.id).await?;
    Ok(Json(ProjectAccessDto {
        members: list
            .members
            .into_iter()
            .map(|(user_id, role)| ProjectMemberDto { user_id, role: role.as_str().to_owned() })
            .collect(),
        default_role: list.default_role.map(|r| r.as_str().to_owned()),
    }))
}

#[utoipa::path(put, path = "/api/v1/projects/{key}/members/{user_id}", request_body = SetMemberRequest, security(("bearer" = [])),
    params(("key" = String, Path), ("user_id" = Uuid, Path)), responses((status = 204), (status = 422)))]
pub async fn set_member(
    State(state): State<AppState>,
    ctx: OrgContext,
    Path((key, user_id)): Path<(String, Uuid)>,
    Json(body): Json<SetMemberRequest>,
) -> ApiResult<StatusCode> {
    let role = parse_role(&body.role)?;
    let (tx, project, _) = open(&state, &ctx, &key, ProjectPermission::ManageProject).await?;
    tx.commit().await?;
    // Only people in this organization can be given access to its projects.
    if !state.access.is_org_member(user_id, ctx.organization_id).await? {
        return Err(ApiError::invalid("user_id", "not_a_member", "That person is not a member of this workspace."));
    }
    state.access.set_role(ctx.tenant_id, project.id, user_id, role).await?;
    state.policy.forget(ctx.tenant_id, user_id);
    let mut tx = state.db.begin(ctx.tenant_id).await?;
    audit_access(
        &mut tx,
        &ctx,
        &project,
        "project.access_granted",
        &[("user_id", user_id.to_string()), ("role", role.as_str().to_owned())],
    )
    .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(delete, path = "/api/v1/projects/{key}/members/{user_id}", security(("bearer" = [])),
    params(("key" = String, Path), ("user_id" = Uuid, Path)), responses((status = 204)))]
pub async fn remove_member(
    State(state): State<AppState>,
    ctx: OrgContext,
    Path((key, user_id)): Path<(String, Uuid)>,
) -> ApiResult<StatusCode> {
    let (tx, project, _) = open(&state, &ctx, &key, ProjectPermission::ManageProject).await?;
    tx.commit().await?;
    state.access.remove_role(ctx.tenant_id, project.id, user_id).await?;
    state.policy.forget(ctx.tenant_id, user_id);
    let mut tx = state.db.begin(ctx.tenant_id).await?;
    audit_access(&mut tx, &ctx, &project, "project.access_removed", &[("user_id", user_id.to_string())]).await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(put, path = "/api/v1/projects/{key}/access", request_body = SetAccessRequest, security(("bearer" = [])),
    params(("key" = String, Path)), responses((status = 200, body = ProjectDto)))]
pub async fn set_access(
    State(state): State<AppState>,
    ctx: OrgContext,
    Path(key): Path<String>,
    Json(body): Json<SetAccessRequest>,
) -> ApiResult<Json<ProjectDto>> {
    let default_role = body.default_role.as_deref().map(parse_role).transpose()?;
    if default_role == Some(ProjectRole::Administrator) {
        return Err(ApiError::invalid("default_role", "too_permissive", "Everyone cannot be a project administrator."));
    }
    let (mut tx, project, role) = open(&state, &ctx, &key, ProjectPermission::ManageProject).await?;
    state.access.set_default_role(ctx.tenant_id, project.id, default_role).await?;
    let updated = store::set_default_role(tx.conn(), ctx.tenant_id, project.id, default_role.map(ProjectRole::as_str)).await?;
    emit_updated(&mut tx, &ctx, &updated).await?;
    let everyone = default_role.map_or("none", ProjectRole::as_str).to_owned();
    audit_access(&mut tx, &ctx, &updated, "project.default_access_changed", &[("default_role", everyone)]).await?;
    tx.commit().await?;
    state.policy.forget_tenant(ctx.tenant_id);
    Ok(Json(ProjectDto::new(updated, role)))
}

// --- router ------------------------------------------------------------------

#[derive(OpenApi)]
#[openapi(
    info(title = "Projects API", version = "1.0.0", description = "Projects, components, versions and project access."),
    paths(
        list,
        create,
        get_one,
        update,
        archive,
        restore,
        remove,
        list_components,
        create_component,
        delete_component,
        list_versions,
        create_version,
        release_version,
        unrelease_version,
        delete_version,
        list_members,
        set_member,
        remove_member,
        set_access
    ),
    components(schemas(
        ProjectDto,
        ProjectList,
        CreateProjectRequest,
        UpdateProjectRequest,
        ComponentDto,
        ComponentList,
        CreateComponentRequest,
        VersionDto,
        VersionList,
        CreateVersionRequest,
        ProjectMemberDto,
        ProjectAccessDto,
        SetMemberRequest,
        SetAccessRequest
    ))
)]
pub struct ApiDoc;

pub fn router(state: AppState) -> Router {
    let api = Router::new()
        .route("/projects", get(list).post(create))
        .route("/projects/{key}", get(get_one).patch(update).delete(remove))
        .route("/projects/{key}/archive", post(archive))
        .route("/projects/{key}/restore", post(restore))
        .route("/projects/{key}/components", get(list_components).post(create_component))
        .route("/projects/{key}/components/{id}", delete(delete_component))
        .route("/projects/{key}/versions", get(list_versions).post(create_version))
        .route("/projects/{key}/versions/{id}", delete(delete_version))
        .route("/projects/{key}/versions/{id}/release", post(release_version))
        .route("/projects/{key}/versions/{id}/unrelease", post(unrelease_version))
        .route("/projects/{key}/members", get(list_members))
        .route("/projects/{key}/members/{user_id}", put(set_member).delete(remove_member))
        .route("/projects/{key}/access", put(set_access));

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
