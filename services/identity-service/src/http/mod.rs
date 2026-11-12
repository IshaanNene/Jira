pub mod auth;
pub mod dto;
pub mod sessions;

use axum::{
    extract::{Request, State},
    http::{header, Method},
    middleware::{self, Next},
    response::Response,
    routing::{delete, get, post},
    Json, Router,
};
use error_handling::ApiError;
use tower_http::cors::{AllowOrigin, CorsLayer};
use utoipa::OpenApi;

use crate::AppState;

#[derive(OpenApi)]
#[openapi(
    info(title = "Identity API", version = "1.0.0", description = "Accounts, sessions and access tokens."),
    paths(
        auth::register,
        auth::login,
        auth::refresh,
        auth::exchange,
        auth::logout,
        auth::me,
        auth::jwks,
        sessions::list,
        sessions::revoke
    ),
    components(schemas(
        dto::RegisterRequest,
        dto::LoginRequest,
        dto::RefreshRequest,
        dto::ExchangeRequest,
        dto::TokenResponse,
        dto::TokenOrganization,
        dto::AccountDto,
        dto::SessionDto,
        dto::SessionList
    ))
)]
pub struct ApiDoc;

pub fn router(state: AppState) -> Router {
    // Cookie-authenticated endpoints get an Origin allow-list check on top of SameSite=Strict.
    let cookie_routes = Router::new()
        .route("/token/refresh", post(auth::refresh))
        .route("/logout", post(auth::logout))
        .route_layer(middleware::from_fn_with_state(state.clone(), require_trusted_origin));

    let auth_routes = Router::new()
        .route("/register", post(auth::register))
        .route("/login", post(auth::login))
        .route("/token/exchange", post(auth::exchange))
        .route("/me", get(auth::me))
        .route("/sessions", get(sessions::list))
        .route("/sessions/{id}", delete(sessions::revoke))
        .route("/.well-known/jwks.json", get(auth::jwks))
        .merge(cookie_routes);

    let origins: Vec<_> = state.settings.allowed_origins.iter().filter_map(|o| o.parse().ok()).collect();
    let cors = CorsLayer::new()
        .allow_origin(AllowOrigin::list(origins))
        .allow_methods([Method::GET, Method::POST, Method::DELETE, Method::PATCH])
        .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE, header::IF_MATCH])
        .allow_credentials(true);

    Router::new()
        .nest("/api/v1/auth", auth_routes)
        .route("/openapi.json", get(|| async { Json(ApiDoc::openapi()) }))
        .layer(cors)
        .with_state(state)
}

/// Browsers attach `Origin` to cross-site POSTs. If it is present it must be one of ours.
/// Non-browser clients (no `Origin`) are unaffected; they cannot ride a victim's cookie.
async fn require_trusted_origin(State(state): State<AppState>, req: Request, next: Next) -> Result<Response, ApiError> {
    if let Some(origin) = req.headers().get(header::ORIGIN) {
        let trusted = origin.to_str().is_ok_and(|o| state.settings.allowed_origins.iter().any(|a| a == o));
        if !trusted {
            tracing::warn!(?origin, "rejected cookie-authenticated request from untrusted origin");
            return Err(ApiError::Forbidden);
        }
    }
    Ok(next.run(req).await)
}
