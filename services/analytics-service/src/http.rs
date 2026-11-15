//! `/dashboards` (saved dashboards and their widgets) and `/analytics/query` (the numbers
//! for one chart).
//!
//! Who may do what: any member or guest of the workspace can make private dashboards. A
//! shared dashboard is visible to everyone in the workspace; only its owner, or an admin,
//! changes or deletes it. Every chart only counts issues in projects the viewer can browse,
//! so a shared dashboard shows each person their own numbers.

use auth_middleware::{OrgContext, OrgRole, VisibleProjects};
use axum::{
    extract::{Path, State},
    http::{header, HeaderMap, HeaderValue, Method, StatusCode},
    routing::{get, patch, post},
    Json, Router,
};
use error_handling::{require_if_match, ApiError, ApiResult};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgConnection};
use time::OffsetDateTime;
use tower_http::cors::{AllowOrigin, CorsLayer};
use utoipa::{OpenApi, ToSchema};
use uuid::Uuid;

use crate::{
    widgets::{self, GroupBy, WidgetData, WidgetQuery},
    AppState,
};

const MAX_DASHBOARDS: i64 = 200;
const MAX_WIDGETS: i64 = 30;

#[derive(OpenApi)]
#[openapi(
    info(title = "Analytics API", version = "1.0.0", description = "Dashboards and the charts on them."),
    paths(list, create, get_one, update, delete, add_widget, update_widget, delete_widget, query),
    components(schemas(
        DashboardSummary,
        DashboardList,
        Dashboard,
        Widget,
        CreateDashboard,
        UpdateDashboard,
        CreateWidget,
        UpdateWidget,
        WidgetSize,
        WidgetQuery,
        GroupBy,
        WidgetData,
        widgets::DayCounts,
        widgets::WeekCycleTime,
        widgets::Group,
        widgets::DayFlow,
        widgets::StatusTime,
        widgets::ControlPoint,
        widgets::SprintResult
    ))
)]
pub struct ApiDoc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema, sqlx::Type)]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "text", rename_all = "snake_case")]
pub enum WidgetSize {
    Half,
    Full,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct DashboardSummary {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub owner_id: Uuid,
    pub shared: bool,
    /// Whether the caller may change it.
    pub editable: bool,
    pub widget_count: i64,
    /// Bumped by every change to the dashboard or its charts; send it as `If-Match`.
    pub version: i64,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct DashboardList {
    pub items: Vec<DashboardSummary>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct Widget {
    pub id: Uuid,
    pub title: String,
    pub size: WidgetSize,
    pub query: WidgetQuery,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct Dashboard {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub owner_id: Uuid,
    pub shared: bool,
    pub editable: bool,
    pub widgets: Vec<Widget>,
    /// Bumped by every change to the dashboard or its charts; send it as `If-Match`.
    pub version: i64,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateDashboard {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub shared: bool,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateDashboard {
    pub name: Option<String>,
    pub description: Option<String>,
    pub shared: Option<bool>,
    /// Every widget id of the dashboard, in the new order.
    pub widget_order: Option<Vec<Uuid>>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateWidget {
    pub title: String,
    #[serde(default = "half")]
    pub size: WidgetSize,
    pub query: WidgetQuery,
}

fn half() -> WidgetSize {
    WidgetSize::Half
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateWidget {
    pub title: Option<String>,
    pub size: Option<WidgetSize>,
    pub query: Option<WidgetQuery>,
}

#[derive(FromRow)]
struct DashboardRow {
    id: Uuid,
    name: String,
    description: String,
    owner_id: Uuid,
    shared: bool,
    version: i64,
    updated_at: OffsetDateTime,
}

#[derive(FromRow)]
struct SummaryRow {
    id: Uuid,
    name: String,
    description: String,
    owner_id: Uuid,
    shared: bool,
    version: i64,
    updated_at: OffsetDateTime,
    widget_count: i64,
}

#[derive(FromRow)]
struct WidgetRow {
    id: Uuid,
    title: String,
    size: WidgetSize,
    query: sqlx::types::Json<serde_json::Value>,
}

fn can_edit(ctx: &OrgContext, d: &DashboardRow) -> bool {
    d.owner_id == ctx.user_id || (d.shared && ctx.role >= OrgRole::Admin)
}

fn name(field: &str, value: &str, label: &str) -> ApiResult<String> {
    let value = value.trim();
    if value.is_empty() || value.chars().count() > 80 || value.chars().any(char::is_control) {
        return Err(ApiError::invalid(field, "invalid", &format!("{label} must be 1 to 80 characters.")));
    }
    Ok(value.to_owned())
}

fn description(value: &str) -> ApiResult<String> {
    let value = value.trim();
    if value.chars().count() > 500 {
        return Err(ApiError::invalid("description", "too_long", "Keep the description under 500 characters."));
    }
    Ok(value.to_owned())
}

/// The dashboard if the caller may see it (their own, or shared). Others are "not found".
async fn visible_dashboard(conn: &mut PgConnection, ctx: &OrgContext, id: Uuid) -> ApiResult<DashboardRow> {
    sqlx::query_as::<_, DashboardRow>(
        "SELECT id, name, description, owner_id, shared, version, updated_at FROM dashboards \
         WHERE tenant_id = $1 AND id = $2 AND (shared OR owner_id = $3) FOR UPDATE",
    )
    .bind(ctx.tenant_id)
    .bind(id)
    .bind(ctx.user_id)
    .fetch_optional(conn)
    .await?
    .ok_or(ApiError::NotFound("dashboard"))
}

async fn editable_dashboard(conn: &mut PgConnection, ctx: &OrgContext, id: Uuid) -> ApiResult<DashboardRow> {
    let d = visible_dashboard(conn, ctx, id).await?;
    if !can_edit(ctx, &d) {
        return Err(ApiError::Forbidden);
    }
    Ok(d)
}

/// Record a change to the dashboard or one of its charts; returns the new version.
async fn touch(conn: &mut PgConnection, id: Uuid) -> ApiResult<i64> {
    let (version,): (i64,) =
        sqlx::query_as("UPDATE dashboards SET updated_at = now(), version = version + 1 WHERE id = $1 RETURNING version")
            .bind(id)
            .fetch_one(conn)
            .await?;
    Ok(version)
}

fn etag(version: i64) -> HeaderMap {
    let mut headers = HeaderMap::new();
    if let Ok(value) = HeaderValue::from_str(&format!("\"{version}\"")) {
        headers.insert(header::ETAG, value);
    }
    headers
}

/// The dashboard, if the caller may change it and their edit is based on its current version.
/// A stale `If-Match` is a 409 carrying the dashboard as it is now.
async fn writable_dashboard(conn: &mut PgConnection, ctx: &OrgContext, id: Uuid, headers: &HeaderMap) -> ApiResult<DashboardRow> {
    let expected = require_if_match(headers)?;
    let d = editable_dashboard(conn, ctx, id).await?;
    if d.version != expected {
        let current = serde_json::to_value(load(conn, ctx, d).await?).ok();
        return Err(ApiError::VersionConflict { current });
    }
    Ok(d)
}

fn widget(row: WidgetRow) -> Option<Widget> {
    // A stored query that no longer parses (a kind since removed) is left out, not fatal.
    let query = serde_json::from_value(row.query.0).ok()?;
    Some(Widget { id: row.id, title: row.title, size: row.size, query })
}

async fn load(conn: &mut PgConnection, ctx: &OrgContext, d: DashboardRow) -> ApiResult<Dashboard> {
    let rows: Vec<WidgetRow> = sqlx::query_as(
        "SELECT id, title, size, query FROM dashboard_widgets WHERE tenant_id = $1 AND dashboard_id = $2 ORDER BY position, created_at",
    )
    .bind(ctx.tenant_id)
    .bind(d.id)
    .fetch_all(conn)
    .await?;
    Ok(Dashboard {
        editable: can_edit(ctx, &d),
        id: d.id,
        name: d.name,
        description: d.description,
        owner_id: d.owner_id,
        shared: d.shared,
        widgets: rows.into_iter().filter_map(widget).collect(),
        version: d.version,
        updated_at: d.updated_at,
    })
}

#[utoipa::path(get, path = "/api/v1/dashboards", security(("bearer" = [])), responses((status = 200, body = DashboardList)))]
pub async fn list(State(state): State<AppState>, ctx: OrgContext) -> ApiResult<Json<DashboardList>> {
    let mut tx = state.db.begin(ctx.tenant_id).await?;
    let rows: Vec<SummaryRow> = sqlx::query_as(
        "SELECT d.id, d.name, d.description, d.owner_id, d.shared, d.version, d.updated_at, \
                (SELECT count(*) FROM dashboard_widgets w WHERE w.dashboard_id = d.id) AS widget_count \
         FROM dashboards d WHERE d.tenant_id = $1 AND (d.shared OR d.owner_id = $2) \
         ORDER BY (d.owner_id = $2) DESC, d.updated_at DESC LIMIT 500",
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .fetch_all(tx.conn())
    .await?;
    tx.commit().await?;
    let items = rows
        .into_iter()
        .map(|r| {
            let editable = r.owner_id == ctx.user_id || (r.shared && ctx.role >= OrgRole::Admin);
            DashboardSummary {
                id: r.id,
                name: r.name,
                description: r.description,
                owner_id: r.owner_id,
                shared: r.shared,
                editable,
                widget_count: r.widget_count,
                version: r.version,
                updated_at: r.updated_at,
            }
        })
        .collect();
    Ok(Json(DashboardList { items }))
}

#[utoipa::path(post, path = "/api/v1/dashboards", request_body = CreateDashboard, security(("bearer" = [])),
    responses((status = 201, body = Dashboard), (status = 422)))]
pub async fn create(
    State(state): State<AppState>,
    ctx: OrgContext,
    Json(body): Json<CreateDashboard>,
) -> ApiResult<(StatusCode, HeaderMap, Json<Dashboard>)> {
    let name = name("name", &body.name, "Name")?;
    let description = description(&body.description)?;
    let mut tx = state.db.begin(ctx.tenant_id).await?;
    let (owned,): (i64,) = sqlx::query_as("SELECT count(*) FROM dashboards WHERE tenant_id = $1 AND owner_id = $2")
        .bind(ctx.tenant_id)
        .bind(ctx.user_id)
        .fetch_one(tx.conn())
        .await?;
    if owned >= MAX_DASHBOARDS {
        return Err(ApiError::Conflict("You have the most dashboards allowed. Delete one first.".into()));
    }
    let row: DashboardRow = sqlx::query_as(
        "INSERT INTO dashboards (tenant_id, id, name, description, owner_id, shared) VALUES ($1, $2, $3, $4, $5, $6) \
         RETURNING id, name, description, owner_id, shared, version, updated_at",
    )
    .bind(ctx.tenant_id)
    .bind(platform_common::new_id())
    .bind(name)
    .bind(description)
    .bind(ctx.user_id)
    .bind(body.shared)
    .fetch_one(tx.conn())
    .await?;
    let dashboard = load(tx.conn(), &ctx, row).await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, etag(dashboard.version), Json(dashboard)))
}

#[utoipa::path(get, path = "/api/v1/dashboards/{id}", security(("bearer" = [])), params(("id" = Uuid, Path)),
    responses((status = 200, body = Dashboard), (status = 404)))]
pub async fn get_one(
    State(state): State<AppState>,
    ctx: OrgContext,
    Path(id): Path<Uuid>,
) -> ApiResult<(HeaderMap, Json<Dashboard>)> {
    let mut tx = state.db.begin(ctx.tenant_id).await?;
    let row = visible_dashboard(tx.conn(), &ctx, id).await?;
    let dashboard = load(tx.conn(), &ctx, row).await?;
    tx.commit().await?;
    Ok((etag(dashboard.version), Json(dashboard)))
}

#[utoipa::path(patch, path = "/api/v1/dashboards/{id}", request_body = UpdateDashboard, security(("bearer" = [])),
    params(("id" = Uuid, Path), ("If-Match" = String, Header, description = "Version the edit is based on")),
    responses((status = 200, body = Dashboard), (status = 403), (status = 404), (status = 409, description = "Changed by someone else"), (status = 422), (status = 428)))]
pub async fn update(
    State(state): State<AppState>,
    ctx: OrgContext,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Json(body): Json<UpdateDashboard>,
) -> ApiResult<(HeaderMap, Json<Dashboard>)> {
    let mut tx = state.db.begin(ctx.tenant_id).await?;
    let current = writable_dashboard(tx.conn(), &ctx, id, &headers).await?;
    // Only the owner decides who else sees it.
    if body.shared.is_some_and(|s| s != current.shared) && current.owner_id != ctx.user_id {
        return Err(ApiError::Forbidden);
    }
    let new_name = body.name.as_deref().map(|n| name("name", n, "Name")).transpose()?;
    let new_description = body.description.as_deref().map(description).transpose()?;
    sqlx::query(
        "UPDATE dashboards SET name = COALESCE($2, name), description = COALESCE($3, description), \
                shared = COALESCE($4, shared) WHERE id = $1",
    )
    .bind(id)
    .bind(new_name)
    .bind(new_description)
    .bind(body.shared)
    .execute(tx.conn())
    .await?;
    if let Some(order) = body.widget_order {
        let existing: Vec<(Uuid,)> =
            sqlx::query_as("SELECT id FROM dashboard_widgets WHERE dashboard_id = $1").bind(id).fetch_all(tx.conn()).await?;
        let mut have: Vec<Uuid> = existing.into_iter().map(|(w,)| w).collect();
        let mut want = order.clone();
        have.sort();
        want.sort();
        if have != want {
            return Err(ApiError::invalid("widget_order", "mismatch", "List every widget of the dashboard exactly once."));
        }
        for (position, widget_id) in order.iter().enumerate() {
            sqlx::query("UPDATE dashboard_widgets SET position = $3 WHERE dashboard_id = $1 AND id = $2")
                .bind(id)
                .bind(widget_id)
                .bind(position as i32)
                .execute(tx.conn())
                .await?;
        }
    }
    touch(tx.conn(), id).await?;
    let row = visible_dashboard(tx.conn(), &ctx, id).await?;
    let dashboard = load(tx.conn(), &ctx, row).await?;
    tx.commit().await?;
    Ok((etag(dashboard.version), Json(dashboard)))
}

#[utoipa::path(delete, path = "/api/v1/dashboards/{id}", security(("bearer" = [])), params(("id" = Uuid, Path)),
    responses((status = 204), (status = 403), (status = 404)))]
