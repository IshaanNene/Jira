//! Replaying dead-lettered events once whatever made them fail has been fixed.
//!
//! A consumer parks an event it can never handle in `<topic>.dlq` (see [`crate::consumer`]),
//! with headers saying which consumer group failed, how often, and why. Replaying sends the
//! group's parked events back to the topic they came from, marked with
//! [`REPLAY_FOR`](crate::consumer::REPLAY_FOR) so that only that group handles them again;
//! every other group skips them. An event that still fails is parked again.
//!
//! Progress is kept as the offsets of a consumer group of its own per failing group,
//! `dlq-replay.<group>`, committed after each event is re-sent: a replay that stops half way
//! resumes where it stopped, and an event is never re-sent twice by the tool. Events are
//! re-sent in the order they were parked (per partition); `limit` stops after that many.
//!
//! Only metadata is reported (offsets, event id and type, tenant, attempts, error), never
//! the payload.

use std::{collections::HashMap, time::Duration};

use event_contracts::topics;
use rdkafka::{
    consumer::{BaseConsumer, CommitMode, Consumer},
    message::{Header, Headers, OwnedHeaders},
    producer::{FutureProducer, FutureRecord},
    util::Timeout,
    Message, Offset, TopicPartitionList,
};
use uuid::Uuid;

use crate::{consumer::REPLAY_FOR, settings::KafkaSettings};

const TIMEOUT: Duration = Duration::from_secs(10);
/// Longest error text reported per event.
const MAX_ERROR: usize = 300;

/// One parked event of the group, described without its payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parked {
    pub partition: i32,
    pub offset: i64,
    pub event_id: Option<Uuid>,
    pub event_type: Option<String>,
    pub tenant_id: Option<Uuid>,
    pub attempts: Option<u32>,
    pub error: String,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Replayed {
    /// Events re-sent to the original topic for the group.
    pub sent: Vec<Parked>,
    /// The group's parked events still waiting after this run (beyond `limit`).
    pub remaining: usize,
}

/// `platform.issue.v1` or `platform.issue.v1.dlq` → (`platform.issue.v1`, `platform.issue.v1.dlq`).
/// The topic events go back to is derived from the dead-letter topic's own name, never taken
/// from a message header.
pub fn topics_for(topic: &str) -> anyhow::Result<(String, String)> {
    let original = topic.strip_suffix(".dlq").unwrap_or(topic);
    let valid = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-');
    if original.is_empty() || original.ends_with(".dlq") || !original.chars().all(valid) {
        anyhow::bail!("not a topic name: {topic}");
    }
    Ok((original.to_owned(), topics::dead_letter(original)))
}

fn valid_group(group: &str) -> anyhow::Result<()> {
    let valid = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-');
    if group.is_empty() || group.len() > 200 || !group.chars().all(valid) {
        anyhow::bail!("not a consumer group name: {group}");
    }
    Ok(())
}

fn header<'a, M: Message>(message: &'a M, key: &str) -> Option<&'a str> {
    let headers = message.headers()?;
    headers.iter().find(|h| h.key == key).and_then(|h| h.value).and_then(|v| std::str::from_utf8(v).ok())
}

fn describe<M: Message>(message: &M) -> Parked {
    let envelope: Option<serde_json::Value> = message.payload().and_then(|p| serde_json::from_slice(p).ok());
    let field = |name: &str| envelope.as_ref().and_then(|e| e.get(name)).and_then(|v| v.as_str());
    let mut error: String = header(message, "x-error").unwrap_or("").chars().filter(|c| !c.is_control()).collect();
    if error.len() > MAX_ERROR {
        let mut end = MAX_ERROR;
        while !error.is_char_boundary(end) {
            end -= 1;
        }
        error.truncate(end);
        error.push('…');
    }
    Parked {
        partition: message.partition(),
        offset: message.offset(),
        event_id: field("event_id").and_then(|v| Uuid::parse_str(v).ok()),
        event_type: field("event_type").map(|t| t.chars().take(100).collect()),
        tenant_id: field("tenant_id").and_then(|v| Uuid::parse_str(v).ok()),
        attempts: header(message, "x-attempts").and_then(|a| a.parse().ok()),
        error,
    }
}

