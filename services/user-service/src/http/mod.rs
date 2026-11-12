pub mod dto;
pub mod invitations;
pub mod members;
pub mod organizations;
pub mod users;

use axum::{
    http::{header, Method},
    routing::{delete, get, patch, post},
    Json, Router,
};
use tower_http::cors::{AllowOrigin, CorsLayer};
use utoipa::OpenApi;

use crate::AppState;

#[derive(OpenApi)]
#[openapi(
    info(
        title = "Users & Organizations API",
        version = "1.0.0",
        description = "Profiles, organizations, members and invitations."
    ),
    paths(
        users::me,
        users::update_me,
        organizations::list_mine,
        organizations::create,
        organizations::current,
        organizations::update_current,
        members::list,
        members::update,
        members::remove,
        invitations::list,
        invitations::create,
        invitations::revoke,
        invitations::preview,
        invitations::accept
    ),
    components(schemas(
        dto::ProfileDto,
        dto::UpdateProfileRequest,
        dto::OrganizationDto,
        dto::OrganizationList,
        dto::CreateOrganizationRequest,
        dto::UpdateOrganizationRequest,
        dto::MemberDto,
        dto::MemberPage,
        dto::UpdateMemberRequest,
        dto::CreateInvitationRequest,
        dto::InvitationDto,
        dto::InvitationList,
        dto::InvitationPreview,
        dto::AcceptInvitationResponse
    ))
)]
pub struct ApiDoc;

pub fn router(state: AppState) -> Router {
    let api = Router::new()
        .route("/users/me", get(users::me).patch(users::update_me))
        .route("/organizations", get(organizations::list_mine).post(organizations::create))
        .route("/organizations/current", get(organizations::current).patch(organizations::update_current))
        .route("/organizations/current/members", get(members::list))
        .route("/organizations/current/members/{user_id}", patch(members::update).delete(members::remove))
        .route("/organizations/current/invitations", get(invitations::list).post(invitations::create))
        .route("/organizations/current/invitations/{id}", delete(invitations::revoke))
        .route("/invitations/{token}", get(invitations::preview))
        .route("/invitations/{token}/accept", post(invitations::accept));

    let origins: Vec<_> = state.settings.allowed_origins.iter().filter_map(|o| o.parse().ok()).collect();
    let cors = CorsLayer::new()
        .allow_origin(AllowOrigin::list(origins))
        .allow_methods([Method::GET, Method::POST, Method::DELETE, Method::PATCH])
        .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE, header::IF_MATCH])
        .expose_headers([header::ETAG]);

    Router::new()
        .nest("/api/v1", api)
        .route("/openapi.json", get(|| async { Json(ApiDoc::openapi()) }))
        .layer(cors)
        .with_state(state)
}
