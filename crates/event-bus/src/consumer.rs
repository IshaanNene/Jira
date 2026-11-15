//! Consumer loop: Kafka → handler, with retries and a dead-letter topic.
//!
//! Offsets are committed only after the handler succeeded or the event was parked in
//! `<topic>.dlq`, so nothing is skipped silently. Events are processed one at a time per
//! consumer instance, or in batches with [`run_batch_consumer`]; parallelism comes from more
//! instances in the group (one per partition at most).
//!
//! A transient failure (a dependency is down) is retried until it succeeds, with backoff up
//! to 30 s: the consumer pauses rather than skip ahead, so a projection never silently misses
//! a change made during an outage. Only events that can never succeed (invalid envelope,
//! payload against its contract, a permanent handler error) go to the dead-letter topic.

use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use error_handling::ApiError;
use event_contracts::{topics, EventEnvelope};
use rdkafka::{
    consumer::{CommitMode, Consumer, StreamConsumer},
    message::{BorrowedMessage, Header, Headers, OwnedHeaders, OwnedMessage},
    producer::{FutureProducer, FutureRecord},
    util::Timeout,
    Message, Offset, TopicPartitionList,
};
use serde::de::DeserializeOwned;
use uuid::Uuid;

use crate::settings::KafkaSettings;

/// Backoff for transient failures: 200 ms, doubling, at most 30 s between attempts.
const FIRST_RETRY: Duration = Duration::from_millis(200);
const MAX_RETRY: Duration = Duration::from_secs(30);

/// A received event: the validated envelope with its payload still as JSON.
#[derive(Debug, Clone)]
pub struct Event {
    pub topic: String,
    pub envelope: EventEnvelope<serde_json::Value>,
}

impl Event {
    pub fn event_type(&self) -> &str {
        &self.envelope.event_type
    }

    pub fn event_id(&self) -> Uuid {
        self.envelope.event_id
    }

    /// The tenant, which every event on a tenant topic must carry.
    pub fn tenant_id(&self) -> Result<Uuid, HandlerError> {
        self.envelope.tenant_id.ok_or_else(|| HandlerError::Permanent(anyhow::anyhow!("event has no tenant_id")))
    }

    /// The acting user, if a person caused the event.
    pub fn actor_user(&self) -> Option<Uuid> {
        match self.envelope.actor {
            event_contracts::Actor::User(id) => Some(id),
            _ => None,
        }
    }

    /// The payload as a versioned struct. Unknown fields are ignored; a payload that does
    /// not match its contract is a permanent failure.
    pub fn payload<P: DeserializeOwned>(&self) -> Result<P, HandlerError> {
        serde_json::from_value(self.envelope.payload.clone()).map_err(|e| {
            HandlerError::Permanent(anyhow::anyhow!("{} payload does not match its contract: {e}", self.event_type()))
        })
    }