pub async fn delete(State(state): State<AppState>, ctx: OrgContext, Path(id): Path<Uuid>) -> ApiResult<StatusCode> {
    let mut tx = state.db.begin(ctx.tenant_id).await?;
    editable_dashboard(tx.conn(), &ctx, id).await?;
    sqlx::query("DELETE FROM dashboards WHERE id = $1").bind(id).execute(tx.conn()).await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(post, path = "/api/v1/dashboards/{id}/widgets", request_body = CreateWidget, security(("bearer" = [])),
    params(("id" = Uuid, Path)), responses((status = 201, body = Widget), (status = 403), (status = 404), (status = 422)))]
pub async fn add_widget(
    State(state): State<AppState>,
    ctx: OrgContext,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Json(body): Json<CreateWidget>,
) -> ApiResult<(StatusCode, HeaderMap, Json<Widget>)> {
    let title = name("title", &body.title, "Title")?;
    body.query.validate()?;
    let mut tx = state.db.begin(ctx.tenant_id).await?;
    writable_dashboard(tx.conn(), &ctx, id, &headers).await?;
    let (count, next): (i64, i32) =
        sqlx::query_as("SELECT count(*), COALESCE(max(position) + 1, 0) FROM dashboard_widgets WHERE dashboard_id = $1")
            .bind(id)
            .fetch_one(tx.conn())
            .await?;
    if count >= MAX_WIDGETS {
        return Err(ApiError::Conflict("A dashboard holds at most 30 charts.".into()));
    }
    let widget_id = platform_common::new_id();
    sqlx::query(
        "INSERT INTO dashboard_widgets (tenant_id, id, dashboard_id, title, query, size, position) VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(ctx.tenant_id)
    .bind(widget_id)
    .bind(id)
    .bind(&title)
    .bind(sqlx::types::Json(&body.query))
    .bind(body.size)
    .bind(next)
    .execute(tx.conn())
    .await?;
    let version = touch(tx.conn(), id).await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, etag(version), Json(Widget { id: widget_id, title, size: body.size, query: body.query })))
}

