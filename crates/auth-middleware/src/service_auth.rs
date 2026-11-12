//! Service-to-service authentication for internal gRPC.
//!
//! Each call carries a 60-second HS256 token naming the caller (`iss`) and the intended
//! callee (`aud`), signed with a secret distributed only to platform workloads. In
//! Kubernetes this sits on top of mesh mTLS and NetworkPolicies; it exists so that
//! authorization of internal calls does not depend on network position alone.
//!
//! Trade-off (documented in SECURITY_DESIGN.md): a shared secret means any service
//! holding it could mint a token as another service. Per-workload identities (SPIFFE)
//! are the upgrade path.

use std::time::{SystemTime, UNIX_EPOCH};

use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use tonic::{metadata::MetadataValue, service::Interceptor, Request, Status};

const TTL_SECS: u64 = 60;

#[derive(Debug, Serialize, Deserialize)]
struct ServiceClaims {
    iss: String,
    aud: String,
    iat: u64,
    exp: u64,
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Client side: attaches the service token, request id and trace context.
#[derive(Clone)]
pub struct ServiceClientAuth {
    caller: &'static str,
    callee: &'static str,
    key: EncodingKey,
}

impl ServiceClientAuth {
    pub fn new(secret: &str, caller: &'static str, callee: &'static str) -> anyhow::Result<Self> {
        check_secret(secret)?;
        Ok(Self { caller, callee, key: EncodingKey::from_secret(secret.as_bytes()) })
    }
}

impl Interceptor for ServiceClientAuth {
    fn call(&mut self, mut request: Request<()>) -> Result<Request<()>, Status> {
        let iat = now();
        let claims = ServiceClaims { iss: self.caller.to_owned(), aud: self.callee.to_owned(), iat, exp: iat + TTL_SECS };
        let token = jsonwebtoken::encode(&Header::new(Algorithm::HS256), &claims, &self.key)
            .map_err(|_| Status::internal("could not sign service token"))?;
        let value = MetadataValue::try_from(format!("Bearer {token}"))
            .map_err(|_| Status::internal("service token is not valid metadata"))?;
        request.metadata_mut().insert("authorization", value);
        if let Some(id) = platform_common::request_context::request_id() {
            if let Ok(v) = MetadataValue::try_from(id) {
                request.metadata_mut().insert("x-request-id", v);
            }
        }
        observability::inject_into_metadata(request.metadata_mut());
        Ok(request)
    }
}

/// Server side: rejects calls without a valid token addressed to this service.
#[derive(Clone)]
pub struct ServiceServerAuth {
    key: DecodingKey,
    validation: Validation,
}

impl ServiceServerAuth {
    pub fn new(secret: &str, this_service: &'static str) -> anyhow::Result<Self> {
        check_secret(secret)?;
        let mut validation = Validation::new(Algorithm::HS256);
        validation.set_audience(&[this_service]);
        validation.leeway = 5;
        Ok(Self { key: DecodingKey::from_secret(secret.as_bytes()), validation })
    }

    /// Returns the calling service's name.
    pub fn verify(&self, token: &str) -> Result<String, Status> {
        jsonwebtoken::decode::<ServiceClaims>(token, &self.key, &self.validation)
            .map(|d| d.claims.iss)
            .map_err(|_| Status::unauthenticated("invalid service token"))
    }
}

impl Interceptor for ServiceServerAuth {
    fn call(&mut self, request: Request<()>) -> Result<Request<()>, Status> {
        let token = request
            .metadata()
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .ok_or_else(|| Status::unauthenticated("missing service token"))?;
        let caller = self.verify(token)?;
        tracing::debug!(caller, "internal call authenticated");
        Ok(request)
    }
}

/// Lazily connected channel with connect and per-request deadlines. Nothing is dialled
/// until the first call, so a service can start before its dependencies are up.
pub fn lazy_channel(url: &str) -> anyhow::Result<tonic::transport::Channel> {
    Ok(tonic::transport::Endpoint::from_shared(url.to_owned())?
        .connect_timeout(std::time::Duration::from_secs(2))
        .timeout(std::time::Duration::from_secs(3))
        .connect_lazy())
}

/// Whether a failed internal call is worth retrying (only ever for idempotent RPCs).
pub fn is_transient(status: &Status) -> bool {
    matches!(status.code(), tonic::Code::Unavailable | tonic::Code::DeadlineExceeded)
}

fn check_secret(secret: &str) -> anyhow::Result<()> {
    if secret.len() < 32 {
        anyhow::bail!("INTERNAL_SERVICE_SECRET must be at least 32 characters");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "test-secret-test-secret-test-secret-0000";

    fn token_from(client: &mut ServiceClientAuth) -> String {
        let req = client.call(Request::new(())).unwrap();
        req.metadata().get("authorization").unwrap().to_str().unwrap().trim_start_matches("Bearer ").to_owned()
    }

    #[test]
    fn accepts_token_for_this_service() {
        let mut client = ServiceClientAuth::new(SECRET, "identity-service", "user-service").unwrap();
        let server = ServiceServerAuth::new(SECRET, "user-service").unwrap();
        assert_eq!(server.verify(&token_from(&mut client)).unwrap(), "identity-service");
    }

    #[test]
    fn rejects_token_addressed_to_another_service() {
        let mut client = ServiceClientAuth::new(SECRET, "identity-service", "tenant-service").unwrap();
        let server = ServiceServerAuth::new(SECRET, "user-service").unwrap();
        assert!(server.verify(&token_from(&mut client)).is_err());
    }

    #[test]
    fn rejects_token_signed_with_other_secret() {
        let mut client =
            ServiceClientAuth::new("another-secret-another-secret-another-0", "identity-service", "user-service").unwrap();
        let server = ServiceServerAuth::new(SECRET, "user-service").unwrap();
        assert!(server.verify(&token_from(&mut client)).is_err());
    }

    #[test]
    fn missing_token_is_rejected() {
        let mut server = ServiceServerAuth::new(SECRET, "user-service").unwrap();
        assert!(server.call(Request::new(())).is_err());
    }

    #[test]
    fn short_secrets_are_refused() {
        assert!(ServiceServerAuth::new("short", "user-service").is_err());
    }
}