    /// Parse and validate raw bytes from `topic`.
    pub fn parse(topic: &str, bytes: &[u8]) -> Result<Self, anyhow::Error> {
        let envelope: EventEnvelope<serde_json::Value> = serde_json::from_slice(bytes)?;
        if envelope.event_type.is_empty() || envelope.aggregate_type.is_empty() {
            anyhow::bail!("envelope without event_type or aggregate_type");
        }
        // Everything except identity events is about a tenant.
        if topic != topics::IDENTITY && envelope.tenant_id.is_none() {
            anyhow::bail!("{} event on {topic} has no tenant_id", envelope.event_type);
        }
        Ok(Self { topic: topic.to_owned(), envelope })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum HandlerError {
    /// Worth retrying: a dependency was down, a deadlock, a timeout.
    #[error("transient: {0}")]
    Transient(anyhow::Error),
    /// Retrying cannot help: the event is invalid for this consumer.
    #[error("permanent: {0}")]
    Permanent(anyhow::Error),
}

impl From<ApiError> for HandlerError {
    fn from(error: ApiError) -> Self {
        match error {
            ApiError::Validation(_) | ApiError::NotFound(_) | ApiError::Forbidden | ApiError::ForbiddenBecause { .. } => {
                Self::Permanent(anyhow::anyhow!("{error}"))
            }
            other => Self::Transient(anyhow::anyhow!("{other}")),
        }
    }
}

impl From<sqlx::Error> for HandlerError {
    fn from(error: sqlx::Error) -> Self {
        Self::Transient(error.into())
    }
}

#[async_trait]
pub trait Handler: Send + Sync + 'static {
    /// Apply the event. Ignore event types this consumer does not care about (`Ok`).
    async fn handle(&self, event: &Event) -> Result<(), HandlerError>;
}

/// A handler that applies events in batches, for stores that want few large writes (ClickHouse
/// makes one data part per insert).
#[async_trait]
pub trait BatchHandler: Send + Sync + 'static {
    /// Apply every event of the batch. `Ok` lists the events (by index) that can never succeed;
    /// they are dead-lettered and the rest count as applied. `Err` is a transient failure: the
    /// whole batch is retried, so applying an event twice must be harmless.
    async fn handle_batch(&self, events: &[Event]) -> Result<Vec<(usize, anyhow::Error)>, anyhow::Error>;
}

/// How large a batch may grow, and how long the first event of a batch may wait for company.
#[derive(Debug, Clone, Copy)]
pub struct Batching {
    pub max_events: usize,
    pub max_wait: Duration,
}

fn subscribe(settings: &KafkaSettings, group: &str, topics: &[&str]) -> anyhow::Result<StreamConsumer> {
    let consumer: StreamConsumer = settings
        .base()
        .set("group.id", group)
        .set("enable.auto.commit", "false")
        .set("auto.offset.reset", "earliest")
        .set("session.timeout.ms", "10000")
        .create()?;
    consumer.subscribe(topics)?;
    Ok(consumer)
}

/// Like [`run_consumer`], but hands the handler batches of up to `batching.max_events` events.
/// Offsets are committed after the whole batch is applied or parked, with the same guarantees.
pub async fn run_batch_consumer(
    settings: &KafkaSettings,
    group: &str,
    topics: &[&str],
    handler: Arc<dyn BatchHandler>,
    batching: Batching,
) -> anyhow::Result<()> {
    let consumer = subscribe(settings, group, topics)?;
    let dlq: FutureProducer = settings.producer().create()?;
    tracing::info!(group, ?topics, max_events = batching.max_events, "batch consumer started");

    loop {
        let batch = collect(&consumer, group, batching).await;
        let mut events = Vec::with_capacity(batch.len());
        let mut origin = Vec::with_capacity(batch.len());
        for (i, message) in batch.iter().enumerate() {
            match message.payload().map(|bytes| Event::parse(message.topic(), bytes)) {
                Some(Ok(event)) => {
                    events.push(event);
                    origin.push(i);
                }
                Some(Err(error)) => park(&dlq, group, message, &format!("invalid envelope: {error}"), 0).await,
                None => park(&dlq, group, message, "empty message", 0).await,
            }
        }
        if !events.is_empty() {
            let rejected = handle_batch_with_retry(handler.as_ref(), &events, group, FIRST_RETRY, MAX_RETRY).await;
            for (index, error) in rejected {
                if let Some(&message) = origin.get(index) {
                    park(&dlq, group, &batch[message], &error.to_string(), 1).await;
                }
            }
        }
        // One commit per batch: the next offset of every partition it touched.
        let mut next: std::collections::BTreeMap<(&str, i32), i64> = Default::default();
        for message in &batch {
            let offset = next.entry((message.topic(), message.partition())).or_insert(0);
            *offset = (*offset).max(message.offset() + 1);
        }
        let mut offsets = TopicPartitionList::new();
        for ((topic, partition), offset) in next {
            if let Err(error) = offsets.add_partition_offset(topic, partition, Offset::Offset(offset)) {
                tracing::warn!(group, %error, "could not record an offset to commit");
            }
        }
        if let Err(error) = consumer.commit(&offsets, CommitMode::Async) {
            tracing::warn!(group, %error, "offset commit failed; the batch may be redelivered");
        }
    }
}