/// A reader of the dead-letter topic as `dlq-replay.<group>`, positioned where the last
/// replay stopped, with each partition's end as it was when it was opened.
struct Reader {
    consumer: BaseConsumer,
    dlq: String,
    /// partition → offset one past the last message present at the start.
    ends: HashMap<i32, i64>,
    /// Partitions with nothing left to read when the reader was opened.
    finished: Vec<i32>,
}

impl Reader {
    fn open(settings: &KafkaSettings, dlq: &str, group: &str) -> anyhow::Result<Self> {
        let consumer: BaseConsumer = settings
            .base()
            .set("group.id", format!("dlq-replay.{group}"))
            .set("enable.auto.commit", "false")
            .set("auto.offset.reset", "earliest")
            .create()?;
        let metadata = consumer.fetch_metadata(Some(dlq), TIMEOUT)?;
        let partitions: Vec<i32> = metadata
            .topics()
            .iter()
            .filter(|t| t.name() == dlq && t.error().is_none())
            .flat_map(|t| t.partitions().iter().map(|p| p.id()))
            .collect();
        let mut wanted = TopicPartitionList::new();
        let mut marks = HashMap::new();
        for &partition in &partitions {
            let (low, high) = consumer.fetch_watermarks(dlq, partition, TIMEOUT)?;
            wanted.add_partition(dlq, partition);
            marks.insert(partition, (low, high.max(low)));
        }
        let (mut start, mut finished) = (TopicPartitionList::new(), Vec::new());
        if !partitions.is_empty() {
            let committed = consumer.committed_offsets(wanted, TIMEOUT)?;
            for element in committed.elements() {
                let (low, high) = marks.get(&element.partition()).copied().unwrap_or((0, 0));
                // Where the last replay stopped; the start when there was none, or when what
                // it stopped at has since been deleted by retention.
                let from = match element.offset() {
                    Offset::Offset(o) if o >= low => o,
                    _ => low,
                };
                if from >= high {
                    finished.push(element.partition());
                }
                start.add_partition_offset(dlq, element.partition(), Offset::Offset(from))?;
            }
        }
        consumer.assign(&start)?;
        let ends = marks.into_iter().map(|(partition, (_, high))| (partition, high)).collect();
        Ok(Self { consumer, dlq: dlq.to_owned(), ends, finished })
    }

