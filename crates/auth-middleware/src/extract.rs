use std::sync::Arc;

use axum::{
    extract::{FromRef, FromRequestParts},
    http::{header::AUTHORIZATION, request::Parts},
};
use error_handling::ApiError;
use uuid::Uuid;

use crate::{
    authz::{OrgPermission, OrgRole},
    claims::{Claims, TokenKind},
    verifier::TokenVerifier,
};

/// A verified caller. Present on any authenticated request.
#[derive(Debug, Clone)]
pub struct AuthContext {
    pub user_id: Uuid,
    pub session_id: Uuid,
    pub email: String,
    pub name: String,
    pub claims: Claims,
}

/// A verified caller acting inside one organization. `tenant_id` is the only tenant
/// identifier handlers are allowed to use.
#[derive(Debug, Clone)]
pub struct OrgContext {
    pub user_id: Uuid,
    pub session_id: Uuid,
    pub tenant_id: Uuid,
    pub organization_id: Uuid,
    pub role: OrgRole,
    pub email: String,
    pub name: String,
}

impl OrgContext {
    /// Deny by default: every handler states the permission it needs.
    pub fn require(&self, permission: OrgPermission) -> Result<(), ApiError> {
        if self.role.allows(permission) {
            Ok(())
        } else {
            Err(ApiError::Forbidden)
        }
    }
}

fn bearer(parts: &Parts) -> Result<&str, ApiError> {
    parts
        .headers
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .ok_or(ApiError::Unauthenticated("Sign in to continue."))
}

impl<S> FromRequestParts<S> for AuthContext
where
    Arc<TokenVerifier>: FromRef<S>,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        if let Some(cached) = parts.extensions.get::<AuthContext>() {
            return Ok(cached.clone());
        }
        let verifier = Arc::<TokenVerifier>::from_ref(state);
        let claims = verifier.verify(bearer(parts)?).await?;
        tracing::Span::current().record("user_id", tracing::field::display(claims.sub));
        if let Some(tid) = claims.tid {
            tracing::Span::current().record("tenant_id", tracing::field::display(tid));
        }
        let ctx = AuthContext {
            user_id: claims.sub,
            session_id: claims.sid,
            email: claims.email.clone(),
            name: claims.name.clone(),
            claims,
        };
        parts.extensions.insert(ctx.clone());
        Ok(ctx)
    }
}

impl<S> FromRequestParts<S> for OrgContext
where
    Arc<TokenVerifier>: FromRef<S>,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let auth = AuthContext::from_request_parts(parts, state).await?;
        OrgContext::try_from(auth)
    }
}

impl TryFrom<AuthContext> for OrgContext {
    type Error = ApiError;

    fn try_from(auth: AuthContext) -> Result<Self, Self::Error> {
        const NO_ORG: ApiError = ApiError::Unauthenticated("Select an organization to continue.");
        if auth.claims.typ != TokenKind::Org {
            return Err(NO_ORG);
        }
        let tenant_id = auth.claims.tid.ok_or(NO_ORG)?;
        let organization_id = auth.claims.org.ok_or(NO_ORG)?;
        // An unknown role string grants nothing rather than something.
        let role = auth.claims.roles.iter().filter_map(|r| OrgRole::parse(r)).max().ok_or(ApiError::Forbidden)?;
        Ok(OrgContext {
            user_id: auth.user_id,
            session_id: auth.session_id,
            tenant_id,
            organization_id,
            role,
            email: auth.email,
            name: auth.name,
        })
    }
}
