//! Authentication and coarse authorization shared by every service.
//!
//! * [`TokenVerifier`] verifies user access tokens (EdDSA) against the identity service's JWKS.
//! * [`AuthContext`] / [`OrgContext`] are Axum extractors that hand handlers a verified identity.
//! * [`authz`] evaluates organization-level permissions from signed role claims.
//! * [`service_auth`] authenticates service-to-service gRPC calls.
//!
//! The tenant a request operates on comes from the verified token and nowhere else.

pub mod authz;
pub mod claims;
pub mod extract;
pub mod keys;
pub mod policy;
pub mod redis_handle;
pub mod service_auth;
pub mod verifier;

pub use authz::{OrgPermission, OrgRole};
pub use claims::{Claims, TokenKind, ACCESS_TOKEN_TTL_SECS, API_AUDIENCE};
pub use extract::{AuthContext, OrgContext};
pub use keys::{generate_ed25519, Jwk, JwkSet, TokenSigner};
pub use policy::{
    policy_version_key, GrpcPolicySource, PolicyClient, PolicySnapshot, PolicySource, ProjectPermission, ProjectRole,
    StaticPolicy, VisibleProjects,
};
pub use redis_handle::RedisHandle;
pub use verifier::{revocation_key, TokenVerifier};