#[utoipa::path(patch, path = "/api/v1/dashboards/{id}/widgets/{widget_id}", request_body = UpdateWidget, security(("bearer" = [])),
    params(("id" = Uuid, Path), ("widget_id" = Uuid, Path)), responses((status = 200, body = Widget), (status = 403), (status = 404), (status = 422)))]
pub async fn update_widget(
    State(state): State<AppState>,
    ctx: OrgContext,
    Path((id, widget_id)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
    Json(body): Json<UpdateWidget>,
) -> ApiResult<(HeaderMap, Json<Widget>)> {
    let title = body.title.as_deref().map(|t| name("title", t, "Title")).transpose()?;
    if let Some(q) = &body.query {
        q.validate()?;
    }
    let mut tx = state.db.begin(ctx.tenant_id).await?;
    writable_dashboard(tx.conn(), &ctx, id, &headers).await?;
    let row: Option<WidgetRow> = sqlx::query_as(
        "UPDATE dashboard_widgets SET title = COALESCE($3, title), size = COALESCE($4, size), query = COALESCE($5, query) \
         WHERE dashboard_id = $1 AND id = $2 RETURNING id, title, size, query",
    )
    .bind(id)
    .bind(widget_id)
    .bind(title)
    .bind(body.size)
    .bind(body.query.as_ref().map(sqlx::types::Json))
    .fetch_optional(tx.conn())
    .await?;
    let widget = row.and_then(widget).ok_or(ApiError::NotFound("widget"))?;
    let version = touch(tx.conn(), id).await?;
    tx.commit().await?;
    Ok((etag(version), Json(widget)))
}

#[utoipa::path(delete, path = "/api/v1/dashboards/{id}/widgets/{widget_id}", security(("bearer" = [])),
    params(("id" = Uuid, Path), ("widget_id" = Uuid, Path)), responses((status = 204), (status = 403), (status = 404)))]
