use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use time::{Date, OffsetDateTime};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::store::{Comment, HistoryRow, Issue};

time::serde::format_description!(pub iso_date, Date, "[year]-[month]-[day]");

/// Distinguishes "field absent" (leave unchanged) from "field: null" (clear it).
pub fn nullable<'de, T: Deserialize<'de>, D: Deserializer<'de>>(d: D) -> Result<Option<Option<T>>, D::Error> {
    Option::<T>::deserialize(d).map(Some)
}

fn nullable_date<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Option<Date>>, D::Error> {
    iso_date::option::deserialize(d).map(Some)
}

#[derive(Debug, Serialize, ToSchema)]
pub struct IssueRef {
    pub key: String,
    pub summary: String,
    #[serde(rename = "type")]
    pub issue_type: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct IssueDto {
    pub id: Uuid,
    pub key: String,
    pub project_key: String,
    #[serde(rename = "type")]
    pub issue_type: String,
    pub status: String,
    /// Display name of the status in the issue's workflow.
    pub status_name: String,
    /// todo | in_progress | done
    pub status_category: String,
    pub priority: String,
    pub summary: String,
    /// Rich-text document. Omitted in list responses.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<Object>)]
    pub description: Option<Value>,
    pub assignee_id: Option<Uuid>,
    pub reporter_id: Uuid,
    pub parent: Option<IssueRef>,
    pub labels: Vec<String>,
    pub component_ids: Vec<Uuid>,
    pub fix_version_ids: Vec<Uuid>,
    /// `{ "<custom field id>": value }`
    #[schema(value_type = Object)]
    pub custom_fields: Value,
    pub story_points: Option<f64>,
    #[serde(with = "iso_date::option")]
    #[schema(value_type = Option<String>, format = Date)]
    pub start_date: Option<Date>,
    #[serde(with = "iso_date::option")]
    #[schema(value_type = Option<String>, format = Date)]
    pub due_date: Option<Date>,
    pub resolution: Option<String>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub resolved_at: Option<OffsetDateTime>,
    pub version: i64,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

impl IssueDto {
    /// `status` is the display `(name, category)` resolved from the issue's workflow.
    pub fn new(i: Issue, with_description: bool, status: (String, String)) -> Self {
        let parent = match (&i.parent_project_key, i.parent_number, &i.parent_summary, &i.parent_type) {
            (Some(pk), Some(n), Some(summary), Some(t)) => {
                Some(IssueRef { key: format!("{pk}-{n}"), summary: summary.clone(), issue_type: t.clone() })
            }
            _ => None,
        };
        Self {
            key: i.key(),
            id: i.id,
            project_key: i.project_key,
            status_name: status.0,
            status_category: status.1,
            issue_type: i.type_key,
            status: i.status,
            priority: i.priority,
            summary: i.summary,
            description: if with_description { i.description } else { None },
            assignee_id: i.assignee_id,
            reporter_id: i.reporter_id,
            parent,
            labels: i.labels,
            component_ids: i.component_ids,
            fix_version_ids: i.fix_version_ids,
            custom_fields: i.custom_fields,
            story_points: i.story_points,
            start_date: i.start_date,
            due_date: i.due_date,
            resolution: i.resolution,
            resolved_at: i.resolved_at,
            version: i.version,
            created_at: i.created_at,
            updated_at: i.updated_at,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct IssueDetail {
    #[serde(flatten)]
    pub issue: IssueDto,
    pub watching: bool,
    pub watcher_count: i64,
    /// What the caller may do with this issue: administrator | member | viewer
    pub my_role: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct IssuePage {
    pub items: Vec<IssueDto>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct IssueQuery {
    pub project: String,
    /// Comma-separated status keys.
    pub status: Option<String>,
    /// Comma-separated categories: todo, in_progress, done.
    pub status_category: Option<String>,
    #[serde(rename = "type")]
    pub issue_type: Option<String>,
    pub priority: Option<String>,
    /// A user id, or `unassigned`.
    pub assignee: Option<String>,
    pub label: Option<String>,
    /// Key of the parent issue (lists its children).
    pub parent: Option<String>,
    /// Matches summary or key.
    pub q: Option<String>,
    pub limit: Option<u32>,
    pub cursor: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateIssueRequest {
    pub project_key: String,
    #[serde(rename = "type")]
    pub issue_type: String,
    pub summary: String,
    #[schema(value_type = Option<Object>)]
    pub description: Option<Value>,
    pub priority: Option<String>,
    pub assignee_id: Option<Uuid>,
    pub parent_key: Option<String>,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default)]
    pub component_ids: Vec<Uuid>,
    #[serde(default)]
    pub fix_version_ids: Vec<Uuid>,
    /// `{ "<custom field id>": value }`
    #[serde(default)]
    #[schema(value_type = Object)]
    pub custom_fields: serde_json::Map<String, Value>,
    pub story_points: Option<f64>,
    #[serde(default, with = "iso_date::option")]
    #[schema(value_type = Option<String>, format = Date)]
    pub start_date: Option<Date>,
    #[serde(default, with = "iso_date::option")]
    #[schema(value_type = Option<String>, format = Date)]
    pub due_date: Option<Date>,
}

/// Every field is optional. For nullable fields, `null` clears the value and omitting the
/// field leaves it unchanged.
#[derive(Debug, Default, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateIssueRequest {
    pub summary: Option<String>,
    #[serde(default, deserialize_with = "nullable")]
    #[schema(value_type = Option<Object>)]
    pub description: Option<Option<Value>>,
    #[serde(rename = "type")]
    pub issue_type: Option<String>,
    pub status: Option<String>,
    pub priority: Option<String>,
    #[serde(default, deserialize_with = "nullable")]
    #[schema(value_type = Option<Uuid>)]
    pub assignee_id: Option<Option<Uuid>>,
    #[serde(default, deserialize_with = "nullable")]
    #[schema(value_type = Option<String>)]
    pub parent_key: Option<Option<String>>,
    pub labels: Option<Vec<String>>,
    pub component_ids: Option<Vec<Uuid>>,
    pub fix_version_ids: Option<Vec<Uuid>>,
    /// Only the fields listed are changed; `null` clears a value.
    #[schema(value_type = Option<Object>)]
    pub custom_fields: Option<serde_json::Map<String, Value>>,
    #[serde(default, deserialize_with = "nullable")]
    #[schema(value_type = Option<f64>)]
    pub story_points: Option<Option<f64>>,
    #[serde(default, deserialize_with = "nullable_date")]
    #[schema(value_type = Option<String>, format = Date)]
    pub start_date: Option<Option<Date>>,
    #[serde(default, deserialize_with = "nullable_date")]
    #[schema(value_type = Option<String>, format = Date)]
    pub due_date: Option<Option<Date>>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct CommentDto {
    pub id: Uuid,
    pub author_id: Uuid,
    #[schema(value_type = Object)]
    pub body: Value,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub edited_at: Option<OffsetDateTime>,
}

impl From<Comment> for CommentDto {
    fn from(c: Comment) -> Self {
        Self { id: c.id, author_id: c.author_id, body: c.body, created_at: c.created_at, edited_at: c.edited_at }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct CommentList {
    pub items: Vec<CommentDto>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CommentRequest {
    #[schema(value_type = Object)]
    pub body: Value,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct LinkDto {
    pub id: Uuid,
    pub link_type: String,
    /// How this issue relates to the other one, e.g. "is blocked by".
    pub label: String,
    pub issue: LinkedIssue,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct LinkedIssue {
    pub key: String,
    pub summary: String,
    #[serde(rename = "type")]
    pub issue_type: String,
    pub status: String,
    pub status_name: String,
    pub status_category: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct LinkList {
    pub items: Vec<LinkDto>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CloneIssueRequest {
    /// Defaults to "CLONE - " and the original's summary.
    #[serde(default)]
    pub summary: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateLinkRequest {
    /// blocks | relates | duplicates | depends_on | clones — read as "this issue <type> target".
    pub link_type: String,
    pub target_key: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct HistoryEntry {
    pub id: Uuid,
    pub actor_id: Uuid,
    /// `[{ "field", "from", "to" }]`
    #[schema(value_type = Vec<Object>)]
    pub changes: Value,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

impl From<HistoryRow> for HistoryEntry {
    fn from(h: HistoryRow) -> Self {
        Self { id: h.id, actor_id: h.actor_id, changes: h.changes, created_at: h.created_at }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct HistoryList {
    pub items: Vec<HistoryEntry>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct MetaType {
    pub key: String,
    pub name: String,
    /// 1 epic, 0 standard, -1 subtask
    pub level: i8,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct MetaLinkType {
    pub key: String,
    pub outward: String,
    pub inward: String,
}

/// The vocabulary issues are built from. Clients render from this instead of hardcoding it.
/// Statuses are not here: they depend on the workflow of each project and issue type.
#[derive(Debug, Serialize, ToSchema)]
pub struct IssueMeta {
    pub types: Vec<MetaType>,
    pub priorities: Vec<String>,
    pub link_types: Vec<MetaLinkType>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct CustomFieldDto {
    pub id: Uuid,
    pub name: String,
    /// text | number | date | select | multi_select | checkbox | url | user
    pub field_type: String,
    pub options: Vec<String>,
    pub required: bool,
    /// Projects the field applies to; empty = all projects.
    pub project_keys: Vec<String>,
    /// Issue types the field applies to; empty = all types.
    pub issue_types: Vec<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

impl From<crate::custom_fields::Field> for CustomFieldDto {
    fn from(f: crate::custom_fields::Field) -> Self {
        Self {
            id: f.id,
            name: f.name,
            field_type: f.field_type,
            options: f.options,
            required: f.required,
            project_keys: f.project_keys,
            issue_types: f.issue_types,
            created_at: f.created_at,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct CustomFieldList {
    pub items: Vec<CustomFieldDto>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateCustomFieldRequest {
    pub name: String,
    pub field_type: String,
    #[serde(default)]
    pub options: Vec<String>,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub project_keys: Vec<String>,
    #[serde(default)]
    pub issue_types: Vec<String>,
}

/// The field type cannot be changed after creation.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateCustomFieldRequest {
    pub name: String,
    #[serde(default)]
    pub options: Vec<String>,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub project_keys: Vec<String>,
    #[serde(default)]
    pub issue_types: Vec<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AttachmentDto {
    pub id: Uuid,
    pub filename: String,
    pub content_type: String,
    pub size_bytes: i64,
    pub uploader_id: Uuid,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

impl From<crate::store::Attachment> for AttachmentDto {
    fn from(a: crate::store::Attachment) -> Self {
        Self {
            id: a.id,
            filename: a.filename,
            content_type: a.content_type,
            size_bytes: a.size_bytes,
            uploader_id: a.uploader_id,
            created_at: a.created_at,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AttachmentList {
    pub items: Vec<AttachmentDto>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct StatusRef {
    pub key: String,
    pub name: String,
    /// todo | in_progress | done
    pub category: String,
}

/// A status change the caller may make from the issue's current status.
#[derive(Debug, Serialize, ToSchema)]
pub struct TransitionDto {
    pub id: String,
    /// Name of the step, e.g. "Send for review".
    pub name: String,
    pub to: StatusRef,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TransitionList {
    pub items: Vec<TransitionDto>,
}
