//! `/api/v1/notifications`: the signed-in person's own notifications, limited to projects
//! they can still see.

use auth_middleware::{OrgContext, VisibleProjects};
use axum::{
    extract::{Path, Query, State},
    http::{header, Method, StatusCode},
    routing::{get, post},
    Json, Router,
};
use error_handling::{ApiError, ApiResult};
use platform_common::pagination::{decode_cursor, encode_cursor, DEFAULT_LIMIT, MAX_LIMIT};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use tower_http::cors::{AllowOrigin, CorsLayer};
use utoipa::{IntoParams, OpenApi, ToSchema};
use uuid::Uuid;

use crate::{store, AppState};

#[derive(Debug, Serialize, ToSchema)]
pub struct NotificationDto {
    pub id: Uuid,
    /// assigned | commented | mentioned | status_changed
    pub kind: String,
    pub project_id: Uuid,
    pub issue_id: Uuid,
    pub issue_key: String,
    /// The issue's summary when the notification was created.
    pub issue_summary: String,
    pub actor_id: Option<Uuid>,
    pub actor_name: String,
    /// Kind-specific: `to_status`/`from_status` (status keys) or `comment_id`.
    #[schema(value_type = Object)]
    pub detail: serde_json::Value,
    pub read: bool,
    #[serde(with = "time::serde::rfc3339")]
    #[schema(value_type = String, format = DateTime)]
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct NotificationList {
    pub items: Vec<NotificationDto>,
    pub next_cursor: Option<String>,
    pub unread_count: i64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct UnreadCount {
    pub count: i64,
}

/// What the person wants emailed in this workspace. In-app notifications are always on.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, PartialEq, Eq)]
pub struct NotificationPreferences {
    /// Master switch for email.
    pub email_enabled: bool,
    pub email_assigned: bool,
    pub email_mentioned: bool,
    pub email_commented: bool,
    pub email_status_changed: bool,
}

impl From<store::Preferences> for NotificationPreferences {
    fn from(p: store::Preferences) -> Self {
        let on = |k: &str| !p.email_muted.iter().any(|m| m == k);
        Self {
            email_enabled: p.email_enabled,
            email_assigned: on("assigned"),
            email_mentioned: on("mentioned"),
            email_commented: on("commented"),
            email_status_changed: on("status_changed"),
        }
    }
}

impl From<&NotificationPreferences> for store::Preferences {
    fn from(p: &NotificationPreferences) -> Self {
        let muted = [
            ("assigned", p.email_assigned),
            ("mentioned", p.email_mentioned),
            ("commented", p.email_commented),
            ("status_changed", p.email_status_changed),
        ];
        Self {
            email_enabled: p.email_enabled,
            email_muted: muted.iter().filter(|(_, on)| !on).map(|(k, _)| (*k).to_owned()).collect(),
        }
    }
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct ListQuery {
    pub limit: Option<u32>,
    pub cursor: Option<String>,
    /// Only unread notifications.
    #[serde(default)]
    pub unread: bool,
}

#[derive(Serialize, Deserialize)]
struct Cursor {
    #[serde(with = "time::serde::rfc3339")]
    at: OffsetDateTime,
    id: Uuid,
}

fn dto(n: store::Notification) -> NotificationDto {
    NotificationDto {
        id: n.id,
        kind: n.kind,
        project_id: n.project_id,
        issue_id: n.issue_id,
        issue_key: n.issue_key,
        issue_summary: n.issue_summary,
        actor_id: n.actor_id,
        actor_name: n.actor_name,
        detail: n.detail,
        read: n.read_at.is_some(),
        created_at: n.created_at,
    }
}

async fn visible(state: &AppState, ctx: &OrgContext) -> ApiResult<Option<Vec<Uuid>>> {
    Ok(match state.policy.snapshot(ctx).await?.visible_projects(ctx) {
        VisibleProjects::All => None,
        VisibleProjects::Only(ids) => Some(ids),
    })
}

#[utoipa::path(get, path = "/api/v1/notifications", params(ListQuery), security(("bearer" = [])),
    responses((status = 200, body = NotificationList)))]
pub async fn list(
    State(state): State<AppState>,
    ctx: OrgContext,
    Query(q): Query<ListQuery>,
) -> ApiResult<Json<NotificationList>> {
    let limit = i64::from(q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT));
    let after = match q.cursor.as_deref() {
        Some(raw) => {
            let c: Cursor =
                decode_cursor(raw).map_err(|_| ApiError::invalid("cursor", "invalid_cursor", "Not a valid cursor."))?;
            Some((c.at, c.id))
        }
        None => None,
    };
    let visible = visible(&state, &ctx).await?;
    let mut tx = state.db.begin(ctx.tenant_id).await?;
    let mut rows = store::list(tx.conn(), ctx.tenant_id, ctx.user_id, visible.as_deref(), q.unread, after, limit + 1).await?;
    let unread_count = store::unread_count(tx.conn(), ctx.tenant_id, ctx.user_id, visible.as_deref()).await?;
    tx.commit().await?;
    let next_cursor = if rows.len() as i64 > limit {
        rows.truncate(limit as usize);
        rows.last().map(|n| encode_cursor(&Cursor { at: n.created_at, id: n.id }))
    } else {
        None
    };
    Ok(Json(NotificationList { items: rows.into_iter().map(dto).collect(), next_cursor, unread_count }))
}

