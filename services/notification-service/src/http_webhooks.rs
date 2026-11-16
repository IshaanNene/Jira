//! `/api/v1/webhooks`: workspace administrators manage outgoing webhooks.

use auth_middleware::{OrgContext, OrgPermission};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use error_handling::{ApiError, ApiResult};
use serde::{Deserialize, Serialize};
use serde_json::json;
use time::OffsetDateTime;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{
    webhooks::{self, Webhook, EVENTS, MAX_PROJECTS, MAX_WEBHOOKS, PING},
    AppState,
};

const NOT_FOUND: ApiError = ApiError::NotFound("webhook");

#[derive(Debug, Serialize, ToSchema)]
pub struct WebhookDto {
    pub id: Uuid,
    pub name: String,
    pub url: String,
    pub events: Vec<String>,
    /// Empty: every project.
    pub project_ids: Vec<Uuid>,
    pub enabled: bool,
    /// Why the service turned it off, if it did.
    pub disabled_reason: Option<String>,
    pub consecutive_failures: i32,
    pub created_by: Uuid,
    #[serde(with = "time::serde::rfc3339")]
    #[schema(value_type = String, format = DateTime)]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    #[schema(value_type = String, format = DateTime)]
    pub updated_at: OffsetDateTime,
}

impl From<Webhook> for WebhookDto {
    fn from(w: Webhook) -> Self {
        Self {
            id: w.id,
            name: w.name,
            url: w.url,
            events: w.events,
            project_ids: w.project_ids,
            enabled: w.enabled,
            disabled_reason: w.disabled_reason,
            consecutive_failures: w.consecutive_failures,
            created_by: w.created_by,
            created_at: w.created_at,
            updated_at: w.updated_at,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct WebhookList {
    pub items: Vec<WebhookDto>,
}

/// Returned once, when a webhook is created or its secret replaced.
#[derive(Debug, Serialize, ToSchema)]
pub struct WebhookWithSecret {
    pub webhook: WebhookDto,
    /// Used to verify `X-Meridian-Signature`. Not shown again.
    pub secret: String,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateWebhookRequest {
    pub name: String,
    pub url: String,
    pub events: Vec<String>,
    #[serde(default)]
    pub project_ids: Vec<Uuid>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateWebhookRequest {
    pub name: Option<String>,
    pub url: Option<String>,
    pub events: Option<Vec<String>>,
    pub project_ids: Option<Vec<Uuid>>,
    /// Turning a webhook back on clears its failure count.
    pub enabled: Option<bool>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct WebhookEventType {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct WebhookEventTypeList {
    pub items: Vec<WebhookEventType>,
}

#[derive(Debug, Serialize, ToSchema, sqlx::FromRow)]
pub struct WebhookDeliveryDto {
    pub id: Uuid,
    pub event_id: Uuid,
    pub event_type: String,
    /// pending | sending | delivered | failed | skipped
    pub status: String,
    pub attempts: i32,
    pub response_status: Option<i32>,
    pub last_error: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    #[schema(value_type = String, format = DateTime)]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    #[schema(value_type = Option<String>, format = DateTime)]
    pub finished_at: Option<OffsetDateTime>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct WebhookDeliveryList {
    pub items: Vec<WebhookDeliveryDto>,
}

fn name(raw: &str) -> ApiResult<String> {
    let n = raw.trim();
    if !(1..=100).contains(&n.chars().count()) {
        return Err(ApiError::invalid("name", "length", "Give the webhook a name of up to 100 characters."));
    }
    Ok(n.to_owned())
}

fn events(mut list: Vec<String>) -> ApiResult<Vec<String>> {
    list.sort();
    list.dedup();
    if list.is_empty() {
        return Err(ApiError::invalid("events", "required", "Choose at least one event."));
    }
    if let Some(bad) = list.iter().find(|e| !webhooks::is_supported(e)) {
        return Err(ApiError::invalid("events", "unknown_event", &format!("Unknown event: {bad}.")));
    }
    Ok(list)
}

fn projects(mut list: Vec<Uuid>) -> ApiResult<Vec<Uuid>> {
    list.sort();
    list.dedup();
    if list.len() > MAX_PROJECTS {
        return Err(ApiError::invalid("project_ids", "too_many", "Limit a webhook to at most 50 projects."));
    }
    Ok(list)
}

fn url(state: &AppState, raw: &str) -> ApiResult<String> {
    webhooks::validate_url(raw, state.webhooks.reach)
        .map(String::from)
        .map_err(|why| ApiError::invalid("url", "invalid_url", why))
}

fn admin(ctx: &OrgContext) -> ApiResult<()> {
    ctx.require(OrgPermission::ManageOrganization)?;
    Ok(())
}

/// The events a webhook can subscribe to.
#[utoipa::path(get, path = "/api/v1/webhooks/events", security(("bearer" = [])), responses((status = 200, body = WebhookEventTypeList)))]
pub async fn event_types(ctx: OrgContext) -> ApiResult<Json<WebhookEventTypeList>> {
    admin(&ctx)?;
    Ok(Json(WebhookEventTypeList {
        items: EVENTS.iter().map(|(n, d)| WebhookEventType { name: (*n).into(), description: (*d).into() }).collect(),
    }))
}

#[utoipa::path(get, path = "/api/v1/webhooks", security(("bearer" = [])), responses((status = 200, body = WebhookList), (status = 403)))]
pub async fn list(State(state): State<AppState>, ctx: OrgContext) -> ApiResult<Json<WebhookList>> {
    admin(&ctx)?;
    let mut tx = state.db.begin(ctx.tenant_id).await?;
    let hooks = webhooks::list(tx.conn(), ctx.tenant_id).await?;
    tx.commit().await?;
    Ok(Json(WebhookList { items: hooks.into_iter().map(Into::into).collect() }))
}

#[utoipa::path(post, path = "/api/v1/webhooks", request_body = CreateWebhookRequest, security(("bearer" = [])),
    responses((status = 201, body = WebhookWithSecret), (status = 403), (status = 409, description = "Too many webhooks"), (status = 422)))]
pub async fn create(
    State(state): State<AppState>,
    ctx: OrgContext,
    Json(body): Json<CreateWebhookRequest>,
) -> ApiResult<(StatusCode, Json<WebhookWithSecret>)> {
    admin(&ctx)?;
    let (name, url, events, projects) =
        (name(&body.name)?, url(&state, &body.url)?, events(body.events)?, projects(body.project_ids)?);
    let secret = webhooks::new_secret();
    let mut tx = state.db.begin(ctx.tenant_id).await?;
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM webhooks WHERE tenant_id = $1").bind(ctx.tenant_id).fetch_one(tx.conn()).await?;
    if count >= MAX_WEBHOOKS {
        return Err(ApiError::Conflict(format!("A workspace can have at most {MAX_WEBHOOKS} webhooks.")));
    }
    let hook: Webhook = sqlx::query_as(&format!(
        "INSERT INTO webhooks (id, tenant_id, name, url, events, project_ids, secret_sealed, created_by) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8) RETURNING {}",
        webhooks::WEBHOOK
    ))
    .bind(platform_common::new_id())
    .bind(ctx.tenant_id)
    .bind(&name)
    .bind(&url)
    .bind(&events)
    .bind(&projects)
    .bind(state.webhooks.secrets.seal(&secret))
    .bind(ctx.user_id)
    .fetch_one(tx.conn())
    .await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(WebhookWithSecret { webhook: hook.into(), secret })))
}

#[utoipa::path(patch, path = "/api/v1/webhooks/{id}", request_body = UpdateWebhookRequest, security(("bearer" = [])),
    params(("id" = Uuid, Path)), responses((status = 200, body = WebhookDto), (status = 404), (status = 422)))]
pub async fn update(
    State(state): State<AppState>,
    ctx: OrgContext,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateWebhookRequest>,
) -> ApiResult<Json<WebhookDto>> {
    admin(&ctx)?;
    let name = body.name.as_deref().map(name).transpose()?;
    let url = body.url.as_deref().map(|u| url(&state, u)).transpose()?;
    let events = body.events.map(events).transpose()?;
    let projects = body.project_ids.map(projects).transpose()?;
    let mut tx = state.db.begin(ctx.tenant_id).await?;
    let hook: Option<Webhook> = sqlx::query_as(&format!(
        "UPDATE webhooks SET name = COALESCE($3, name), url = COALESCE($4, url), events = COALESCE($5, events), \
                project_ids = COALESCE($6, project_ids), enabled = COALESCE($7, enabled), \
                disabled_reason = CASE WHEN $7 THEN NULL ELSE disabled_reason END, \
                consecutive_failures = CASE WHEN $7 THEN 0 ELSE consecutive_failures END, updated_at = now() \
         WHERE tenant_id = $1 AND id = $2 RETURNING {}",
        webhooks::WEBHOOK
    ))
    .bind(ctx.tenant_id)
    .bind(id)
    .bind(name)
    .bind(url)
    .bind(events)
    .bind(projects)
    .bind(body.enabled)
    .fetch_optional(tx.conn())
    .await?;
    tx.commit().await?;
    Ok(Json(hook.ok_or(NOT_FOUND)?.into()))
}

#[utoipa::path(delete, path = "/api/v1/webhooks/{id}", security(("bearer" = [])), params(("id" = Uuid, Path)), responses((status = 204), (status = 404)))]
pub async fn remove(State(state): State<AppState>, ctx: OrgContext, Path(id): Path<Uuid>) -> ApiResult<StatusCode> {
    admin(&ctx)?;
    let mut tx = state.db.begin(ctx.tenant_id).await?;
    let gone = sqlx::query("DELETE FROM webhooks WHERE tenant_id = $1 AND id = $2")
        .bind(ctx.tenant_id)
        .bind(id)
        .execute(tx.conn())
        .await?;
    tx.commit().await?;
    if gone.rows_affected() == 0 {
        return Err(NOT_FOUND);
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Replace the signing secret; the old one stops working at once.
#[utoipa::path(post, path = "/api/v1/webhooks/{id}/secret", security(("bearer" = [])), params(("id" = Uuid, Path)),
    responses((status = 200, body = WebhookWithSecret), (status = 404)))]
pub async fn rotate(State(state): State<AppState>, ctx: OrgContext, Path(id): Path<Uuid>) -> ApiResult<Json<WebhookWithSecret>> {
    admin(&ctx)?;
    let secret = webhooks::new_secret();
    let mut tx = state.db.begin(ctx.tenant_id).await?;
    let hook: Option<Webhook> = sqlx::query_as(&format!(
        "UPDATE webhooks SET secret_sealed = $3, updated_at = now() WHERE tenant_id = $1 AND id = $2 RETURNING {}",
        webhooks::WEBHOOK
    ))
    .bind(ctx.tenant_id)
    .bind(id)
    .bind(state.webhooks.secrets.seal(&secret))
    .fetch_optional(tx.conn())
    .await?;
    tx.commit().await?;
    Ok(Json(WebhookWithSecret { webhook: hook.ok_or(NOT_FOUND)?.into(), secret }))
}

/// Queue a `webhook.ping` delivery, to check the endpoint and its signature handling.
#[utoipa::path(post, path = "/api/v1/webhooks/{id}/test", security(("bearer" = [])), params(("id" = Uuid, Path)),
    responses((status = 202), (status = 404), (status = 409, description = "The webhook is off")))]
