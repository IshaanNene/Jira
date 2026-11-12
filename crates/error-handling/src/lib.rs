//! One error type for request handling. It renders as RFC 9457 `application/problem+json`
//! over HTTP and maps to/from `tonic::Status` for internal calls.
//!
//! Internal details (SQL errors, upstream messages) are logged, never returned to clients.

use axum::{
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use serde::Serialize;
use serde_json::{json, Value};

pub type ApiResult<T> = Result<T, ApiError>;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct FieldError {
    pub field: String,
    pub code: String,
    pub message: String,
}

impl FieldError {
    pub fn new(field: impl Into<String>, code: impl Into<String>, message: impl Into<String>) -> Self {
        Self { field: field.into(), code: code.into(), message: message.into() }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("unauthenticated: {0}")]
    Unauthenticated(&'static str),
    #[error("forbidden")]
    Forbidden,
    #[error("{0} not found")]
    NotFound(&'static str),
    #[error("validation failed")]
    Validation(Vec<FieldError>),
    #[error("version conflict")]
    VersionConflict { current: Option<Value> },
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("precondition required")]
    PreconditionRequired,
    #[error("rate limited")]
    RateLimited { retry_after_secs: u64 },
    #[error("tenant unavailable: {0}")]
    TenantUnavailable(&'static str),
    #[error("dependency unavailable: {0}")]
    DependencyUnavailable(&'static str),
    #[error("internal error: {0:#}")]
    Internal(#[from] anyhow::Error),
}

impl ApiError {
    pub fn invalid(field: &str, code: &str, message: &str) -> Self {
        ApiError::Validation(vec![FieldError::new(field, code, message)])
    }

    pub fn internal(err: impl Into<anyhow::Error>) -> Self {
        ApiError::Internal(err.into())
    }

    pub fn code(&self) -> &'static str {
        match self {
            ApiError::Unauthenticated(_) => "unauthenticated",
            ApiError::Forbidden => "forbidden",
            ApiError::NotFound(_) => "not_found",
            ApiError::Validation(_) => "validation_failed",
            ApiError::VersionConflict { .. } => "version_conflict",
            ApiError::Conflict(_) => "conflict",
            ApiError::PreconditionRequired => "precondition_required",
            ApiError::RateLimited { .. } => "rate_limited",
            ApiError::TenantUnavailable(_) => "tenant_unavailable",
            ApiError::DependencyUnavailable(_) => "dependency_unavailable",
            ApiError::Internal(_) => "internal",
        }
    }

    pub fn status(&self) -> StatusCode {
        match self {
            ApiError::Unauthenticated(_) => StatusCode::UNAUTHORIZED,
            ApiError::Forbidden => StatusCode::FORBIDDEN,
            ApiError::NotFound(_) => StatusCode::NOT_FOUND,
            ApiError::Validation(_) => StatusCode::UNPROCESSABLE_ENTITY,
            ApiError::VersionConflict { .. } | ApiError::Conflict(_) => StatusCode::CONFLICT,
            ApiError::PreconditionRequired => StatusCode::PRECONDITION_REQUIRED,
            ApiError::RateLimited { .. } => StatusCode::TOO_MANY_REQUESTS,
            ApiError::TenantUnavailable(_) | ApiError::DependencyUnavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
            ApiError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    fn title(&self) -> &'static str {
        match self {
            ApiError::Unauthenticated(_) => "Authentication required",
            ApiError::Forbidden => "Not allowed",
            ApiError::NotFound(_) => "Not found",
            ApiError::Validation(_) => "Validation failed",
            ApiError::VersionConflict { .. } => "Edit conflict",
            ApiError::Conflict(_) => "Conflict",
            ApiError::PreconditionRequired => "Precondition required",
            ApiError::RateLimited { .. } => "Too many requests",
            ApiError::TenantUnavailable(_) => "Workspace temporarily unavailable",
            ApiError::DependencyUnavailable(_) => "Service temporarily unavailable",
            ApiError::Internal(_) => "Internal error",
        }
    }

    /// Client-safe detail. Never includes internal error text.
    fn detail(&self) -> String {
        match self {
            ApiError::Unauthenticated(reason) => (*reason).to_owned(),
            ApiError::Forbidden => "You do not have permission to perform this action.".to_owned(),
            ApiError::NotFound(what) => format!("The requested {what} does not exist."),
            ApiError::Validation(_) => "One or more fields are invalid.".to_owned(),
            ApiError::VersionConflict { .. } => {
                "The resource was changed by someone else. Review the latest version and retry.".to_owned()
            }
            ApiError::Conflict(msg) => msg.clone(),
            ApiError::PreconditionRequired => "This request requires an If-Match header.".to_owned(),
            ApiError::RateLimited { .. } => "Slow down and retry shortly.".to_owned(),
            ApiError::TenantUnavailable(reason) => (*reason).to_owned(),
            ApiError::DependencyUnavailable(_) => "A required dependency is unavailable. Retry shortly.".to_owned(),
            ApiError::Internal(_) => "An unexpected error occurred.".to_owned(),
        }
    }

    pub fn problem(&self) -> Value {
        let mut body = json!({
            "type": format!("https://errors.platform.dev/{}", self.code()),
            "title": self.title(),
            "status": self.status().as_u16(),
            "code": self.code(),
            "detail": self.detail(),
        });
        if let Some(request_id) = platform_common::request_context::request_id() {
            body["request_id"] = Value::String(request_id);
        }
        match self {
            ApiError::Validation(errors) => body["errors"] = json!(errors),
            ApiError::VersionConflict { current: Some(current) } => body["current"] = current.clone(),
            _ => {}
        }
        body
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        match &self {
            ApiError::Internal(err) => {
                tracing::error!(error = %format!("{err:#}"), "request failed")
            }
            ApiError::DependencyUnavailable(dep) => {
                tracing::warn!(dependency = dep, "dependency unavailable")
            }
            _ => {}
        }
        let mut response = (self.status(), axum::Json(self.problem())).into_response();
        response.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("application/problem+json"));
        match self {
            ApiError::RateLimited { retry_after_secs } => {
                response.headers_mut().insert(header::RETRY_AFTER, HeaderValue::from(retry_after_secs));
            }
            ApiError::TenantUnavailable(_) | ApiError::DependencyUnavailable(_) => {
                response.headers_mut().insert(header::RETRY_AFTER, HeaderValue::from(5_u64));
            }
            _ => {}
        }
        response
    }
}

impl From<sqlx::Error> for ApiError {
    fn from(err: sqlx::Error) -> Self {
        match &err {
            sqlx::Error::RowNotFound => ApiError::NotFound("resource"),
            sqlx::Error::PoolTimedOut | sqlx::Error::PoolClosed | sqlx::Error::Io(_) => {
                tracing::warn!(error = %err, "database unavailable");
                ApiError::DependencyUnavailable("database")
            }
            sqlx::Error::Database(db) if db.is_unique_violation() => {
                ApiError::Conflict("A resource with the same unique value already exists.".to_owned())
            }
            _ => ApiError::Internal(err.into()),
        }
    }
}

impl From<validator::ValidationErrors> for ApiError {
    fn from(errors: validator::ValidationErrors) -> Self {
        let mut out = Vec::new();
        for (field, errs) in errors.field_errors() {
            for e in errs {
                let message = e.message.as_ref().map_or_else(|| format!("{field} is invalid"), ToString::to_string);
                out.push(FieldError::new(field.to_string(), e.code.to_string(), message));
            }
        }
        out.sort_by(|a, b| a.field.cmp(&b.field));
        ApiError::Validation(out)
    }
}

impl From<ApiError> for tonic::Status {
    fn from(err: ApiError) -> Self {
        use tonic::Code;
        let code = match &err {
            ApiError::Unauthenticated(_) => Code::Unauthenticated,
            ApiError::Forbidden => Code::PermissionDenied,
            ApiError::NotFound(_) => Code::NotFound,
            ApiError::Validation(_) => Code::InvalidArgument,
            ApiError::VersionConflict { .. } => Code::Aborted,
            ApiError::Conflict(_) => Code::AlreadyExists,
            ApiError::PreconditionRequired => Code::FailedPrecondition,
            ApiError::RateLimited { .. } => Code::ResourceExhausted,
            ApiError::TenantUnavailable(_) | ApiError::DependencyUnavailable(_) => Code::Unavailable,
            ApiError::Internal(e) => {
                tracing::error!(error = %format!("{e:#}"), "rpc failed");
                Code::Internal
            }
        };
        tonic::Status::new(code, err.detail())
    }
}

/// Read the version a client based its edit on from `If-Match: "<version>"`.
/// Missing header → 428 so that clients cannot overwrite blindly.
pub fn require_if_match(headers: &http::HeaderMap) -> ApiResult<i64> {
    let raw = headers.get(http::header::IF_MATCH).ok_or(ApiError::PreconditionRequired)?;
    raw.to_str()
        .ok()
        .map(|v| v.trim().trim_start_matches("W/").trim_matches('"'))
        .and_then(|v| v.parse::<i64>().ok())
        .ok_or_else(|| ApiError::invalid("If-Match", "invalid_version", "If-Match must carry the resource version."))
}

/// Translate a failed internal call into what our own caller should see.
/// `dependency` names the callee for logs and metrics.
pub fn from_status(status: &tonic::Status, dependency: &'static str) -> ApiError {
    use tonic::Code;
    match status.code() {
        Code::NotFound => ApiError::NotFound("resource"),
        Code::PermissionDenied => ApiError::Forbidden,
        Code::AlreadyExists => ApiError::Conflict(status.message().to_owned()),
        Code::InvalidArgument => ApiError::invalid("request", "invalid", status.message()),
        Code::Unavailable | Code::DeadlineExceeded | Code::Cancelled | Code::ResourceExhausted => {
            tracing::warn!(dependency, code = ?status.code(), "internal call unavailable");
            ApiError::DependencyUnavailable(dependency)
        }
        code => ApiError::Internal(anyhow::anyhow!("{dependency} returned {code:?}: {}", status.message())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn internal_errors_do_not_leak_details() {
        let err = ApiError::internal(anyhow::anyhow!("password=hunter2 connection string leaked"));
        let body = err.problem().to_string();
        assert!(!body.contains("hunter2"));
        assert_eq!(err.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn validation_lists_fields() {
        let err = ApiError::invalid("email", "invalid_email", "Enter a valid email address.");
        let body = err.problem();
        assert_eq!(body["code"], "validation_failed");
        assert_eq!(body["errors"][0]["field"], "email");
        assert_eq!(err.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[test]
    fn conflict_carries_current_entity() {
        let err = ApiError::VersionConflict { current: Some(json!({ "version": 3 })) };
        assert_eq!(err.problem()["current"]["version"], 3);
    }

    #[tokio::test]
    async fn request_id_is_included_when_in_scope() {
        let body = platform_common::request_context::scope("req-1".into(), async { ApiError::Forbidden.problem() }).await;
        assert_eq!(body["request_id"], "req-1");
    }

    #[test]
    fn if_match_parsing() {
        let mut h = http::HeaderMap::new();
        assert!(matches!(require_if_match(&h), Err(ApiError::PreconditionRequired)));
        h.insert(http::header::IF_MATCH, "\"7\"".parse().unwrap());
        assert_eq!(require_if_match(&h).unwrap(), 7);
        h.insert(http::header::IF_MATCH, "banana".parse().unwrap());
        assert!(matches!(require_if_match(&h), Err(ApiError::Validation(_))));
    }

    #[test]
    fn grpc_round_trip_keeps_semantics() {
        let status: tonic::Status = ApiError::NotFound("tenant").into();
        assert!(matches!(from_status(&status, "tenant-service"), ApiError::NotFound(_)));
        let down = tonic::Status::unavailable("x");
        assert!(matches!(from_status(&down, "tenant-service"), ApiError::DependencyUnavailable("tenant-service")));
    }
}
