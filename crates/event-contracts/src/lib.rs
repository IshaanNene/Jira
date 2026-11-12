//! Event envelope, topic names, versioned payloads and the transactional-outbox writer.
//! See docs/EVENT_ARCHITECTURE.md. Contracts only — no business rules.

pub mod envelope;
pub mod outbox;
pub mod payloads;
pub mod topics;

pub use envelope::{Actor, EventEnvelope};
