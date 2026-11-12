use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", content = "id", rename_all = "snake_case")]
pub enum Actor {
    User(Uuid),
    System(String),
    Automation(Uuid),
}

/// The wire format of every event. `payload` is one of the versioned structs in
/// [`crate::payloads`]; consumers must tolerate unknown fields.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventEnvelope<P> {
    pub event_id: Uuid,
    pub event_type: String,
    pub schema_version: u32,
    /// `None` only for events about global (non-tenant) aggregates such as accounts.
    pub tenant_id: Option<Uuid>,
    pub aggregate_type: String,
    pub aggregate_id: Uuid,
    pub aggregate_version: i64,
    #[serde(with = "time::serde::rfc3339")]
    pub occurred_at: OffsetDateTime,
    pub correlation_id: Option<String>,
    pub causation_id: Option<Uuid>,
    pub actor: Actor,
    pub payload: P,
}

/// Implemented by every payload struct: binds it to its type name, version and topic so
/// producers cannot publish a payload under the wrong name.
pub trait EventPayload: Serialize {
    const EVENT_TYPE: &'static str;
    const SCHEMA_VERSION: u32;
    const TOPIC: &'static str;
    const AGGREGATE_TYPE: &'static str;
}

impl<P: EventPayload> EventEnvelope<P> {
    pub fn new(tenant_id: Option<Uuid>, aggregate_id: Uuid, aggregate_version: i64, actor: Actor, payload: P) -> Self {
        Self {
            event_id: platform_common::new_id(),
            event_type: P::EVENT_TYPE.to_owned(),
            schema_version: P::SCHEMA_VERSION,
            tenant_id,
            aggregate_type: P::AGGREGATE_TYPE.to_owned(),
            aggregate_id,
            aggregate_version,
            occurred_at: OffsetDateTime::now_utc(),
            correlation_id: platform_common::request_context::request_id(),
            causation_id: None,
            actor,
            payload,
        }
    }

    #[must_use]
    pub fn caused_by(mut self, event_id: Uuid) -> Self {
        self.causation_id = Some(event_id);
        self
    }

    /// Kafka key: keeps all events of one aggregate in order on one partition.
    pub fn partition_key(&self) -> String {
        match self.tenant_id {
            Some(tenant) => format!("{tenant}:{}", self.aggregate_id),
            None => self.aggregate_id.to_string(),
        }
    }

    pub fn topic(&self) -> &'static str {
        P::TOPIC
    }
}
