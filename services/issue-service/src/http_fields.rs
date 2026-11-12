//! Administration of custom field definitions.

use auth_middleware::{OrgContext, OrgPermission};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{get, patch},
    Json, Router,
};
use error_handling::{ApiError, ApiResult};
use uuid::Uuid;

use crate::{
    custom_fields::validate_definition,
    dto::{CreateCustomFieldRequest, CustomFieldDto, CustomFieldList, UpdateCustomFieldRequest},
    store, AppState,
};

fn clean(values: Vec<String>, upper: bool) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for v in values {
        let v = if upper { v.trim().to_uppercase() } else { v.trim().to_owned() };
        if !v.is_empty() && !out.contains(&v) {
            out.push(v);
        }
    }
    out
}

fn duplicate(err: sqlx::Error) -> ApiError {
    match &err {
        sqlx::Error::Database(db) if db.is_unique_violation() => {
            ApiError::Conflict("A custom field with that name already exists.".into())
        }
        _ => err.into(),
    }
}

/// Definitions are readable by everyone in the organization: the issue form needs them.
#[utoipa::path(get, path = "/api/v1/custom-fields", security(("bearer" = [])), responses((status = 200, body = CustomFieldList)))]
pub async fn list(State(state): State<AppState>, ctx: OrgContext) -> ApiResult<Json<CustomFieldList>> {
    let mut tx = state.db.begin(ctx.tenant_id).await?;
    let rows = store::fields(tx.conn(), ctx.tenant_id).await?;
    tx.commit().await?;
    Ok(Json(CustomFieldList { items: rows.into_iter().map(Into::into).collect() }))
}

#[utoipa::path(post, path = "/api/v1/custom-fields", request_body = CreateCustomFieldRequest, security(("bearer" = [])),
    responses((status = 201, body = CustomFieldDto), (status = 403), (status = 409), (status = 422)))]
pub async fn create(
    State(state): State<AppState>,
    ctx: OrgContext,
    Json(body): Json<CreateCustomFieldRequest>,
) -> ApiResult<(StatusCode, Json<CustomFieldDto>)> {
    ctx.require(OrgPermission::ManageOrganization)?;
    let name = body.name.trim().to_owned();
    let (options, projects, types) = (clean(body.options, false), clean(body.project_keys, true), clean(body.issue_types, false));
    validate_definition(&name, &body.field_type, &options, &projects, &types)?;
    let mut tx = state.db.begin(ctx.tenant_id).await?;
    let field = store::insert_field(
        tx.conn(),
        ctx.tenant_id,
        &name,
        &body.field_type,
        &options,
        body.required,
        &projects,
        &types,
        ctx.user_id,
    )
    .await
    .map_err(duplicate)?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(field.into())))
}

#[utoipa::path(patch, path = "/api/v1/custom-fields/{id}", request_body = UpdateCustomFieldRequest, security(("bearer" = [])),
    params(("id" = Uuid, Path)), responses((status = 200, body = CustomFieldDto), (status = 403), (status = 404), (status = 422)))]
pub async fn update(
    State(state): State<AppState>,
    ctx: OrgContext,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateCustomFieldRequest>,
) -> ApiResult<Json<CustomFieldDto>> {
    ctx.require(OrgPermission::ManageOrganization)?;
    let name = body.name.trim().to_owned();
    let (options, projects, types) = (clean(body.options, false), clean(body.project_keys, true), clean(body.issue_types, false));
    let mut tx = state.db.begin(ctx.tenant_id).await?;
    let existing = store::fields(tx.conn(), ctx.tenant_id)
        .await?
        .into_iter()
        .find(|f| f.id == id)
        .ok_or(ApiError::NotFound("custom field"))?;
    validate_definition(&name, &existing.field_type, &options, &projects, &types)?;
    let field = store::update_field(tx.conn(), ctx.tenant_id, id, &name, &options, body.required, &projects, &types)
        .await
        .map_err(duplicate)?
        .ok_or(ApiError::NotFound("custom field"))?;
    tx.commit().await?;
    Ok(Json(field.into()))
}

#[utoipa::path(delete, path = "/api/v1/custom-fields/{id}", security(("bearer" = [])), params(("id" = Uuid, Path)),
    responses((status = 204), (status = 403), (status = 404)))]
pub async fn archive(State(state): State<AppState>, ctx: OrgContext, Path(id): Path<Uuid>) -> ApiResult<StatusCode> {
    ctx.require(OrgPermission::ManageOrganization)?;
    let mut tx = state.db.begin(ctx.tenant_id).await?;
    let done = store::archive_field(tx.conn(), ctx.tenant_id, id).await?;
    tx.commit().await?;
    if done {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound("custom field"))
    }
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/custom-fields", get(list).post(create)).route("/custom-fields/{id}", patch(update).delete(archive))
}