#[utoipa::path(get, path = "/api/v1/notifications/unread-count", security(("bearer" = [])),
    responses((status = 200, body = UnreadCount)))]
pub async fn unread(State(state): State<AppState>, ctx: OrgContext) -> ApiResult<Json<UnreadCount>> {
    let visible = visible(&state, &ctx).await?;
    let mut tx = state.db.begin(ctx.tenant_id).await?;
    let count = store::unread_count(tx.conn(), ctx.tenant_id, ctx.user_id, visible.as_deref()).await?;
    tx.commit().await?;
    Ok(Json(UnreadCount { count }))
}

#[utoipa::path(post, path = "/api/v1/notifications/{id}/read", params(("id" = Uuid, Path)), security(("bearer" = [])),
    responses((status = 204), (status = 404)))]
pub async fn read_one(State(state): State<AppState>, ctx: OrgContext, Path(id): Path<Uuid>) -> ApiResult<StatusCode> {
    let mut tx = state.db.begin(ctx.tenant_id).await?;
    let found = store::mark_read(tx.conn(), ctx.tenant_id, ctx.user_id, id).await?;
    tx.commit().await?;
    if found {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound("notification"))
    }
}

#[utoipa::path(post, path = "/api/v1/notifications/read-all", security(("bearer" = [])), responses((status = 204)))]
pub async fn read_all(State(state): State<AppState>, ctx: OrgContext) -> ApiResult<StatusCode> {
    let mut tx = state.db.begin(ctx.tenant_id).await?;
    store::mark_all_read(tx.conn(), ctx.tenant_id, ctx.user_id).await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(get, path = "/api/v1/notifications/preferences", security(("bearer" = [])),
    responses((status = 200, body = NotificationPreferences)))]
pub async fn get_preferences(State(state): State<AppState>, ctx: OrgContext) -> ApiResult<Json<NotificationPreferences>> {
    let mut tx = state.db.begin(ctx.tenant_id).await?;
    let prefs = store::preferences(tx.conn(), ctx.tenant_id, ctx.user_id).await?;
    tx.commit().await?;
    Ok(Json(prefs.into()))
}

#[utoipa::path(put, path = "/api/v1/notifications/preferences", request_body = NotificationPreferences, security(("bearer" = [])),
    responses((status = 200, body = NotificationPreferences)))]