    /// Every message present when the reader was opened, from where the group stopped.
    fn for_each(
        &self,
        mut visit: impl FnMut(&rdkafka::message::BorrowedMessage<'_>) -> anyhow::Result<bool>,
    ) -> anyhow::Result<()> {
        let mut done: HashMap<i32, bool> = self.ends.keys().map(|p| (*p, self.finished.contains(p))).collect();
        let deadline = std::time::Instant::now() + Duration::from_secs(120);
        while done.values().any(|d| !d) {
            if std::time::Instant::now() > deadline {
                anyhow::bail!("timed out reading {}", self.dlq);
            }
            let Some(result) = self.consumer.poll(Duration::from_millis(500)) else { continue };
            let message = result?;
            let end = self.ends.get(&message.partition()).copied().unwrap_or(0);
            if message.offset() >= end {
                done.insert(message.partition(), true);
                continue;
            }
            if !visit(&message)? {
                return Ok(());
            }
            if message.offset() + 1 >= end {
                done.insert(message.partition(), true);
            }
        }
        Ok(())
    }

    fn commit(&self, partition: i32, next: i64) -> anyhow::Result<()> {
        let mut offsets = TopicPartitionList::new();
        offsets.add_partition_offset(&self.dlq, partition, Offset::Offset(next))?;
        self.consumer.commit(&offsets, CommitMode::Sync)?;
        Ok(())
    }
}

/// The group's parked events not yet replayed, oldest first per partition (at most `limit`
/// listed; the count is of all of them).
pub fn pending(settings: &KafkaSettings, topic: &str, group: &str, limit: usize) -> anyhow::Result<(Vec<Parked>, usize)> {
    valid_group(group)?;
    let (_, dlq) = topics_for(topic)?;
    let reader = Reader::open(settings, &dlq, group)?;
    let (mut listed, mut total) = (Vec::new(), 0);
    reader.for_each(|message| {
        if header(message, "x-consumer") == Some(group) {
            total += 1;
            if listed.len() < limit {
                listed.push(describe(message));
            }
        }
        Ok(true)
    })?;
    Ok((listed, total))
}

/// Send up to `limit` of the group's parked events back to the original topic, for that
/// group only.
pub async fn replay(settings: &KafkaSettings, topic: &str, group: &str, limit: usize) -> anyhow::Result<Replayed> {
    valid_group(group)?;
    let (original, dlq) = topics_for(topic)?;
    let reader = Reader::open(settings, &dlq, group)?;
    let producer: FutureProducer = settings.producer().create()?;
    let mut parked = Vec::new();
    // Collect first (the reader is synchronous), then send and commit one at a time.
    reader.for_each(|message| {
        let ours = header(message, "x-consumer") == Some(group);
        parked.push((
            message.partition(),
            message.offset(),
            ours.then(|| {
                (message.key().map(<[u8]>::to_vec), message.payload().map(<[u8]>::to_vec), headers_of(message), describe(message))
            }),
        ));
        Ok(true)
    })?;
    let mut out = Replayed::default();
    for (partition, offset, ours) in parked {
        let Some((key, payload, headers, described)) = ours else {
            // Another group's event: not ours to replay, but nothing to wait for either.
            if out.sent.len() < limit {
                reader.commit(partition, offset + 1)?;
            }
            continue;
        };
        if out.sent.len() >= limit {
            out.remaining += 1;
            continue;
        }
        let replayed_from = format!("{partition}:{offset}");
        let headers = headers
            .insert(Header { key: REPLAY_FOR, value: Some(group) })
            .insert(Header { key: "x-replayed-from", value: Some(replayed_from.as_str()) });
        let mut record = FutureRecord::to(&original).headers(headers);
        if let Some(key) = key.as_deref() {
            record = record.key(key);
        }
        if let Some(payload) = payload.as_deref() {
            record = record.payload(payload);
        }
        producer.send(record, Timeout::After(Duration::from_secs(30))).await.map_err(|(e, _)| e)?;
        reader.commit(partition, offset + 1)?;
        metrics::counter!("events_replayed_total", "consumer" => group.to_owned()).increment(1);
        out.sent.push(described);
    }
    Ok(out)
}

/// The event's own headers, without the dead-letter and replay ones (set afresh).
fn headers_of<M: Message>(message: &M) -> OwnedHeaders {
    let mut headers = OwnedHeaders::new();
    if let Some(original) = message.headers() {
        for h in original.iter().filter(|h| !h.key.starts_with("x-")) {
            headers = headers.insert(Header { key: h.key, value: h.value });
        }
    }
    headers
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_destination_comes_from_the_topic_name() {
        let expected = ("platform.issue.v1".to_owned(), "platform.issue.v1.dlq".to_owned());
        assert_eq!(topics_for("platform.issue.v1").unwrap(), expected);
        assert_eq!(topics_for("platform.issue.v1.dlq").unwrap(), expected);
        for bad in ["", ".dlq", "a.dlq.dlq", "a b", "a/b", "a\n"] {
            assert!(topics_for(bad).is_err(), "{bad:?}");
        }
        assert!(valid_group("search-service").is_ok());
        assert!(valid_group("").is_err() && valid_group("a,b").is_err());
    }
}