/// Wait for one message, then gather more until the batch is full or `max_wait` has passed.
async fn collect(consumer: &StreamConsumer, group: &str, batching: Batching) -> Vec<OwnedMessage> {
    let first = loop {
        match consumer.recv().await {
            Ok(m) => break m.detach(),
            Err(error) => {
                tracing::warn!(group, %error, "kafka receive failed");
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
    };
    let mut batch = vec![first];
    let deadline = tokio::time::Instant::now() + batching.max_wait;
    while batch.len() < batching.max_events {
        match tokio::time::timeout_at(deadline, consumer.recv()).await {
            Ok(Ok(m)) => batch.push(m.detach()),
            Ok(Err(error)) => {
                tracing::warn!(group, %error, "kafka receive failed");
                break;
            }
            Err(_) => break,
        }
    }
    batch
}

/// Run the batch handler until it succeeds; returns the events it rejected for good.
async fn handle_batch_with_retry(
    handler: &dyn BatchHandler,
    events: &[Event],
    group: &str,
    first_retry: Duration,
    max_retry: Duration,
) -> Vec<(usize, anyhow::Error)> {
    let mut delay = first_retry;
    let mut attempt: u32 = 1;
    loop {
        match handler.handle_batch(events).await {
            Ok(rejected) => {
                for (i, event) in events.iter().enumerate() {
                    if !rejected.iter().any(|(r, _)| *r == i) {
                        metrics::counter!("events_consumed_total", "event_type" => event.event_type().to_owned()).increment(1);
                    }
                }
                if attempt > 1 {
                    tracing::info!(consumer = group, events = events.len(), attempt, "batch handled after retrying");
                }
                return rejected;
            }
            Err(error) => {
                metrics::counter!("events_retried_total", "consumer" => group.to_owned()).increment(events.len() as u64);
                if attempt <= 5 || delay == max_retry && attempt.is_multiple_of(2) {
                    tracing::warn!(consumer = group, events = events.len(), attempt, %error,
                        "batch handling failed; retrying (the consumer waits until it succeeds)");
                }
                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(max_retry);
                attempt = attempt.saturating_add(1);
            }
        }
    }
}

/// Consume `topics` as consumer group `group` until the process stops. Connection problems
/// are retried by the client; this never returns under normal operation.
pub async fn run_consumer(
    settings: &KafkaSettings,
    group: &str,
    topics: &[&str],
    handler: Arc<dyn Handler>,
) -> anyhow::Result<()> {
    let consumer = subscribe(settings, group, topics)?;
    let dlq: FutureProducer = settings.producer().create()?;
    tracing::info!(group, ?topics, "consumer started");

    loop {
        let message = match consumer.recv().await {
            Ok(m) => m,
            Err(error) => {
                tracing::warn!(group, %error, "kafka receive failed");
                tokio::time::sleep(Duration::from_secs(1)).await;
                continue;
            }
        };
        let outcome = process(&message, handler.as_ref(), group).await;
        if let Err((error, attempts)) = outcome {
            park(&dlq, group, &message, &error, attempts).await;
        }
        if let Err(error) = consumer.commit_message(&message, CommitMode::Async) {
            tracing::warn!(group, %error, "offset commit failed; the event may be redelivered");
        }
    }
}

async fn process(message: &BorrowedMessage<'_>, handler: &dyn Handler, group: &str) -> Result<(), (String, u32)> {
    let topic = message.topic();
    let event = match message.payload().map(|bytes| Event::parse(topic, bytes)) {
        Some(Ok(event)) => event,
        Some(Err(error)) => return Err((format!("invalid envelope: {error}"), 0)),
        None => return Err(("empty message".to_owned(), 0)),
    };
    handle_with_retry(handler, &event, group, FIRST_RETRY, MAX_RETRY).await
}

/// Run the handler until it succeeds or fails permanently. Transient failures are retried
/// without limit; `events_retried_total` counts them so a stalled consumer is visible.
async fn handle_with_retry(
    handler: &dyn Handler,
    event: &Event,
    group: &str,
    first_retry: Duration,
    max_retry: Duration,
) -> Result<(), (String, u32)> {
    let mut delay = first_retry;
    let mut attempt: u32 = 1;
    loop {
        match handler.handle(event).await {
            Ok(()) => {
                metrics::counter!("events_consumed_total", "event_type" => event.event_type().to_owned()).increment(1);
                if attempt > 1 {
                    tracing::info!(consumer = group, event_id = %event.event_id(), attempt, "event handled after retrying");
                }
                return Ok(());
            }
            Err(HandlerError::Permanent(error)) => return Err((error.to_string(), attempt)),
            Err(HandlerError::Transient(error)) => {
                metrics::counter!("events_retried_total", "consumer" => group.to_owned()).increment(1);
                // Every attempt while it is short, then once a minute or so, to keep logs readable.
                if attempt <= 5 || delay == max_retry && attempt.is_multiple_of(2) {
                    tracing::warn!(consumer = group, event_type = event.event_type(), event_id = %event.event_id(), attempt, %error,
                        "event handling failed; retrying (the consumer waits until it succeeds)");
                }
                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(max_retry);
                attempt = attempt.saturating_add(1);
            }
        }
    }
}

/// Park a failed event in `<topic>.dlq`. Blocks (retrying) rather than lose it.
async fn park<M: Message>(dlq: &FutureProducer, group: &str, message: &M, error: &str, attempts: u32) {
    let topic = topics::dead_letter(message.topic());
    let attempts = attempts.to_string();
    let mut headers = OwnedHeaders::new();
    if let Some(original) = message.headers() {
        for h in original.iter() {
            headers = headers.insert(Header { key: h.key, value: h.value });
        }
    }
    let headers = headers
        .insert(Header { key: "x-error", value: Some(error) })
        .insert(Header { key: "x-consumer", value: Some(group) })
        .insert(Header { key: "x-attempts", value: Some(attempts.as_str()) })
        .insert(Header { key: "x-original-topic", value: Some(message.topic()) });
    let mut wait = Duration::from_secs(1);
    loop {
        let mut record = FutureRecord::to(&topic).headers(headers.clone());
        if let Some(key) = message.key() {
            record = record.key(key);
        }
        if let Some(payload) = message.payload() {
            record = record.payload(payload);
        }
        match dlq.send(record, Timeout::After(Duration::from_secs(30))).await {
            Ok(_) => {
                metrics::counter!("events_dead_lettered_total", "consumer" => group.to_owned()).increment(1);
                tracing::error!(consumer = group, dlq = %topic, %error, "event moved to the dead-letter topic");
                return;
            }
            Err((send_error, _)) => {
                tracing::error!(%send_error, "could not park event in the dead-letter topic; retrying");
                tokio::time::sleep(wait).await;
                wait = (wait * 2).min(Duration::from_secs(30));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;

    /// Fails transiently `outage` times, then succeeds; or always fails permanently.
    struct Flaky {
        outage: u32,
        permanent: bool,
        calls: AtomicU32,
    }

    #[async_trait]
    impl Handler for Flaky {
        async fn handle(&self, _: &Event) -> Result<(), HandlerError> {
            let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
            if self.permanent {
                Err(HandlerError::Permanent(anyhow::anyhow!("bad payload")))
            } else if call <= self.outage {
                Err(HandlerError::Transient(anyhow::anyhow!("index unavailable")))
            } else {
                Ok(())
            }
        }
    }

    fn sample_event() -> Event {
        let raw = r#"{"event_id":"0195c1e2-0000-7000-8000-000000000001","event_type":"issue.created","schema_version":1,
            "tenant_id":"0195c1e2-0000-7000-8000-000000000002","aggregate_type":"issue","aggregate_id":"0195c1e2-0000-7000-8000-000000000003",
            "aggregate_version":1,"occurred_at":"2026-11-11T09:00:00Z","correlation_id":null,"causation_id":null,
            "actor":{"type":"system","id":"test"},"payload":{}}"#;
        Event::parse(topics::ISSUE, raw.as_bytes()).unwrap()
    }

    #[tokio::test]
    async fn an_outage_is_waited_out_instead_of_dead_lettering_the_event() {
        let event = sample_event();
        // Far more failures than the old three attempts: the event is still handled.
        let flaky = Flaky { outage: 12, permanent: false, calls: AtomicU32::new(0) };
        let outcome = handle_with_retry(&flaky, &event, "test", Duration::from_millis(1), Duration::from_millis(4)).await;
        assert!(outcome.is_ok());
        assert_eq!(flaky.calls.load(Ordering::SeqCst), 13);

        // A permanent failure goes to the dead-letter topic at once.
        let broken = Flaky { outage: 0, permanent: true, calls: AtomicU32::new(0) };
        let outcome = handle_with_retry(&broken, &event, "test", Duration::from_millis(1), Duration::from_millis(4)).await;
        assert_eq!(outcome.unwrap_err().1, 1);
        assert_eq!(broken.calls.load(Ordering::SeqCst), 1);
    }

    /// Fails the whole batch transiently `outage` times, then rejects event 1 for good.
    struct FlakyBatch {
        outage: u32,
        calls: AtomicU32,
    }

    #[async_trait]
    impl BatchHandler for FlakyBatch {
        async fn handle_batch(&self, _: &[Event]) -> Result<Vec<(usize, anyhow::Error)>, anyhow::Error> {
            if self.calls.fetch_add(1, Ordering::SeqCst) < self.outage {
                Err(anyhow::anyhow!("store unavailable"))
            } else {
                Ok(vec![(1, anyhow::anyhow!("bad payload"))])
            }
        }
    }

    #[tokio::test]
    async fn a_batch_is_retried_whole_and_only_rejected_events_are_dead_lettered() {
        let events = vec![sample_event(), sample_event(), sample_event()];
        let handler = FlakyBatch { outage: 6, calls: AtomicU32::new(0) };
        let rejected =
            handle_batch_with_retry(&handler, &events, "test", Duration::from_millis(1), Duration::from_millis(4)).await;
        assert_eq!(handler.calls.load(Ordering::SeqCst), 7);
        assert_eq!(rejected.iter().map(|(i, _)| *i).collect::<Vec<_>>(), [1]);
    }

    #[test]
    fn envelopes_without_a_tenant_are_rejected_on_tenant_topics() {
        let raw = |tenant: &str| {
            format!(
                r#"{{"event_id":"0195c1e2-0000-7000-8000-000000000001","event_type":"issue.created","schema_version":1,
                "tenant_id":{tenant},"aggregate_type":"issue","aggregate_id":"0195c1e2-0000-7000-8000-000000000003",
                "aggregate_version":1,"occurred_at":"2026-11-11T09:00:00Z","correlation_id":null,"causation_id":null,
                "actor":{{"type":"user","id":"0195c1e2-0000-7000-8000-000000000004"}},"payload":{{"x":1}},"extra":true}}"#
            )
        };
        assert!(Event::parse(topics::ISSUE, raw("null").as_bytes()).is_err());
        let event = Event::parse(topics::ISSUE, raw("\"0195c1e2-0000-7000-8000-000000000002\"").as_bytes()).unwrap();
        assert_eq!(event.actor_user().unwrap().to_string(), "0195c1e2-0000-7000-8000-000000000004");
        assert!(event.tenant_id().is_ok());
        assert!(Event::parse(topics::ISSUE, b"not json").is_err());
        // An identity event needs no tenant.
        assert!(Event::parse(topics::IDENTITY, raw("null").as_bytes()).is_ok());
    }
}
