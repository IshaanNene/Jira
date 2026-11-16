//! Who is told about what. Pure functions over an event and the issue's audience.

use event_bus::{Event, HandlerError};
use event_contracts::payloads::{CommentCreatedV1, IssueAssignedV1, IssueCreatedV1, IssueMentionedV1, IssueTransitionedV1};
use serde_json::{json, Value};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Assigned,
    Commented,
    Mentioned,
    StatusChanged,
    /// The scheduled email of a saved filter someone subscribed to. Email only.
    FilterDigest,
}

impl Kind {
    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "assigned" => Self::Assigned,
            "commented" => Self::Commented,
            "mentioned" => Self::Mentioned,
            "status_changed" => Self::StatusChanged,
            "filter_digest" => Self::FilterDigest,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Assigned => "assigned",
            Self::Commented => "commented",
            Self::Mentioned => "mentioned",
            Self::StatusChanged => "status_changed",
            Self::FilterDigest => "filter_digest",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum What {
    Assigned(Uuid),
    Commented {
        comment_id: Uuid,
        mentioned: Vec<Uuid>,
    },
    /// Mentioned in a description, or added to an existing comment.
    Mentioned {
        comment_id: Option<Uuid>,
        mentioned: Vec<Uuid>,
    },
    Transitioned {
        from: String,
        to: String,
    },
}

/// An event that can cause notifications.
#[derive(Debug, Clone, PartialEq)]
pub struct Trigger {
    pub issue_id: Uuid,
    pub actor: Option<Uuid>,
    pub what: What,
}

impl Trigger {
    /// The comment this is about, if any.
    pub fn comment_id(&self) -> Option<Uuid> {
        match &self.what {
            What::Commented { comment_id, .. } => Some(*comment_id),
            What::Mentioned { comment_id, .. } => *comment_id,
            _ => None,
        }
    }

    pub fn detail(&self) -> Value {
        match &self.what {
            What::Assigned(_) => json!({}),
            What::Commented { comment_id, .. } => json!({ "comment_id": comment_id }),
            What::Mentioned { comment_id: Some(comment_id), .. } => json!({ "comment_id": comment_id }),
            What::Mentioned { comment_id: None, .. } => json!({ "in": "description" }),
            What::Transitioned { from, to } => json!({ "from_status": from, "to_status": to }),
        }
    }
}

/// `None` for events that never notify anyone.
pub fn trigger(event: &Event) -> Result<Option<Trigger>, HandlerError> {
    let actor = event.actor_user();
    let make = |issue_id, what| Ok(Some(Trigger { issue_id, actor, what }));
    match event.event_type() {
        "issue.created" => {
            let p: IssueCreatedV1 = event.payload()?;
            match p.assignee_id {
                Some(user) => make(p.issue_id, What::Assigned(user)),
                None => Ok(None),
            }
        }
        "issue.assigned" => {
            let p: IssueAssignedV1 = event.payload()?;
            match p.assignee_id {
                Some(user) => make(p.issue_id, What::Assigned(user)),
                None => Ok(None),
            }
        }
        "comment.created" => {
            let p: CommentCreatedV1 = event.payload()?;
            make(p.issue_id, What::Commented { comment_id: p.comment_id, mentioned: p.mentioned_user_ids })
        }
        "issue.mentioned" => {
            let p: IssueMentionedV1 = event.payload()?;
            make(p.issue_id, What::Mentioned { comment_id: p.comment_id, mentioned: p.mentioned_user_ids })
        }
        "issue.transitioned" => {
            let p: IssueTransitionedV1 = event.payload()?;
            make(p.issue_id, What::Transitioned { from: p.from_status, to: p.to_status })
        }
        _ => Ok(None),
    }
}

/// The people around an issue, as issue-service reports them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Audience {
    pub key: String,
    pub summary: String,
    pub project_id: Uuid,
    pub reporter_id: Uuid,
    pub assignee_id: Option<Uuid>,
    pub watcher_ids: Vec<Uuid>,
    /// The comment the notification is about, as plain text (bounded by issue-service);
    /// empty for other notifications or when the comment is gone.
    pub comment_text: String,
}

