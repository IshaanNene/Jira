//! Cross-cutting plumbing shared by every service. No business rules live here.

pub mod config;
pub mod net;
pub mod pagination;
pub mod request_context;
pub mod resilience;
pub mod server;

/// Time-ordered identifier (UUIDv7) used for every primary key and event id.
pub fn new_id() -> uuid::Uuid {
    uuid::Uuid::now_v7()
}
