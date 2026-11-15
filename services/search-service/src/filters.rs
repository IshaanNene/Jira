//! `/api/v1/search/filters`: saved MQL queries.
//!
//! A filter stores only the query text. It is run like any typed query, with the viewer's own
//! access, so sharing a filter never shares issues the viewer could not already see. The owner
//! edits it; a shared filter can also be edited or removed by a workspace admin.

use std::cmp::Reverse;

use auth_middleware::{OrgContext, OrgRole};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use error_handling::{ApiError, ApiResult, FieldError};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgConnection};
use time::OffsetDateTime;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{mql, AppState};

/// Filters one person can own in a workspace.
const MAX_FILTERS: i64 = 100;
/// Shared filters listed, newest first.
const MAX_SHARED: i64 = 500;
const COLUMNS: &str = "id, owner_id, name, query, shared, updated_at";

#[derive(Debug, FromRow)]
struct Row {
    id: Uuid,
    owner_id: Uuid,
    name: String,
    query: String,
    shared: bool,
    updated_at: OffsetDateTime,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SavedFilter {
    pub id: Uuid,
    pub name: String,
    /// MQL, as typed.
    pub query: String,
    pub owner_id: Uuid,
    /// Every member of the workspace can run it.
    pub shared: bool,
    /// The caller may rename, change or delete it.
    pub editable: bool,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SavedFilterList {
    /// The caller's own filters first, then filters others shared; each group newest first.
    pub items: Vec<SavedFilter>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateSavedFilter {
    pub name: String,
    pub query: String,
    #[serde(default)]
    pub shared: bool,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateSavedFilter {
    pub name: Option<String>,
    pub query: Option<String>,
    /// Only the owner can change this.
    pub shared: Option<bool>,
}

fn can_edit(ctx: &OrgContext, r: &Row) -> bool {
    r.owner_id == ctx.user_id || (r.shared && ctx.role >= OrgRole::Admin)
}

fn dto(ctx: &OrgContext, r: Row) -> SavedFilter {
    SavedFilter {
        editable: can_edit(ctx, &r),
        id: r.id,
        name: r.name,
        query: r.query,
        owner_id: r.owner_id,
        shared: r.shared,
        updated_at: r.updated_at,
    }
}

fn name(value: &str) -> ApiResult<String> {
    let value = value.trim();
    if value.is_empty() || value.chars().count() > 80 || value.chars().any(char::is_control) {
        return Err(ApiError::invalid("name", "invalid", "Name the filter (up to 80 characters)."));
    }
    Ok(value.to_owned())
}

/// Saved only if it parses; names of people and projects are resolved when it runs.
fn query(value: &str) -> ApiResult<String> {
    let value = value.trim();
    if value.is_empty() {
        return Err(ApiError::invalid("query", "required", "Type a query to save."));
    }
    mql::parse(value)
        .map_err(|e| ApiError::Validation(vec![FieldError::new("query", "invalid_query", e.message).at(e.start, e.end)]))?;
    Ok(value.to_owned())
}

fn duplicate(err: sqlx::Error, name: &str) -> ApiError {
    match &err {
        sqlx::Error::Database(db) if db.is_unique_violation() => {
            ApiError::invalid("name", "taken", &format!("You already have a filter called \"{name}\"."))
        }
        _ => err.into(),
    }
}

async fn visible(conn: &mut PgConnection, ctx: &OrgContext, id: Uuid) -> ApiResult<Row> {
    sqlx::query_as(&format!("SELECT {COLUMNS} FROM saved_filters WHERE tenant_id = $1 AND id = $2 AND (shared OR owner_id = $3)"))
        .bind(ctx.tenant_id)
        .bind(id)
        .bind(ctx.user_id)
        .fetch_optional(conn)
        .await?
        .ok_or(ApiError::NotFound("filter"))
}

async fn editable(conn: &mut PgConnection, ctx: &OrgContext, id: Uuid) -> ApiResult<Row> {
    let row = visible(conn, ctx, id).await?;
    if !can_edit(ctx, &row) {
        return Err(ApiError::Forbidden);
    }
    Ok(row)
}

#[utoipa::path(get, path = "/api/v1/search/filters", security(("bearer" = [])), responses((status = 200, body = SavedFilterList)))]
pub async fn list(State(state): State<AppState>, ctx: OrgContext) -> ApiResult<Json<SavedFilterList>> {
    let mut tx = state.db.begin(ctx.tenant_id).await?;
    let rows: Vec<Row> = sqlx::query_as(&format!(
        "(SELECT {COLUMNS} FROM saved_filters WHERE tenant_id = $1 AND owner_id = $2 ORDER BY updated_at DESC) \
         UNION ALL \
         (SELECT {COLUMNS} FROM saved_filters WHERE tenant_id = $1 AND shared AND owner_id <> $2 \
          ORDER BY updated_at DESC LIMIT $3)"
    ))
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .bind(MAX_SHARED)
    .fetch_all(tx.conn())
    .await?;
    tx.commit().await?;
    let (mut own, mut others): (Vec<Row>, Vec<Row>) = rows.into_iter().partition(|r| r.owner_id == ctx.user_id);
    own.sort_by_key(|r| Reverse(r.updated_at));
    others.sort_by_key(|r| Reverse(r.updated_at));
    Ok(Json(SavedFilterList { items: own.into_iter().chain(others).map(|r| dto(&ctx, r)).collect() }))
}

#[utoipa::path(post, path = "/api/v1/search/filters", request_body = CreateSavedFilter, security(("bearer" = [])),
    responses((status = 201, body = SavedFilter), (status = 409, description = "Too many filters"),
              (status = 422, description = "Invalid name or query, or the name is taken")))]
pub async fn create(
    State(state): State<AppState>,
    ctx: OrgContext,
    Json(body): Json<CreateSavedFilter>,
) -> ApiResult<(StatusCode, Json<SavedFilter>)> {
    let name = name(&body.name)?;
    let query = query(&body.query)?;
    let mut tx = state.db.begin(ctx.tenant_id).await?;
    // Serialize one person's creations so the limit cannot be raced past.
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext('saved_filters:' || $1::text || ':' || $2::text))")
        .bind(ctx.tenant_id)
        .bind(ctx.user_id)
        .execute(tx.conn())
        .await?;
    let (owned,): (i64,) = sqlx::query_as("SELECT count(*) FROM saved_filters WHERE tenant_id = $1 AND owner_id = $2")
        .bind(ctx.tenant_id)
        .bind(ctx.user_id)
        .fetch_one(tx.conn())
        .await?;
    if owned >= MAX_FILTERS {
        return Err(ApiError::Conflict(format!("You have {MAX_FILTERS} saved filters. Delete one first.")));
    }
    let row: Row = sqlx::query_as(&format!(
        "INSERT INTO saved_filters (tenant_id, id, owner_id, name, query, shared) VALUES ($1, $2, $3, $4, $5, $6) \
         RETURNING {COLUMNS}"
    ))
    .bind(ctx.tenant_id)
    .bind(platform_common::new_id())
    .bind(ctx.user_id)
    .bind(&name)
    .bind(&query)
    .bind(body.shared)
    .fetch_one(tx.conn())
    .await
    .map_err(|e| duplicate(e, &name))?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(dto(&ctx, row))))
}

