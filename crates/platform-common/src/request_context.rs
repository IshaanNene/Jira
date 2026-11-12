//! Per-request correlation id, available anywhere on the request's task without
//! threading it through every function signature.

use std::future::Future;

tokio::task_local! {
    static REQUEST_ID: String;
}

pub async fn scope<F: Future>(request_id: String, fut: F) -> F::Output {
    REQUEST_ID.scope(request_id, fut).await
}

/// The current request id, if the caller is running inside a request scope.
pub fn request_id() -> Option<String> {
    REQUEST_ID.try_with(Clone::clone).ok()
}
