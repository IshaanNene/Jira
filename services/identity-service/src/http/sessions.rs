use auth_middleware::AuthContext;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use error_handling::{ApiError, ApiResult};
use uuid::Uuid;

use super::{
    auth::{require_session, revoke_session_everywhere},
    dto::{SessionDto, SessionList},
};
use crate::{store, AppState};

#[utoipa::path(get, path = "/api/v1/auth/sessions", security(("bearer" = [])), responses((status = 200, body = SessionList)))]
pub async fn list(State(state): State<AppState>, auth: AuthContext) -> ApiResult<Json<SessionList>> {
    require_session(&auth)?;
    let sessions = store::active_sessions(&state.pool, auth.user_id).await?;
    Ok(Json(SessionList { items: sessions.into_iter().map(|s| SessionDto::from_session(s, auth.session_id)).collect() }))
}

#[utoipa::path(delete, path = "/api/v1/auth/sessions/{id}", security(("bearer" = [])),
    params(("id" = Uuid, Path, description = "Session id")),
    responses((status = 204), (status = 404, description = "No such session for this account")))]
pub async fn revoke(State(state): State<AppState>, auth: AuthContext, Path(id): Path<Uuid>) -> ApiResult<StatusCode> {
    require_session(&auth)?;
    // Scoped to the caller's own sessions: another account's session id is simply "not found".
    if revoke_session_everywhere(&state, id, auth.user_id, "revoked_by_user").await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound("session"))
    }
}