#[utoipa::path(get, path = "/api/v1/search/filters/{id}", security(("bearer" = [])), params(("id" = Uuid, Path)),
    responses((status = 200, body = SavedFilter), (status = 404)))]
pub async fn get_one(State(state): State<AppState>, ctx: OrgContext, Path(id): Path<Uuid>) -> ApiResult<Json<SavedFilter>> {
    let mut tx = state.db.begin(ctx.tenant_id).await?;
    let row = visible(tx.conn(), &ctx, id).await?;
    tx.commit().await?;
    Ok(Json(dto(&ctx, row)))
}

#[utoipa::path(patch, path = "/api/v1/search/filters/{id}", request_body = UpdateSavedFilter, security(("bearer" = [])),
    params(("id" = Uuid, Path)), responses((status = 200, body = SavedFilter), (status = 403), (status = 404), (status = 422)))]
pub async fn update(
    State(state): State<AppState>,
    ctx: OrgContext,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateSavedFilter>,
) -> ApiResult<Json<SavedFilter>> {
    let new_name = body.name.as_deref().map(name).transpose()?;
    let new_query = body.query.as_deref().map(query).transpose()?;
    let mut tx = state.db.begin(ctx.tenant_id).await?;
    let current = editable(tx.conn(), &ctx, id).await?;
    // Only the owner decides who else sees it.
    if body.shared.is_some_and(|s| s != current.shared) && current.owner_id != ctx.user_id {
        return Err(ApiError::Forbidden);
    }
    let shown_name = new_name.clone().unwrap_or_else(|| current.name.clone());
    let row: Row = sqlx::query_as(&format!(
        "UPDATE saved_filters SET name = COALESCE($3, name), query = COALESCE($4, query), shared = COALESCE($5, shared), \
         updated_at = now() WHERE tenant_id = $1 AND id = $2 RETURNING {COLUMNS}"
    ))
    .bind(ctx.tenant_id)
    .bind(id)
    .bind(new_name)
    .bind(new_query)
    .bind(body.shared)
    .fetch_one(tx.conn())
    .await
    .map_err(|e| duplicate(e, &shown_name))?;
    tx.commit().await?;
    Ok(Json(dto(&ctx, row)))
}

#[utoipa::path(delete, path = "/api/v1/search/filters/{id}", security(("bearer" = [])), params(("id" = Uuid, Path)),
    responses((status = 204), (status = 403), (status = 404)))]
pub async fn delete(State(state): State<AppState>, ctx: OrgContext, Path(id): Path<Uuid>) -> ApiResult<StatusCode> {
    let mut tx = state.db.begin(ctx.tenant_id).await?;
    editable(tx.conn(), &ctx, id).await?;
    sqlx::query("DELETE FROM saved_filters WHERE tenant_id = $1 AND id = $2")
        .bind(ctx.tenant_id)
        .bind(id)
        .execute(tx.conn())
        .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}