/// Candidates, before access checks: each person once, with the most specific reason,
/// never the person who acted.
pub fn recipients(trigger: &Trigger, audience: &Audience) -> Vec<(Uuid, Kind)> {
    let mut out: Vec<(Uuid, Kind)> = Vec::new();
    let mut add = |user: Uuid, kind: Kind| {
        if Some(user) != trigger.actor && !out.iter().any(|(u, _)| *u == user) {
            out.push((user, kind));
        }
    };
    let followers =
        || std::iter::once(audience.reporter_id).chain(audience.assignee_id).chain(audience.watcher_ids.iter().copied());
    match &trigger.what {
        What::Assigned(user) => add(*user, Kind::Assigned),
        What::Commented { mentioned, .. } => {
            for user in mentioned {
                add(*user, Kind::Mentioned);
            }
            for user in followers() {
                add(user, Kind::Commented);
            }
        }
        What::Mentioned { mentioned, .. } => {
            for user in mentioned {
                add(*user, Kind::Mentioned);
            }
        }
        What::Transitioned { .. } => {
            for user in followers() {
                add(user, Kind::StatusChanged);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn audience() -> Audience {
        Audience {
            key: "PAY-1".into(),
            summary: "Checkout".into(),
            project_id: Uuid::now_v7(),
            reporter_id: Uuid::from_u128(1),
            assignee_id: Some(Uuid::from_u128(2)),
            watcher_ids: vec![Uuid::from_u128(3), Uuid::from_u128(1)],
            comment_text: String::new(),
        }
    }

    fn trig(actor: u128, what: What) -> Trigger {
        Trigger { issue_id: Uuid::now_v7(), actor: Some(Uuid::from_u128(actor)), what }
    }

    #[test]
    fn comments_reach_mentions_first_then_followers_once_and_never_the_author() {
        let a = audience();
        let t = trig(2, What::Commented { comment_id: Uuid::now_v7(), mentioned: vec![Uuid::from_u128(3), Uuid::from_u128(9)] });
        assert_eq!(
            recipients(&t, &a),
            vec![
                (Uuid::from_u128(3), Kind::Mentioned),
                (Uuid::from_u128(9), Kind::Mentioned),
                (Uuid::from_u128(1), Kind::Commented)
            ]
        );
    }

    #[test]
    fn assignment_tells_only_the_new_assignee_unless_they_did_it() {
        let a = audience();
        assert_eq!(recipients(&trig(1, What::Assigned(Uuid::from_u128(5))), &a), vec![(Uuid::from_u128(5), Kind::Assigned)]);
        assert!(recipients(&trig(5, What::Assigned(Uuid::from_u128(5))), &a).is_empty());
    }

    #[test]
    fn status_changes_reach_reporter_assignee_and_watchers() {
        let a = audience();
        let t = trig(3, What::Transitioned { from: "todo".into(), to: "done".into() });
        assert_eq!(
            recipients(&t, &a),
            vec![(Uuid::from_u128(1), Kind::StatusChanged), (Uuid::from_u128(2), Kind::StatusChanged)]
        );
        assert_eq!(t.detail()["to_status"], "done");
    }

    #[test]
    fn later_mentions_reach_only_the_people_mentioned_and_not_followers() {
        let a = audience();
        let t = trig(1, What::Mentioned { comment_id: None, mentioned: vec![Uuid::from_u128(1), Uuid::from_u128(9)] });
        assert_eq!(recipients(&t, &a), vec![(Uuid::from_u128(9), Kind::Mentioned)]);
        assert_eq!(t.detail(), json!({ "in": "description" }));
        let c = Uuid::now_v7();
        assert_eq!(trig(1, What::Mentioned { comment_id: Some(c), mentioned: vec![] }).detail(), json!({ "comment_id": c }));
    }
}