pub async fn test(State(state): State<AppState>, ctx: OrgContext, Path(id): Path<Uuid>) -> ApiResult<StatusCode> {
    admin(&ctx)?;
    let mut tx = state.db.begin(ctx.tenant_id).await?;
    let hook = webhooks::get(tx.conn(), ctx.tenant_id, id).await?.ok_or(NOT_FOUND)?;
    if !hook.enabled {
        return Err(ApiError::Conflict("Turn the webhook on before sending a test.".into()));
    }
    let event_id = platform_common::new_id();
    let body = webhooks::body(event_id, PING, OffsetDateTime::now_utc(), &json!({ "webhook_id": hook.id }));
    webhooks::enqueue(tx.conn(), ctx.tenant_id, hook.id, event_id, PING, &body).await?;
    tx.commit().await?;
    Ok(StatusCode::ACCEPTED)
}

/// The latest 50 deliveries, newest first.
#[utoipa::path(get, path = "/api/v1/webhooks/{id}/deliveries", security(("bearer" = [])), params(("id" = Uuid, Path)),
    responses((status = 200, body = WebhookDeliveryList), (status = 404)))]
pub async fn deliveries(
    State(state): State<AppState>,
    ctx: OrgContext,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<WebhookDeliveryList>> {
    admin(&ctx)?;
    let mut tx = state.db.begin(ctx.tenant_id).await?;
    // Loaded under row-level security first: the queue itself is not row-level secured.
    let hook = webhooks::get(tx.conn(), ctx.tenant_id, id).await?.ok_or(NOT_FOUND)?;
    let items: Vec<WebhookDeliveryDto> = sqlx::query_as(
        "SELECT id, event_id, event_type, status, attempts, response_status, last_error, created_at, finished_at \
         FROM webhook_deliveries WHERE tenant_id = $1 AND webhook_id = $2 ORDER BY created_at DESC, id DESC LIMIT 50",
    )
    .bind(ctx.tenant_id)
    .bind(hook.id)
    .fetch_all(tx.conn())
    .await?;
    tx.commit().await?;
    Ok(Json(WebhookDeliveryList { items }))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/webhooks", get(list).post(create))
        .route("/webhooks/events", get(event_types))
        .route("/webhooks/{id}", axum::routing::patch(update).delete(remove))
        .route("/webhooks/{id}/secret", post(rotate))
        .route("/webhooks/{id}/test", post(test))
        .route("/webhooks/{id}/deliveries", get(deliveries))
}
