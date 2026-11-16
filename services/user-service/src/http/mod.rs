pub mod audit;
pub mod dto;
pub mod invitations;
pub mod members;
pub mod organizations;
pub mod provisioning;
pub mod scim;
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
        invitations::resend,
        invitations::preview,
        invitations::accept,
        audit::list,
        audit::export,
        audit::verify,
        provisioning::list_tokens,
        provisioning::create_token,
        provisioning::revoke_token,
        provisioning::list_users
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
        dto::AcceptInvitationResponse,
        audit::AuditPage,
        audit::AuditEntryDto,
        audit::AuditActor,
        audit::AuditVerification,
        provisioning::ProvisioningToken,
        provisioning::ProvisioningTokenList,
        provisioning::CreateProvisioningToken,
        provisioning::CreatedProvisioningToken,
        provisioning::ProvisionedUser,
        provisioning::ProvisionedUserList
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
        .route("/organizations/current/invitations/{id}/resend", post(invitations::resend))
        .route("/invitations/{token}", get(invitations::preview))
        .route("/invitations/{token}/accept", post(invitations::accept))
        .route("/organizations/current/audit", get(audit::list))
        .route("/organizations/current/audit/export", get(audit::export))
        .route("/organizations/current/audit/verify", get(audit::verify))
        .route("/organizations/current/provisioning/tokens", get(provisioning::list_tokens).post(provisioning::create_token))
        .route("/organizations/current/provisioning/tokens/{id}", delete(provisioning::revoke_token))
        .route("/organizations/current/provisioning/users", get(provisioning::list_users));

    // Called by identity providers with a provisioning token; no browser, so no CORS.
    let scim = Router::new()
        .route("/ServiceProviderConfig", get(scim::service_provider_config))
        .route("/ResourceTypes", get(scim::resource_types))
        .route("/Schemas", get(scim::schemas))
        .route("/Users", get(scim::list_users).post(scim::create_user))
        .route("/Users/{id}", get(scim::get_user).put(scim::replace_user).patch(scim::patch_user).delete(scim::delete_user))
        .route("/Groups", get(scim::groups).post(scim::groups))
        .route(
            "/Groups/{*rest}",
            get(scim::groups).post(scim::groups).put(scim::groups).patch(scim::groups).delete(scim::groups),
        );

    let origins: Vec<_> = state.settings.allowed_origins.iter().filter_map(|o| o.parse().ok()).collect();
    let cors = CorsLayer::new()
        .allow_origin(AllowOrigin::list(origins))
        .allow_methods([Method::GET, Method::POST, Method::DELETE, Method::PATCH])
        .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE, header::IF_MATCH])
        .expose_headers([header::ETAG, header::CONTENT_DISPOSITION]);

    Router::new()
        .nest("/api/v1", api)
        .route("/openapi.json", get(|| async { Json(ApiDoc::openapi()) }))
        .layer(cors)
        .nest("/scim/v2", scim)
        .with_state(state)
}
