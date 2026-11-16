//! Moving events between services: the outbox relay (database → Kafka) and the consumer
//! loop (Kafka → handler, with retries and a dead-letter topic). See
//! docs/EVENT_ARCHITECTURE.md §3–4.

pub mod consumer;
pub mod inbox;
pub mod relay;
pub mod replay;
pub mod settings;

pub use consumer::{run_batch_consumer, run_consumer, BatchHandler, Batching, Event, Handler, HandlerError};
pub use relay::{spawn_relay, OutboxRelay};
pub use settings::KafkaSettings;
