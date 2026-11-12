use auth_middleware::AuthContext;
use axum::{extract::State, Json};
use error_handling::ApiResult;
use validator::Validate;

use super::dto::{ProfileDto, UpdateProfileRequest};
use crate::{store, AppState};

#[utoipa::path(get, path = "/api/v1/users/me", security(("bearer" = [])), responses((status = 200, body = ProfileDto)))]
pub async fn me(State(state): State<AppState>, auth: AuthContext) -> ApiResult<Json<ProfileDto>> {
    let profile = store::ensure_profile(state.db.global()?, auth.user_id, &auth.email, &auth.name).await?;
    Ok(Json(profile.into()))
}

#[utoipa::path(patch, path = "/api/v1/users/me", request_body = UpdateProfileRequest, security(("bearer" = [])),
    responses((status = 200, body = ProfileDto), (status = 422)))]
pub async fn update_me(
    State(state): State<AppState>,
    auth: AuthContext,
    Json(mut body): Json<UpdateProfileRequest>,
) -> ApiResult<Json<ProfileDto>> {
    body.display_name = body.display_name.map(|n| n.trim().to_owned());
    body.validate()?;
    let pool = state.db.global()?;
    store::ensure_profile(pool, auth.user_id, &auth.email, &auth.name).await?;
    let profile =
        store::update_profile(pool, auth.user_id, body.display_name.as_deref(), body.timezone.as_deref(), body.locale.as_deref())
            .await?;
    Ok(Json(profile.into()))
}