pub async fn put_preferences(
    State(state): State<AppState>,
    ctx: OrgContext,
    Json(body): Json<NotificationPreferences>,
) -> ApiResult<Json<NotificationPreferences>> {
    let prefs = store::Preferences::from(&body);
    let mut tx = state.db.begin(ctx.tenant_id).await?;
    store::save_preferences(tx.conn(), ctx.tenant_id, ctx.user_id, &prefs).await?;
    tx.commit().await?;
    Ok(Json(prefs.into()))
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct UnsubscribeQuery {
    /// From the link in the email.
    pub token: String,
}

/// Optional body of an unsubscribe request.
#[derive(Debug, Deserialize, ToSchema)]
pub struct UnsubscribeRequest {
    /// Turn off all email, not just this kind.
    pub all: Option<bool>,
}

/// What an unsubscribe link would turn off. Says nothing about who it is for.
#[derive(Debug, Serialize, ToSchema)]
pub struct UnsubscribeLink {
    /// assigned | commented | mentioned | status_changed
    pub kind: String,
    /// e.g. "comments"
    pub kind_label: String,
}

fn verify_link(state: &AppState, token: &str) -> ApiResult<crate::unsubscribe::Claims> {
    let claims = state
        .unsubscribe
        .verify(token, OffsetDateTime::now_utc().unix_timestamp())
        .ok_or(ApiError::NotFound("unsubscribe link"))?;
    if crate::domain::Kind::parse(&claims.kind).is_none() {
        return Err(ApiError::NotFound("unsubscribe link"));
    }
    Ok(claims)
}

/// Public (the link works signed out). Reading a link changes nothing, so mail scanners
/// that follow links cannot unsubscribe anyone.
#[utoipa::path(get, path = "/api/v1/notifications/unsubscribe", params(UnsubscribeQuery),
    responses((status = 200, body = UnsubscribeLink), (status = 404, description = "Invalid or expired link")))]
pub async fn describe_unsubscribe(
    State(state): State<AppState>,
    Query(q): Query<UnsubscribeQuery>,
) -> ApiResult<Json<UnsubscribeLink>> {
    let claims = verify_link(&state, &q.token)?;
    let kind = crate::domain::Kind::parse(&claims.kind).expect("checked");
    Ok(Json(UnsubscribeLink { kind: claims.kind, kind_label: crate::email::kind_label(kind).into() }))
}

/// Public. Turns off email about the link's kind; with a JSON body `{"all": true}`, all
/// email. A form body `List-Unsubscribe=One-Click` (RFC 8058, sent by mail clients) is
/// the same as no body.
#[utoipa::path(post, path = "/api/v1/notifications/unsubscribe", params(UnsubscribeQuery), request_body(content = Option<UnsubscribeRequest>),
    responses((status = 204), (status = 404, description = "Invalid or expired link")))]
pub async fn unsubscribe(
    State(state): State<AppState>,
    Query(q): Query<UnsubscribeQuery>,
    body: axum::body::Bytes,
) -> ApiResult<StatusCode> {
    let claims = verify_link(&state, &q.token)?;
    let all = serde_json::from_slice::<UnsubscribeRequest>(&body).ok().and_then(|b| b.all).unwrap_or(false);
    let mut tx = state.db.begin(claims.tenant_id).await?;
    store::mute_email(tx.conn(), claims.tenant_id, claims.user_id, if all { None } else { Some(&claims.kind) }).await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(OpenApi)]
#[openapi(
    info(title = "Notifications API", version = "1.0.0", description = "The signed-in person's in-app notifications."),
    paths(list, unread, read_one, read_all, get_preferences, put_preferences, describe_unsubscribe, unsubscribe),
    components(schemas(
        NotificationDto,
        NotificationList,
        UnreadCount,
        NotificationPreferences,
        UnsubscribeLink,
        UnsubscribeRequest
    ))
)]
pub struct ApiDoc;

pub fn router(state: AppState) -> Router {
    let api = Router::new()
        .route("/notifications", get(list))
        .route("/notifications/unread-count", get(unread))
        .route("/notifications/read-all", post(read_all))
        .route("/notifications/preferences", get(get_preferences).put(put_preferences))
        .route("/notifications/unsubscribe", get(describe_unsubscribe).post(unsubscribe))
        .route("/notifications/{id}/read", post(read_one));
    let origins: Vec<_> = state.allowed_origins.iter().filter_map(|o| o.parse().ok()).collect();
    let cors = CorsLayer::new()
        .allow_origin(AllowOrigin::list(origins))
        .allow_methods([Method::GET, Method::POST, Method::PUT])
        .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE]);
    Router::new()
        .nest("/api/v1", api)
        .route("/openapi.json", get(|| async { Json(ApiDoc::openapi()) }))
        .layer(cors)
        .with_state(state)
}
