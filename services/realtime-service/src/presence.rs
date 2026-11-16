//! Who else is looking at an issue.
//!
//! `PUT /api/v1/rt/presence` with `{project_id, issue_id}` says "I am here" and answers with
//! the others who are; the page repeats it every 30 s while open, and `DELETE` (same body)
//! says "I left". Presence lasts 60 s without a repeat, so a closed laptop drops out on its
//! own. Arriving and leaving are announced on the project's channel as a
//! `presence.changed` hint (the issue id, the person), so other viewers ask again at once.
//!
//! Kept in Redis per workspace, project and issue (`rt:presence:{tenant}:<project>:<issue>`,
//! a sorted set of user ids scored by when they drop out). Needs Browse on the project; the
//! key includes the project, so naming an issue of another project only ever reaches the
//! viewers of that issue within the project the caller can browse (none). Only user ids
//! are returned: names come from the workspace directory, which the caller can read anyway.

use auth_middleware::{AuthContext, OrgContext};
use axum::{extract::State, http::StatusCode, Json};
use error_handling::{ApiError, ApiResult};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{channels::Channel, http::authorize, AppState};

/// How long someone counts as present after their last `PUT`.
pub const PRESENT_FOR_MS: i64 = 60_000;
/// Most viewers returned for one issue.
const MAX_VIEWERS: isize = 50;

#[derive(Debug, Deserialize)]
pub struct Where {
    pub project_id: Uuid,
    pub issue_id: Uuid,
}

#[derive(Debug, Serialize)]
pub struct Viewers {
    /// The others looking at the issue now, most recently active first.
    pub viewers: Vec<Uuid>,
}

fn key(tenant: Uuid, w: &Where) -> String {
    format!("rt:presence:{tenant}:{}:{}", w.project_id, w.issue_id)
}

fn now_ms() -> i64 {
    (time::OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000) as i64
}

fn unavailable(error: impl std::fmt::Display) -> ApiError {
    tracing::warn!(%error, "presence unavailable");
    ApiError::DependencyUnavailable("presence")
}

async fn announce(state: &AppState, tenant: Uuid, w: &Where, user: Uuid) {
    let hint = serde_json::json!({
        "type": "presence.changed", "id": w.issue_id, "version": 0, "actor": user,
        "at": time::OffsetDateTime::now_utc().format(&time::format_description::well_known::Rfc3339).unwrap_or_default(),
    });
    if let Err(error) = state.hub.publish(tenant, &Channel::Project(w.project_id), &hint.to_string()).await {
        tracing::warn!(%error, "could not announce presence");
    }
}

pub async fn here(State(state): State<AppState>, auth: AuthContext, Json(w): Json<Where>) -> ApiResult<Json<Viewers>> {
    let ctx = OrgContext::try_from(auth)?;
    authorize(&state, &ctx, Channel::Project(w.project_id)).await?;
    let mut conn = state.redis.get().ok_or_else(|| unavailable("redis is not connected"))?;
    let key = key(ctx.tenant_id, &w);
    let now = now_ms();
    let me = ctx.user_id.to_string();
    let (added, others): (i64, Vec<String>) = redis::pipe()
        .cmd("ZADD")
        .arg(&key)
        .arg(now + PRESENT_FOR_MS)
        .arg(&me)
        .cmd("ZREMRANGEBYSCORE")
        .arg(&key)
        .arg("-inf")
        .arg(now)
        .ignore()
        .cmd("PEXPIREAT")
        .arg(&key)
        .arg(now + PRESENT_FOR_MS)
        .ignore()
        .cmd("ZREVRANGE")
        .arg(&key)
        .arg(0)
        .arg(MAX_VIEWERS)
        .query_async(&mut conn)
        .await
        .map_err(unavailable)?;
    // Someone new: the others' pages should show them now, not at their next repeat.
    if added == 1 {
        announce(&state, ctx.tenant_id, &w, ctx.user_id).await;
    }
    let viewers =
        others.iter().filter(|v| **v != me).filter_map(|v| Uuid::parse_str(v).ok()).take(MAX_VIEWERS as usize).collect();
    Ok(Json(Viewers { viewers }))
}

pub async fn left(State(state): State<AppState>, auth: AuthContext, Json(w): Json<Where>) -> ApiResult<StatusCode> {
    let ctx = OrgContext::try_from(auth)?;
    authorize(&state, &ctx, Channel::Project(w.project_id)).await?;
    let mut conn = state.redis.get().ok_or_else(|| unavailable("redis is not connected"))?;
    let removed: i64 = redis::cmd("ZREM")
        .arg(key(ctx.tenant_id, &w))
        .arg(ctx.user_id.to_string())
        .query_async(&mut conn)
        .await
        .map_err(unavailable)?;
    if removed == 1 {
        announce(&state, ctx.tenant_id, &w, ctx.user_id).await;
    }
    Ok(StatusCode::NO_CONTENT)
}
