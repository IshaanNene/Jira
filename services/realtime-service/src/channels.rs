//! What a client may subscribe to, and which channel each event is announced on.
//!
//! Clients receive *hints* (event type, aggregate id and version, issue key, actor), never
//! payloads: they refetch over REST, where every read is authorized again. A hint can
//! therefore never reveal more than the subscriber may already see.

use std::fmt;

use event_bus::Event;
use event_contracts::topics;
use serde::Serialize;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Channel {
    /// Everything about one project's issues, boards and sprints. Needs Browse.
    Project(Uuid),
    /// One person's notifications. Only that person.
    User(Uuid),
    /// Workspace-wide changes (projects created or renamed, workflows published). Any member.
    Org,
}

impl Channel {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.split_once(':') {
            Some(("project", id)) => Uuid::parse_str(id).ok().map(Self::Project),
            Some(("user", id)) => Uuid::parse_str(id).ok().map(Self::User),
            None if raw == "org" => Some(Self::Org),
            _ => None,
        }
    }
}

impl fmt::Display for Channel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Project(id) => write!(f, "project:{id}"),
            Self::User(id) => write!(f, "user:{id}"),
            Self::Org => f.write_str("org"),
        }
    }
}

/// What a subscriber is told about a change.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Hint {
    #[serde(rename = "type")]
    pub event_type: String,
    pub id: Uuid,
    pub version: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actor: Option<Uuid>,
    #[serde(with = "time::serde::rfc3339")]
    pub at: time::OffsetDateTime,
}

fn uuid_field(event: &Event, name: &str) -> Option<Uuid> {
    event.envelope.payload.get(name).and_then(|v| v.as_str()).and_then(|v| Uuid::parse_str(v).ok())
}

/// The channels an event is announced on (possibly none) and the hint to send.
pub fn route(event: &Event) -> (Vec<Channel>, Hint) {
    let hint = Hint {
        event_type: event.event_type().to_owned(),
        id: event.envelope.aggregate_id,
        version: event.envelope.aggregate_version,
        key: event.envelope.payload.get("key").and_then(|v| v.as_str()).map(str::to_owned),
        actor: event.actor_user(),
        at: event.envelope.occurred_at,
    };
    let project = uuid_field(event, "project_id").map(Channel::Project);
    let channels = match event.topic.as_str() {
        topics::ISSUE | topics::AGILE => project.into_iter().collect(),
        topics::PROJECT => project.into_iter().chain([Channel::Org]).collect(),
        topics::WORKFLOW => vec![Channel::Org],
        topics::NOTIFICATION => uuid_field(event, "recipient_id").map(Channel::User).into_iter().collect(),
        _ => vec![],
    };
    (channels, hint)
}

/// Redis stream ids (`<ms>-<seq>`) double as client cursors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct StreamId(pub u64, pub u64);

impl StreamId {
    pub fn parse(raw: &str) -> Option<Self> {
        let (ms, seq) = raw.split_once('-')?;
        Some(Self(ms.parse().ok()?, seq.parse().ok()?))
    }
}

impl fmt::Display for StreamId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-{}", self.0, self.1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(topic: &str, event_type: &str, payload: serde_json::Value) -> Event {
        let raw = serde_json::json!({
            "event_id": Uuid::now_v7(), "event_type": event_type, "schema_version": 1, "tenant_id": Uuid::now_v7(),
            "aggregate_type": "issue", "aggregate_id": Uuid::now_v7(), "aggregate_version": 4,
            "occurred_at": "2026-11-12T09:00:00Z", "correlation_id": null, "causation_id": null,
            "actor": {"type": "user", "id": "0195c1e2-0000-7000-8000-000000000004"}, "payload": payload
        });
        Event::parse(topic, &serde_json::to_vec(&raw).unwrap()).unwrap()
    }

    #[test]
    fn channels_round_trip_and_reject_anything_else() {
        let id = Uuid::now_v7();
        for channel in [Channel::Project(id), Channel::User(id), Channel::Org] {
            assert_eq!(Channel::parse(&channel.to_string()), Some(channel));
        }
        for bad in ["", "project:", "project:nope", "tenant:x", "org:1", "user"] {
            assert_eq!(Channel::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn events_are_announced_where_they_belong_with_hints_only() {
        let project = Uuid::now_v7();
        let (channels, hint) = route(&event(
            topics::ISSUE,
            "issue.updated",
            serde_json::json!({ "project_id": project, "key": "PAY-1", "summary": "secret text" }),
        ));
        assert_eq!(channels, vec![Channel::Project(project)]);
        assert_eq!(hint.key.as_deref(), Some("PAY-1"));
        let json = serde_json::to_string(&hint).unwrap();
        assert!(!json.contains("secret text"), "payload fields never reach clients");

        let (channels, _) = route(&event(topics::PROJECT, "project.updated", serde_json::json!({ "project_id": project })));
        assert_eq!(channels, vec![Channel::Project(project), Channel::Org]);

        let user = Uuid::now_v7();
        let (channels, _) =
            route(&event(topics::NOTIFICATION, "notification.created", serde_json::json!({ "recipient_id": user })));
        assert_eq!(channels, vec![Channel::User(user)]);

        let (channels, _) = route(&event(topics::ISSUE, "comment.deleted", serde_json::json!({ "key": "PAY-1" })));
        assert!(channels.is_empty(), "no project, no announcement");
    }

    #[test]
    fn stream_ids_order_numerically() {
        let a = StreamId::parse("1700000000000-2").unwrap();
        let b = StreamId::parse("1700000000000-10").unwrap();
        assert!(a < b);
        assert_eq!(b.to_string(), "1700000000000-10");
        assert!(StreamId::parse("abc").is_none());
        assert!(StreamId::parse("1-x").is_none());
    }
}