pub async fn delete_widget(
    State(state): State<AppState>,
    ctx: OrgContext,
    Path((id, widget_id)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
) -> ApiResult<(StatusCode, HeaderMap)> {
    let mut tx = state.db.begin(ctx.tenant_id).await?;
    writable_dashboard(tx.conn(), &ctx, id, &headers).await?;
    let gone = sqlx::query("DELETE FROM dashboard_widgets WHERE dashboard_id = $1 AND id = $2")
        .bind(id)
        .bind(widget_id)
        .execute(tx.conn())
        .await?;
    if gone.rows_affected() == 0 {
        return Err(ApiError::NotFound("widget"));
    }
    let version = touch(tx.conn(), id).await?;
    tx.commit().await?;
    Ok((StatusCode::NO_CONTENT, etag(version)))
}

#[utoipa::path(post, path = "/api/v1/analytics/query", request_body = WidgetQuery, security(("bearer" = [])),
    responses((status = 200, body = WidgetData), (status = 422), (status = 503, description = "The analytics store is unavailable")))]
pub async fn query(State(state): State<AppState>, ctx: OrgContext, Json(q): Json<WidgetQuery>) -> ApiResult<Json<WidgetData>> {
    let visible = match state.policy.snapshot(&ctx).await?.visible_projects(&ctx) {
        VisibleProjects::All => None,
        VisibleProjects::Only(ids) => Some(ids),
    };
    Ok(Json(widgets::run(&state.reader, ctx.tenant_id, &q, visible.as_deref()).await?))
}

pub fn router(state: AppState) -> Router {
    let api = Router::new()
        .route("/dashboards", get(list).post(create))
        .route("/dashboards/{id}", get(get_one).patch(update).delete(delete))
        .route("/dashboards/{id}/widgets", post(add_widget))
        .route("/dashboards/{id}/widgets/{widget_id}", patch(update_widget).delete(delete_widget))
        .route("/analytics/query", post(query));
    let origins: Vec<_> = state.allowed_origins.iter().filter_map(|o| o.parse().ok()).collect();
    let cors = CorsLayer::new()
        .allow_origin(AllowOrigin::list(origins))
        .allow_methods([Method::GET, Method::POST, Method::PATCH, Method::DELETE])
        .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE, header::IF_MATCH])
        .expose_headers([header::ETAG]);
    Router::new()
        .nest("/api/v1", api)
        .route("/openapi.json", get(|| async { Json(ApiDoc::openapi()) }))
        .layer(cors)
        .with_state(state)
}
