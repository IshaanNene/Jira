//! Public REST API for issues.

use auth_middleware::{AuthContext, OrgContext, ProjectPermission, ProjectRole};
use axum::{
    extract::{Path, Query, State},
    http::{
        header::{self, ETAG},
        HeaderMap, HeaderValue, Method, StatusCode,
    },
    routing::{delete, get, patch, put},
    Json, Router,
};
use error_handling::{require_if_match, ApiError, ApiResult};
use event_contracts::{
    envelope::EventPayload,
    outbox,
    payloads::{
        CommentCreatedV1, CommentDeletedV1, IssueAssignedV1, IssueCreatedV1, IssueDeletedV1, IssueMentionedV1,
        IssueTransitionedV1, IssueUpdatedV1,
    },
    Actor, EventEnvelope,
};
use platform_common::pagination::{Page, PageParams};
use serde::Serialize;
use serde_json::{json, Value};
use tenant_context::TenantTx;
use time::OffsetDateTime;
use tower_http::cors::{AllowOrigin, CorsLayer};
use utoipa::OpenApi;
use uuid::Uuid;

use crate::{
    automation,
    domain::{
        self, check_parent, issue_type, normalize_labels, parse_key, ISSUE_TYPES, LINK_TYPES, PRIORITIES, STATUS_CATEGORIES,
    },
    dto::*,
    richtext,
    store::{self, AssigneeFilter, Issue, IssueFilter, NewIssue},
    workflows::{humanize, Evaluation, IssueFacts, PostFunction, Scope, StatusInfo},
    AppState,
};

pub(crate) const NOT_FOUND: ApiError = ApiError::NotFound("issue");

pub(crate) fn etag(version: i64) -> HeaderMap {
    let mut headers = HeaderMap::new();
    if let Ok(v) = HeaderValue::from_str(&format!("\"{version}\"")) {
        headers.insert(ETAG, v);
    }
    headers
}

fn validate_summary(summary: &str) -> ApiResult<String> {
    let trimmed = summary.trim();
    if trimmed.is_empty() || trimmed.chars().count() > 255 {
        return Err(ApiError::invalid("summary", "invalid_summary", "The summary must be between 1 and 255 characters."));
    }
    Ok(trimmed.to_owned())
}

fn validate_points(points: Option<f64>) -> ApiResult<()> {
    match points {
        Some(p) if !(p.is_finite() && (0.0..=1000.0).contains(&p)) => {
            Err(ApiError::invalid("story_points", "invalid_points", "Story points must be between 0 and 1000."))
        }
        _ => Ok(()),
    }
}

fn validate_dates(start: Option<time::Date>, due: Option<time::Date>) -> ApiResult<()> {
    match (start, due) {
        (Some(s), Some(d)) if d < s => {
            Err(ApiError::invalid("due_date", "before_start", "The due date cannot be before the start date."))
        }
        _ => Ok(()),
    }
}

pub(crate) async fn emit<P: EventPayload + Serialize>(
    tx: &mut TenantTx,
    ctx: &OrgContext,
    issue_id: Uuid,
    version: i64,
    payload: P,
) -> ApiResult<()> {
    outbox::enqueue(tx.conn(), &EventEnvelope::new(Some(ctx.tenant_id), issue_id, version, Actor::User(ctx.user_id), payload))
        .await?;
    Ok(())
}

/// Open a tenant transaction, load the issue by key and check the caller's permission on
/// its project. Issues the caller cannot see are reported exactly like missing ones.
pub(crate) async fn open(
    state: &AppState,
    ctx: &OrgContext,
    key: &str,
    permission: ProjectPermission,
    lock: bool,
) -> ApiResult<(TenantTx, Issue, ProjectRole)> {
    let (project_key, number) = parse_key(key).ok_or(NOT_FOUND)?;
    let snapshot = state.policy.snapshot(ctx).await?;
    let mut tx = state.db.begin(ctx.tenant_id).await?;
    let issue = store::issue_by_key(tx.conn(), ctx.tenant_id, project_key, number, lock).await?.ok_or(NOT_FOUND)?;
    let role = snapshot.require(ctx, issue.project_id, permission).map_err(|e| match e {
        ApiError::NotFound(_) => NOT_FOUND,
        other => other,
    })?;
    Ok((tx, issue, role))
}

/// How to show an issue's status: `(name, category)` from its workflow. If the status is
/// no longer in the workflow, or the workflow service cannot be reached, the issue is
/// still shown — with a readable form of the stored key and the stored category.
pub(crate) async fn status_display(
    state: &AppState,
    ctx: &OrgContext,
    project_id: Uuid,
    issue_type: &str,
    status: &str,
    stored_category: &str,
) -> (String, String) {
    match state.workflows.info(Scope { tenant_id: ctx.tenant_id, project_id, issue_type }).await {
        Ok(info) => match info.status(status) {
            Some(s) => (s.name.clone(), s.category.clone()),
            None => (humanize(status), stored_category.to_owned()),
        },
        Err(err) => {
            tracing::warn!(error = %err, "workflow lookup failed; showing stored status");
            (humanize(status), stored_category.to_owned())
        }
    }
}

/// Move `next` into `target`, as the workflow allowed: the status, its category, the
/// resolution that goes with it, and the step's post-functions. `current` is the issue
/// before the move; `actor` is who is moving it.
pub(crate) fn enter_status(
    next: &mut Issue,
    current: &Issue,
    target: &StatusInfo,
    post_functions: Vec<PostFunction>,
    actor: Uuid,
) {
    let was_done = current.status_category == "done";
    next.status = target.key.clone();
    next.status_category = target.category.clone();
    // Defaults; a workflow's post-functions below can override them.
    if target.category == "done" && !was_done {
        next.resolution = Some("done".to_owned());
        next.resolved_at = Some(OffsetDateTime::now_utc());
    } else if target.category != "done" && was_done {
        next.resolution = None;
        next.resolved_at = None;
    }
    for effect in post_functions {
        match effect {
            PostFunction::AssignToCurrentUser => next.assignee_id = Some(actor),
            PostFunction::AssignToReporter => next.assignee_id = Some(current.reporter_id),
            PostFunction::ClearAssignee => next.assignee_id = None,
            PostFunction::SetResolution(value) => {
                next.resolution = Some(value);
                next.resolved_at = next.resolved_at.or_else(|| Some(OffsetDateTime::now_utc()));
            }
            PostFunction::ClearResolution => {
                next.resolution = None;
                next.resolved_at = None;
            }
        }
    }
}

pub(crate) async fn dto(state: &AppState, ctx: &OrgContext, issue: Issue, with_description: bool) -> IssueDto {
    let display = status_display(state, ctx, issue.project_id, &issue.type_key, &issue.status, &issue.status_category).await;
    IssueDto::new(issue, with_description, display)
}

/// What workflow rules may depend on, taken from the issue as it will be after the
/// current request's other edits.
pub(crate) fn facts(ctx: &OrgContext, role: ProjectRole, issue: &Issue, children_done: bool) -> IssueFacts {
    let mut present: Vec<String> = Vec::new();
    let mut mark = |name: &str, has_value: bool| {
        if has_value {
            present.push(name.to_owned());
        }
    };
    mark("assignee", issue.assignee_id.is_some());
    mark("description", issue.description.as_ref().is_some_and(has_text));
    mark("due_date", issue.due_date.is_some());
    mark("start_date", issue.start_date.is_some());
    mark("story_points", issue.story_points.is_some());
    mark("labels", !issue.labels.is_empty());
    mark("components", !issue.component_ids.is_empty());
    mark("fix_versions", !issue.fix_version_ids.is_empty());
    mark("parent", issue.parent_id.is_some());
    if let Some(custom) = issue.custom_fields.as_object() {
        present.extend(custom.keys().map(|id| format!("custom:{id}")));
    }
    IssueFacts {
        actor_role: role.as_str().to_owned(),
        actor_is_assignee: issue.assignee_id == Some(ctx.user_id),
        actor_is_reporter: issue.reporter_id == ctx.user_id,
        present_fields: present,
        children_done,
    }
}

/// Archived projects are read-only.
pub(crate) async fn ensure_writable(state: &AppState, ctx: &OrgContext, project_key: &str) -> ApiResult<()> {
    if state.directory.project(ctx.tenant_id, project_key, false).await?.archived {
        return Err(ApiError::Conflict("This project is archived. Restore it to make changes.".into()));
    }
    Ok(())
}

pub(crate) async fn ensure_assignable(state: &AppState, ctx: &OrgContext, user: Uuid) -> ApiResult<()> {
    if user != ctx.user_id && !state.directory.is_org_member(user, ctx.organization_id).await? {
        return Err(ApiError::invalid("assignee_id", "not_a_member", "The assignee must be a member of this workspace."));
    }
    Ok(())
}

/// Check that every id is a component (or version) of the project. A miss against the
/// cached project is retried once against fresh data, so something created seconds ago
/// is accepted. Returns the ids de-duplicated.
async fn check_refs(
    state: &AppState,
    ctx: &OrgContext,
    project_key: &str,
    field: &'static str,
    ids: &[Uuid],
    pick: fn(&crate::directory::ProjectInfo) -> &std::collections::HashSet<Uuid>,
) -> ApiResult<Vec<Uuid>> {
    let mut unique: Vec<Uuid> = Vec::new();
    for id in ids {
        if !unique.contains(id) {
            unique.push(*id);
        }
    }
    if unique.len() > 30 {
        return Err(ApiError::invalid(field, "too_many", "Choose at most 30."));
    }
    if unique.is_empty() {
        return Ok(unique);
    }
    let mut project = state.directory.project(ctx.tenant_id, project_key, false).await?;
    if !unique.iter().all(|id| pick(&project).contains(id)) {
        project = state.directory.project(ctx.tenant_id, project_key, true).await?;
    }
    if unique.iter().all(|id| pick(&project).contains(id)) {
        Ok(unique)
    } else {
        Err(ApiError::invalid(field, "unknown_reference", "One of the selected values does not belong to this project."))
    }
}

/// Validate custom field values for an issue and confirm that people referenced by
/// `user` fields belong to the organization.
#[allow(clippy::too_many_arguments)]
async fn apply_custom_fields(
    state: &AppState,
    ctx: &OrgContext,
    tx: &mut TenantTx,
    project_key: &str,
    issue_type: &str,
    current: &Value,
    incoming: &serde_json::Map<String, Value>,
    creating: bool,
) -> ApiResult<crate::custom_fields::Applied> {
    let fields = store::fields(tx.conn(), ctx.tenant_id).await?;
    let empty = serde_json::Map::new();
    let applied =
        crate::custom_fields::apply(&fields, project_key, issue_type, current.as_object().unwrap_or(&empty), incoming, creating)?;
    for user in &applied.users {
        if !state.directory.is_org_member(*user, ctx.organization_id).await? {
            return Err(ApiError::invalid(
                "custom_fields",
                "not_a_member",
                "A selected person is not a member of this workspace.",
            ));
        }
    }
    Ok(applied)
}

/// Resolve a parent key to an issue in the same project.
async fn load_parent(tx: &mut TenantTx, ctx: &OrgContext, project_id: Uuid, parent_key: &str) -> ApiResult<Issue> {
    let bad = || ApiError::invalid("parent_key", "invalid_parent", "The parent issue was not found in this project.");
    let (project_key, number) = parse_key(parent_key).ok_or_else(bad)?;
    let parent = store::issue_by_key(tx.conn(), ctx.tenant_id, project_key, number, false).await?.ok_or_else(bad)?;
    if parent.project_id != project_id {
        return Err(bad());
    }
    Ok(parent)
}

// --- meta --------------------------------------------------------------------

#[utoipa::path(get, path = "/api/v1/issues/meta", security(("bearer" = [])), responses((status = 200, body = IssueMeta)))]
pub async fn meta(_auth: AuthContext) -> Json<IssueMeta> {
    Json(IssueMeta {
        types: ISSUE_TYPES.iter().map(|t| MetaType { key: t.key.into(), name: t.name.into(), level: t.level }).collect(),
        priorities: PRIORITIES.iter().map(|p| (*p).to_owned()).collect(),
        link_types: LINK_TYPES
            .iter()
            .map(|(k, o, i)| MetaLinkType { key: (*k).into(), outward: (*o).into(), inward: (*i).into() })
            .collect(),
    })
}

// --- create / read -----------------------------------------------------------

#[utoipa::path(post, path = "/api/v1/issues", request_body = CreateIssueRequest, security(("bearer" = [])),
    responses((status = 201, body = IssueDto), (status = 404, description = "No such project"), (status = 422)))]
pub async fn create(
    State(state): State<AppState>,
    ctx: OrgContext,
    Json(body): Json<CreateIssueRequest>,
) -> ApiResult<(StatusCode, HeaderMap, Json<IssueDto>)> {
    let summary = validate_summary(&body.summary)?;
    let kind = issue_type(&body.issue_type)?;
    let priority = body.priority.as_deref().unwrap_or("medium");
    domain::validate_priority(priority)?;
    let labels = normalize_labels(body.labels)?;
    validate_points(body.story_points)?;
    validate_dates(body.start_date, body.due_date)?;
    let description_mentions = match &body.description {
        Some(doc) => richtext::validate("description", doc)?,
        None => Vec::new(),
    };

    let project = state.directory.project(ctx.tenant_id, &body.project_key.trim().to_uppercase(), false).await?;
    state.policy.snapshot(&ctx).await?.require(&ctx, project.id, ProjectPermission::CreateIssue)?;
    if project.archived {
        return Err(ApiError::Conflict("This project is archived. Restore it to make changes.".into()));
    }
    if let Some(assignee) = body.assignee_id {
        ensure_assignable(&state, &ctx, assignee).await?;
    }
    let component_ids =
        check_refs(&state, &ctx, &project.key, "component_ids", &body.component_ids, |p| &p.component_ids).await?;
    let fix_version_ids =
        check_refs(&state, &ctx, &project.key, "fix_version_ids", &body.fix_version_ids, |p| &p.version_ids).await?;

    let workflow = state.workflows.info(Scope { tenant_id: ctx.tenant_id, project_id: project.id, issue_type: kind.key }).await?;
    let initial = workflow
        .status(&workflow.initial)
        .cloned()
        .ok_or_else(|| ApiError::internal(anyhow::anyhow!("workflow has no initial status")))?;

    let mut tx = state.db.begin(ctx.tenant_id).await?;
    let custom =
        apply_custom_fields(&state, &ctx, &mut tx, &project.key, kind.key, &Value::Null, &body.custom_fields, true).await?;
    let custom_values = Value::Object(custom.values);
    let parent = match &body.parent_key {
        Some(key) => Some(load_parent(&mut tx, &ctx, project.id, key).await?),
        None => None,
    };
    check_parent(kind, parent.as_ref().map(|p| issue_type(&p.type_key)).transpose()?)?;

    let id = platform_common::new_id();
    // Allocated last, right before the insert, so validation failures never hold the counter lock.
    let number = store::allocate_number(tx.conn(), ctx.tenant_id, project.id).await?;
    store::insert_issue(
        tx.conn(),
        ctx.tenant_id,
        &NewIssue {
            id,
            project_id: project.id,
            project_key: &project.key,
            number,
            type_key: kind.key,
            status: &initial.key,
            status_category: &initial.category,
            priority,
            summary: &summary,
            description: body.description.as_ref(),
            assignee_id: body.assignee_id,
            reporter_id: ctx.user_id,
            parent_id: parent.as_ref().map(|p| p.id),
            labels: &labels,
            component_ids: &component_ids,
            fix_version_ids: &fix_version_ids,
            custom_fields: &custom_values,
            story_points: body.story_points,
            start_date: body.start_date,
            due_date: body.due_date,
        },
    )
    .await?;
    store::watch(tx.conn(), ctx.tenant_id, id, ctx.user_id).await?;
    store::add_history(tx.conn(), ctx.tenant_id, id, ctx.user_id, 1, &json!([{ "field": "created", "from": null, "to": null }]))
        .await?;
    let issue = store::issue_by_id(tx.conn(), ctx.tenant_id, id).await?.ok_or(NOT_FOUND)?;
    emit(
        &mut tx,
        &ctx,
        id,
        1,
        IssueCreatedV1 {
            issue_id: id,
            key: issue.key(),
            project_id: project.id,
            issue_type: kind.key.to_owned(),
            summary: summary.clone(),
            reporter_id: ctx.user_id,
            assignee_id: body.assignee_id,
        },
    )
    .await?;
    if !description_mentions.is_empty() {
        let mentioned = IssueMentionedV1 {
            issue_id: id,
            key: issue.key(),
            project_id: project.id,
            comment_id: None,
            mentioned_user_ids: description_mentions,
        };
        emit(&mut tx, &ctx, id, 1, mentioned).await?;
    }
    let issue = automation::run(&state, &ctx, &mut tx, issue, automation::Event::Created).await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, etag(issue.version), Json(dto(&state, &ctx, issue, true).await)))
}

/// A copy of an issue in the same project, linked to it ("clones"). Copies the type, priority,
/// description, assignee, labels, components, fix versions, custom fields, parent, story points and
/// dates; starts in the workflow's initial status with the caller as reporter and watcher. Comments,
/// attachments, links, watchers and sprint placement stay with the original. References that no
/// longer exist (a removed component, an assignee who left) are dropped rather than refused.
#[utoipa::path(post, path = "/api/v1/issues/{key}/clone", request_body = CloneIssueRequest, security(("bearer" = [])),
    params(("key" = String, Path)), responses((status = 201, body = IssueDto), (status = 404), (status = 409, description = "Archived project"), (status = 422)))]
pub async fn clone_issue(
    State(state): State<AppState>,
    ctx: OrgContext,
    Path(key): Path<String>,
    Json(body): Json<CloneIssueRequest>,
) -> ApiResult<(StatusCode, HeaderMap, Json<IssueDto>)> {
    let (mut tx, original, _) = open(&state, &ctx, &key, ProjectPermission::Browse, false).await?;
    state.policy.snapshot(&ctx).await?.require(&ctx, original.project_id, ProjectPermission::CreateIssue)?;
    ensure_writable(&state, &ctx, &original.project_key).await?;
    let summary = match &body.summary {
        Some(s) => validate_summary(s)?,
        None => format!("CLONE - {}", original.summary).chars().take(255).collect(),
    };
    let kind = issue_type(&original.type_key)?;
    let assignee_id = match original.assignee_id {
        Some(user) if ensure_assignable(&state, &ctx, user).await.is_ok() => Some(user),
        _ => None,
    };
    let project = state.directory.project(ctx.tenant_id, &original.project_key, true).await?;
    let component_ids: Vec<Uuid> =
        original.component_ids.iter().copied().filter(|id| project.component_ids.contains(id)).collect();
    let fix_version_ids: Vec<Uuid> =
        original.fix_version_ids.iter().copied().filter(|id| project.version_ids.contains(id)).collect();
    // Values of fields that still exist for this kind of issue, validated as on create.
    let fields = store::fields(tx.conn(), ctx.tenant_id).await?;
    let carried: serde_json::Map<String, Value> = original
        .custom_fields
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(id, _)| fields.iter().any(|f| f.id.to_string() == **id && f.applies_to(&original.project_key, kind.key)))
        .map(|(id, v)| (id.clone(), v.clone()))
        .collect();
    let custom =
        apply_custom_fields(&state, &ctx, &mut tx, &original.project_key, kind.key, &Value::Null, &carried, true).await?;
    let custom_values = Value::Object(custom.values);

    let workflow =
        state.workflows.info(Scope { tenant_id: ctx.tenant_id, project_id: original.project_id, issue_type: kind.key }).await?;
    let initial = workflow
        .status(&workflow.initial)
        .cloned()
        .ok_or_else(|| ApiError::internal(anyhow::anyhow!("workflow has no initial status")))?;

    let id = platform_common::new_id();
    let number = store::allocate_number(tx.conn(), ctx.tenant_id, original.project_id).await?;
    store::insert_issue(
        tx.conn(),
        ctx.tenant_id,
        &NewIssue {
            id,
            project_id: original.project_id,
            project_key: &original.project_key,
            number,
            type_key: kind.key,
            status: &initial.key,
            status_category: &initial.category,
            priority: &original.priority,
            summary: &summary,
            description: original.description.as_ref(),
            assignee_id,
            reporter_id: ctx.user_id,
            parent_id: original.parent_id,
            labels: &original.labels,
            component_ids: &component_ids,
            fix_version_ids: &fix_version_ids,
            custom_fields: &custom_values,
            story_points: original.story_points,
            start_date: original.start_date,
            due_date: original.due_date,
        },
    )
    .await?;
    store::watch(tx.conn(), ctx.tenant_id, id, ctx.user_id).await?;
    store::insert_link(tx.conn(), ctx.tenant_id, id, original.id, "clones", ctx.user_id).await?;
    store::add_history(
        tx.conn(),
        ctx.tenant_id,
        id,
        ctx.user_id,
        1,
        &json!([{ "field": "created", "from": null, "to": null }, { "field": "link", "from": null, "to": format!("clones {}", original.key()) }]),
    )
    .await?;
    let issue = store::issue_by_id(tx.conn(), ctx.tenant_id, id).await?.ok_or(NOT_FOUND)?;
    // People mentioned in the copied description already heard about it, so no mention event.
    emit(
        &mut tx,
        &ctx,
        id,
        1,
        IssueCreatedV1 {
            issue_id: id,
            key: issue.key(),
            project_id: original.project_id,
            issue_type: kind.key.to_owned(),
            summary,
            reporter_id: ctx.user_id,
            assignee_id,
        },
    )
    .await?;
    let issue = automation::run(&state, &ctx, &mut tx, issue, automation::Event::Created).await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, etag(issue.version), Json(dto(&state, &ctx, issue, true).await)))
}

#[utoipa::path(get, path = "/api/v1/issues", security(("bearer" = [])),
    params(("project" = String, Query), ("status" = Option<String>, Query), ("status_category" = Option<String>, Query), ("type" = Option<String>, Query),
           ("priority" = Option<String>, Query), ("assignee" = Option<String>, Query), ("label" = Option<String>, Query),
           ("parent" = Option<String>, Query), ("q" = Option<String>, Query), ("limit" = Option<u32>, Query), ("cursor" = Option<String>, Query)),
    responses((status = 200, body = IssuePage), (status = 404, description = "No such project")))]
pub async fn list(State(state): State<AppState>, ctx: OrgContext, Query(q): Query<IssueQuery>) -> ApiResult<Json<IssuePage>> {
    let split = |v: &Option<String>| {
        v.as_deref()
            .map(|s| s.split(',').map(|p| p.trim().to_owned()).filter(|p| !p.is_empty()).collect::<Vec<_>>())
            .unwrap_or_default()
    };
    // Status keys are workflow-defined, so any key is a legitimate filter value.
    let statuses = split(&q.status);
    let status_categories = split(&q.status_category);
    if status_categories.iter().any(|c| !STATUS_CATEGORIES.contains(&c.as_str())) {
        return Err(ApiError::invalid("status_category", "unknown_category", "Category must be todo, in_progress or done."));
    }
    let types = split(&q.issue_type);
    types.iter().try_for_each(|t| issue_type(t).map(|_| ()))?;
    let priorities = split(&q.priority);
    priorities.iter().try_for_each(|p| domain::validate_priority(p))?;
    let assignee =
        match q.assignee.as_deref() {
            None | Some("") => None,
            Some("unassigned") => Some(AssigneeFilter::Unassigned),
            Some("me") => Some(AssigneeFilter::User(ctx.user_id)),
            Some(id) => Some(AssigneeFilter::User(Uuid::parse_str(id).map_err(|_| {
                ApiError::invalid("assignee", "invalid_assignee", "Assignee must be a user id, me or unassigned.")
            })?)),
        };
    let params = PageParams { limit: q.limit, cursor: q.cursor };
    let limit = params.limit();
    let before_number =
        params.decode_cursor::<i64>().map_err(|_| ApiError::invalid("cursor", "invalid_cursor", "The cursor is not valid."))?;

    let project = state.directory.project(ctx.tenant_id, &q.project.trim().to_uppercase(), false).await?;
    state.policy.snapshot(&ctx).await?.require(&ctx, project.id, ProjectPermission::Browse)?;

    let mut tx = state.db.begin(ctx.tenant_id).await?;
    let parent_id = match &q.parent {
        Some(key) => Some(load_parent(&mut tx, &ctx, project.id, key).await.map_err(|_| NOT_FOUND)?.id),
        None => None,
    };
    let filter = IssueFilter {
        statuses,
        status_categories,
        types,
        priorities,
        assignee,
        label: q.label.map(|l| l.trim().to_lowercase()).filter(|l| !l.is_empty()),
        parent_id,
        text: q.q.map(|t| t.trim().chars().take(100).collect::<String>()).filter(|t| !t.is_empty()),
        before_number,
    };
    let rows = store::list_issues(tx.conn(), ctx.tenant_id, project.id, &filter, limit).await?;
    tx.commit().await?;
    let page = Page::from_overfetch(rows, limit, |i| i.number);
    let mut items = Vec::with_capacity(page.items.len());
    for issue in page.items {
        items.push(dto(&state, &ctx, issue, false).await);
    }
    Ok(Json(IssuePage { items, next_cursor: page.next_cursor }))
}

#[utoipa::path(get, path = "/api/v1/issues/{key}", security(("bearer" = [])), params(("key" = String, Path)),
    responses((status = 200, body = IssueDetail), (status = 404)))]
pub async fn get_one(
    State(state): State<AppState>,
    ctx: OrgContext,
    Path(key): Path<String>,
) -> ApiResult<(HeaderMap, Json<IssueDetail>)> {
    let (mut tx, issue, role) = open(&state, &ctx, &key, ProjectPermission::Browse, false).await?;
    let (watching, watcher_count) = store::watch_state(tx.conn(), ctx.tenant_id, issue.id, ctx.user_id).await?;
    let (voted, vote_count) = store::vote_state(tx.conn(), ctx.tenant_id, issue.id, ctx.user_id).await?;
    tx.commit().await?;
    Ok((
        etag(issue.version),
        Json(IssueDetail {
            issue: dto(&state, &ctx, issue, true).await,
            watching,
            watcher_count,
            voted,
            vote_count,
            my_role: role.as_str().to_owned(),
        }),
    ))
}

// --- update ------------------------------------------------------------------

#[derive(Default)]
pub(crate) struct Changes(pub(crate) Vec<Value>);

impl Changes {
    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    pub(crate) fn extend(&mut self, other: Changes) {
        self.0.extend(other.0);
    }
    /// A change made by an automation rule: recorded with the rule's name.
    pub(crate) fn record_by_rule<T: Serialize + PartialEq>(&mut self, field: &str, from: &T, to: &T, rule: &str) -> bool {
        if from == to {
            return false;
        }
        self.0.push(json!({ "field": field, "from": from, "to": to, "rule": rule }));
        true
    }
    fn record<T: Serialize + PartialEq>(&mut self, field: &str, from: &T, to: &T) -> bool {
        if from == to {
            return false;
        }
        self.0.push(json!({ "field": field, "from": from, "to": to }));
        true
    }
    pub(crate) fn fields(&self) -> Vec<String> {
        self.0.iter().filter_map(|c| c["field"].as_str().map(str::to_owned)).collect()
    }
}

#[utoipa::path(patch, path = "/api/v1/issues/{key}", request_body = UpdateIssueRequest, security(("bearer" = [])),
    params(("key" = String, Path), ("If-Match" = String, Header, description = "Version the edit is based on")),
    responses((status = 200, body = IssueDto), (status = 409, description = "Edited by someone else; body carries the current issue"), (status = 428)))]
pub async fn update(
    State(state): State<AppState>,
    ctx: OrgContext,
    Path(key): Path<String>,
    headers: HeaderMap,
    Json(body): Json<UpdateIssueRequest>,
) -> ApiResult<(HeaderMap, Json<IssueDto>)> {
    let expected = require_if_match(&headers)?;
    let permission = if body.status.is_some() { ProjectPermission::TransitionIssue } else { ProjectPermission::EditIssue };
    let (mut tx, current, role) = open(&state, &ctx, &key, permission, true).await?;
    if body.status.is_some() {
        // A request that changes status and other fields needs both permissions.
        state.policy.snapshot(&ctx).await?.require(&ctx, current.project_id, ProjectPermission::EditIssue)?;
    }
    if current.version != expected {
        return Err(ApiError::VersionConflict { current: serde_json::to_value(dto(&state, &ctx, current, true).await).ok() });
    }
    ensure_writable(&state, &ctx, &current.project_key).await?;

    let mut next = current.clone();
    let mut changes = Changes(Vec::new());

    if let Some(summary) = &body.summary {
        next.summary = validate_summary(summary)?;
        changes.record("summary", &current.summary, &next.summary);
    }
    let mut newly_mentioned = Vec::new();
    if let Some(description) = body.description {
        if let Some(doc) = &description {
            newly_mentioned = richtext::newly_mentioned(richtext::validate("description", doc)?, current.description.as_ref());
        }
        if current.description != description {
            // Bodies can be large; history records that it changed, not both versions.
            changes.0.push(json!({ "field": "description", "from": null, "to": null }));
        }
        next.description = description;
    }
    if let Some(priority) = &body.priority {
        domain::validate_priority(priority)?;
        next.priority = priority.clone();
        changes.record("priority", &current.priority, &next.priority);
    }
    if let Some(labels) = body.labels {
        next.labels = normalize_labels(labels)?;
        changes.record("labels", &current.labels, &next.labels);
    }
    if let Some(points) = body.story_points {
        validate_points(points)?;
        next.story_points = points;
        changes.record("story_points", &current.story_points, &next.story_points);
    }
    if let Some(date) = body.start_date {
        next.start_date = date;
        changes.record("start_date", &current.start_date.map(|d| d.to_string()), &date.map(|d| d.to_string()));
    }
    if let Some(date) = body.due_date {
        next.due_date = date;
        changes.record("due_date", &current.due_date.map(|d| d.to_string()), &date.map(|d| d.to_string()));
    }
    validate_dates(next.start_date, next.due_date)?;
    if let Some(ids) = &body.component_ids {
        next.component_ids = check_refs(&state, &ctx, &current.project_key, "component_ids", ids, |p| &p.component_ids).await?;
        changes.record("components", &current.component_ids, &next.component_ids);
    }
    if let Some(ids) = &body.fix_version_ids {
        next.fix_version_ids = check_refs(&state, &ctx, &current.project_key, "fix_version_ids", ids, |p| &p.version_ids).await?;
        changes.record("fix_versions", &current.fix_version_ids, &next.fix_version_ids);
    }

    // Recorded after the status change below: a transition may reassign the issue too.
    if let Some(assignee) = body.assignee_id {
        if let Some(user) = assignee.filter(|u| Some(*u) != current.assignee_id) {
            ensure_assignable(&state, &ctx, user).await?;
        }
        next.assignee_id = assignee;
    }

    // Type and parent are validated together: either may change the hierarchy.
    if let Some(type_key) = &body.issue_type {
        next.type_key = issue_type(type_key)?.key.to_owned();
    }
    let mut parent_type = current.parent_type.as_deref().map(issue_type).transpose()?;
    let mut parent_key_change: Option<(Option<String>, Option<String>)> = None;
    if let Some(parent_key) = &body.parent_key {
        let old_key = match (&current.parent_project_key, current.parent_number) {
            (Some(pk), Some(n)) => Some(format!("{pk}-{n}")),
            _ => None,
        };
        match parent_key {
            Some(key) => {
                let parent = load_parent(&mut tx, &ctx, current.project_id, key).await?;
                if parent.id == current.id {
                    return Err(ApiError::invalid("parent_key", "invalid_parent", "An issue cannot be its own parent."));
                }
                parent_type = Some(issue_type(&parent.type_key)?);
                next.parent_id = Some(parent.id);
                parent_key_change = Some((old_key, Some(parent.key())));
            }
            None => {
                parent_type = None;
                next.parent_id = None;
                parent_key_change = Some((old_key, None));
            }
        }
    }
    let (old_kind, new_kind) = (issue_type(&current.type_key)?, issue_type(&next.type_key)?);
    if old_kind != new_kind || parent_key_change.is_some() {
        check_parent(new_kind, parent_type)?;
        if old_kind.level != new_kind.level && store::has_children(tx.conn(), ctx.tenant_id, current.id).await? {
            return Err(ApiError::invalid(
                "type",
                "has_children",
                "Move or remove this issue's child issues before changing its type.",
            ));
        }
    }
    changes.record("type", &current.type_key, &next.type_key);
    if let Some(incoming) = &body.custom_fields {
        // Validated against the type the issue will have after this request.
        let applied = apply_custom_fields(
            &state,
            &ctx,
            &mut tx,
            &current.project_key,
            &next.type_key,
            &current.custom_fields,
            incoming,
            false,
        )
        .await?;
        for (name, from, to) in applied.changes {
            changes.0.push(json!({ "field": name, "from": from, "to": to, "custom": true }));
        }
        next.custom_fields = Value::Object(applied.values);
    }
    if let Some((from, to)) = &parent_key_change {
        changes.record("parent", from, to);
    }

    let scope = Scope { tenant_id: ctx.tenant_id, project_id: current.project_id, issue_type: &next.type_key };
    let status_changed = match body.status.as_deref().filter(|to| *to != current.status) {
        Some(to) => {
            if to.len() > 40 {
                return Err(ApiError::invalid("status", "unknown_status", "That status is not part of this issue's workflow."));
            }
            // The workflow engine decides. It sees the issue as this request will leave it,
            // so a required field can be filled in and the issue moved in one step.
            let children_done = store::children_done(tx.conn(), ctx.tenant_id, current.id).await?;
            let (target, post_functions) =
                match state.workflows.evaluate(scope, &current.status, to, &facts(&ctx, role, &next, children_done)).await? {
                    Evaluation::Allowed { to, post_functions } => (to, post_functions),
                    Evaluation::Denied { message } => return Err(ApiError::invalid("status", "transition_denied", &message)),
                };
            enter_status(&mut next, &current, &target, post_functions, ctx.user_id);
            let (from_name, _) =
                status_display(&state, &ctx, current.project_id, &current.type_key, &current.status, &current.status_category)
                    .await;
            changes.record("status", &from_name, &target.name)
        }
        None => {
            if next.type_key != current.type_key {
                // A different issue type may be governed by a different workflow.
                let info = state.workflows.info(scope).await?;
                if info.status(&current.status).is_none() {
                    return Err(ApiError::invalid(
                        "type",
                        "status_not_in_workflow",
                        "This issue's status does not exist in the workflow for that issue type. Move the issue to another status first.",
                    ));
                }
            }
            false
        }
    };
    let assignee_changed = changes.record("assignee", &current.assignee_id, &next.assignee_id);
    if next.resolution != current.resolution {
        changes.record("resolution", &current.resolution, &next.resolution);
    }

    if changes.0.is_empty() {
        return Ok((etag(current.version), Json(dto(&state, &ctx, current, true).await)));
    }

    store::save_issue(tx.conn(), ctx.tenant_id, &next).await?;
    let version = current.version + 1;
    store::add_history(tx.conn(), ctx.tenant_id, current.id, ctx.user_id, version, &Value::Array(changes.0.clone())).await?;
    let (issue_id, issue_key, project_id) = (current.id, current.key(), current.project_id);
    emit(
        &mut tx,
        &ctx,
        issue_id,
        version,
        IssueUpdatedV1 { issue_id, key: issue_key.clone(), project_id, changed_fields: changes.fields() },
    )
    .await?;
    if !newly_mentioned.is_empty() {
        let mentioned = IssueMentionedV1 {
            issue_id,
            key: issue_key.clone(),
            project_id,
            comment_id: None,
            mentioned_user_ids: newly_mentioned,
        };
        emit(&mut tx, &ctx, issue_id, version, mentioned).await?;
    }
    if status_changed {
        emit(
            &mut tx,
            &ctx,
            issue_id,
            version,
            IssueTransitionedV1 {
                issue_id,
                key: issue_key.clone(),
                project_id,
                from_status: current.status.clone(),
                to_status: next.status.clone(),
                from_category: Some(current.status_category.clone()),
                to_category: Some(next.status_category.clone()),
            },
        )
        .await?;
    }
    if assignee_changed {
        emit(
            &mut tx,
            &ctx,
            issue_id,
            version,
            IssueAssignedV1 {
                issue_id,
                key: issue_key,
                project_id,
                assignee_id: next.assignee_id,
                previous_assignee_id: current.assignee_id,
            },
        )
        .await?;
    }
    let mut saved = store::issue_by_id(tx.conn(), ctx.tenant_id, issue_id).await?.ok_or(NOT_FOUND)?;
    if status_changed {
        let to_status = saved.status.clone();
        saved = automation::run(&state, &ctx, &mut tx, saved, automation::Event::Transitioned { to_status: &to_status }).await?;
    }
    tx.commit().await?;
    Ok((etag(saved.version), Json(dto(&state, &ctx, saved, true).await)))
}

#[utoipa::path(delete, path = "/api/v1/issues/{key}", security(("bearer" = [])), params(("key" = String, Path)),
    responses((status = 204), (status = 403), (status = 409, description = "Has child issues")))]
pub async fn remove(State(state): State<AppState>, ctx: OrgContext, Path(key): Path<String>) -> ApiResult<StatusCode> {
    let (mut tx, issue, _) = open(&state, &ctx, &key, ProjectPermission::DeleteIssue, true).await?;
    if store::has_children(tx.conn(), ctx.tenant_id, issue.id).await? {
        return Err(ApiError::Conflict("This issue has child issues. Delete or move them first.".into()));
    }
    store::soft_delete(tx.conn(), ctx.tenant_id, issue.id).await?;
    emit(
        &mut tx,
        &ctx,
        issue.id,
        issue.version + 1,
        IssueDeletedV1 { issue_id: issue.id, key: issue.key(), project_id: issue.project_id },
    )
    .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

/// The status changes the caller may make right now. Steps whose conditions the caller
/// does not meet are left out; a step whose validators would fail is listed, and trying
/// it explains what is missing.
#[utoipa::path(get, path = "/api/v1/issues/{key}/transitions", security(("bearer" = [])), params(("key" = String, Path)),
    responses((status = 200, body = TransitionList), (status = 404)))]
pub async fn transitions(
    State(state): State<AppState>,
    ctx: OrgContext,
    Path(key): Path<String>,
) -> ApiResult<Json<TransitionList>> {
    let (mut tx, issue, role) = open(&state, &ctx, &key, ProjectPermission::Browse, false).await?;
    let children_done = store::children_done(tx.conn(), ctx.tenant_id, issue.id).await?;
    tx.commit().await?;
    if !role.allows(ProjectPermission::TransitionIssue) {
        return Ok(Json(TransitionList { items: Vec::new() }));
    }
    let scope = Scope { tenant_id: ctx.tenant_id, project_id: issue.project_id, issue_type: &issue.type_key };
    let options = state.workflows.transitions(scope, &issue.status, &facts(&ctx, role, &issue, children_done)).await?;
    Ok(Json(TransitionList {
        items: options
            .into_iter()
            .map(|t| TransitionDto {
                id: t.id,
                name: t.name,
                to: StatusRef { key: t.to.key, name: t.to.name, category: t.to.category },
            })
            .collect(),
    }))
}

// --- comments ----------------------------------------------------------------

#[utoipa::path(get, path = "/api/v1/issues/{key}/comments", security(("bearer" = [])), params(("key" = String, Path)),
    responses((status = 200, body = CommentList)))]
pub async fn list_comments(
    State(state): State<AppState>,
    ctx: OrgContext,
    Path(key): Path<String>,
) -> ApiResult<Json<CommentList>> {
    let (mut tx, issue, _) = open(&state, &ctx, &key, ProjectPermission::Browse, false).await?;
    let rows = store::comments(tx.conn(), ctx.tenant_id, issue.id).await?;
    tx.commit().await?;
    Ok(Json(CommentList { items: rows.into_iter().map(Into::into).collect() }))
}

#[utoipa::path(post, path = "/api/v1/issues/{key}/comments", request_body = CommentRequest, security(("bearer" = [])),
    params(("key" = String, Path)), responses((status = 201, body = CommentDto), (status = 422)))]
pub async fn add_comment(
    State(state): State<AppState>,
    ctx: OrgContext,
    Path(key): Path<String>,
    Json(body): Json<CommentRequest>,
) -> ApiResult<(StatusCode, Json<CommentDto>)> {
    let mentions = richtext::validate("body", &body.body)?;
    if !has_text(&body.body) {
        return Err(ApiError::invalid("body", "empty_comment", "Write something before posting."));
    }
    let (mut tx, issue, _) = open(&state, &ctx, &key, ProjectPermission::Comment, false).await?;
    ensure_writable(&state, &ctx, &issue.project_key).await?;
    let comment = store::insert_comment(tx.conn(), ctx.tenant_id, issue.id, ctx.user_id, &body.body, None).await?;
    emit(
        &mut tx,
        &ctx,
        issue.id,
        issue.version,
        CommentCreatedV1 {
            comment_id: comment.id,
            issue_id: issue.id,
            key: issue.key(),
            project_id: issue.project_id,
            author_id: ctx.user_id,
            mentioned_user_ids: mentions,
        },
    )
    .await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(comment.into())))
}

/// Whether a document contains any visible text or mention.
fn has_text(node: &Value) -> bool {
    node.get("text").and_then(Value::as_str).is_some_and(|t| !t.trim().is_empty())
        || node.get("type").and_then(Value::as_str) == Some("mention")
        || node.get("content").and_then(Value::as_array).is_some_and(|c| c.iter().any(has_text))
}

#[utoipa::path(patch, path = "/api/v1/issues/{key}/comments/{id}", request_body = CommentRequest, security(("bearer" = [])),
    params(("key" = String, Path), ("id" = Uuid, Path)), responses((status = 200, body = CommentDto), (status = 403), (status = 404)))]
pub async fn edit_comment(
    State(state): State<AppState>,
    ctx: OrgContext,
    Path((key, id)): Path<(String, Uuid)>,
    Json(body): Json<CommentRequest>,
) -> ApiResult<Json<CommentDto>> {
    let mentions = richtext::validate("body", &body.body)?;
    if !has_text(&body.body) {
        return Err(ApiError::invalid("body", "empty_comment", "A comment cannot be empty."));
    }
    let (mut tx, issue, _) = open(&state, &ctx, &key, ProjectPermission::Comment, false).await?;
    let comment = store::comment(tx.conn(), ctx.tenant_id, issue.id, id).await?.ok_or(ApiError::NotFound("comment"))?;
    // Only the author edits their words — not even administrators.
    if comment.author_id != ctx.user_id {
        return Err(ApiError::Forbidden);
    }
    let updated = store::update_comment(tx.conn(), ctx.tenant_id, id, &body.body).await?;
    // Only people the edit adds are told; those already mentioned heard when they were.
    let added = richtext::newly_mentioned(mentions, Some(&comment.body));
    if !added.is_empty() {
        let mentioned = IssueMentionedV1 {
            issue_id: issue.id,
            key: issue.key(),
            project_id: issue.project_id,
            comment_id: Some(id),
            mentioned_user_ids: added,
        };
        emit(&mut tx, &ctx, issue.id, issue.version, mentioned).await?;
    }
    tx.commit().await?;
    Ok(Json(updated.into()))
}

#[utoipa::path(delete, path = "/api/v1/issues/{key}/comments/{id}", security(("bearer" = [])),
    params(("key" = String, Path), ("id" = Uuid, Path)), responses((status = 204), (status = 403), (status = 404)))]
pub async fn delete_comment(
    State(state): State<AppState>,
    ctx: OrgContext,
    Path((key, id)): Path<(String, Uuid)>,
) -> ApiResult<StatusCode> {
    let (mut tx, issue, role) = open(&state, &ctx, &key, ProjectPermission::Browse, false).await?;
    let comment = store::comment(tx.conn(), ctx.tenant_id, issue.id, id).await?.ok_or(ApiError::NotFound("comment"))?;
    if comment.author_id != ctx.user_id && !role.allows(ProjectPermission::ModerateComments) {
        return Err(ApiError::Forbidden);
    }
    store::delete_comment(tx.conn(), ctx.tenant_id, id).await?;
    emit(
        &mut tx,
        &ctx,
        issue.id,
        issue.version,
        CommentDeletedV1 { comment_id: id, issue_id: issue.id, key: issue.key(), project_id: Some(issue.project_id) },
    )
    .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

// --- links -------------------------------------------------------------------

#[utoipa::path(get, path = "/api/v1/issues/{key}/links", security(("bearer" = [])), params(("key" = String, Path)),
    responses((status = 200, body = LinkList)))]
pub async fn list_links(State(state): State<AppState>, ctx: OrgContext, Path(key): Path<String>) -> ApiResult<Json<LinkList>> {
    let (mut tx, issue, _) = open(&state, &ctx, &key, ProjectPermission::Browse, false).await?;
    let rows = store::links(tx.conn(), ctx.tenant_id, issue.id).await?;
    tx.commit().await?;
    let snapshot = state.policy.snapshot(&ctx).await?;
    let mut items = Vec::new();
    // A link into a project the caller cannot browse is not shown at all.
    for l in rows.into_iter().filter(|l| snapshot.project_role(&ctx, l.other_project_id).is_some()) {
        let (_, outward, inward) =
            LINK_TYPES.iter().find(|(k, ..)| *k == l.link_type).copied().unwrap_or(("", "relates to", "relates to"));
        let (status_name, status_category) =
            status_display(&state, &ctx, l.other_project_id, &l.other_type, &l.other_status, &l.other_status_category).await;
        items.push(LinkDto {
            id: l.id,
            label: if l.outward { outward } else { inward }.to_owned(),
            link_type: l.link_type,
            issue: LinkedIssue {
                key: format!("{}-{}", l.other_project_key, l.other_number),
                summary: l.other_summary,
                issue_type: l.other_type,
                status_name,
                status_category,
                status: l.other_status,
            },
        });
    }
    Ok(Json(LinkList { items }))
}

#[utoipa::path(post, path = "/api/v1/issues/{key}/links", request_body = CreateLinkRequest, security(("bearer" = [])),
    params(("key" = String, Path)), responses((status = 201), (status = 409, description = "Already linked"), (status = 422)))]
pub async fn add_link(
    State(state): State<AppState>,
    ctx: OrgContext,
    Path(key): Path<String>,
    Json(body): Json<CreateLinkRequest>,
) -> ApiResult<StatusCode> {
    domain::validate_link_type(&body.link_type)?;
    let bad_target = || ApiError::invalid("target_key", "unknown_issue", "That issue was not found.");
    let (target_project, target_number) = parse_key(body.target_key.trim()).ok_or_else(bad_target)?;
    let (mut tx, source, _) = open(&state, &ctx, &key, ProjectPermission::EditIssue, false).await?;
    let target =
        store::issue_by_key(tx.conn(), ctx.tenant_id, target_project, target_number, false).await?.ok_or_else(bad_target)?;
    // Linking must not reveal issues in projects the caller cannot see.
    if state.policy.snapshot(&ctx).await?.project_role(&ctx, target.project_id).is_none() {
        return Err(bad_target());
    }
    if target.id == source.id {
        return Err(ApiError::invalid("target_key", "self_link", "An issue cannot be linked to itself."));
    }
    store::insert_link(tx.conn(), ctx.tenant_id, source.id, target.id, &body.link_type, ctx.user_id).await.map_err(|err| {
        match &err {
            sqlx::Error::Database(db) if db.is_unique_violation() => {
                ApiError::Conflict("These issues are already linked that way.".into())
            }
            _ => err.into(),
        }
    })?;
    store::add_history(
        tx.conn(),
        ctx.tenant_id,
        source.id,
        ctx.user_id,
        source.version,
        &json!([{ "field": "link", "from": null, "to": format!("{} {}", body.link_type, target.key()) }]),
    )
    .await?;
    tx.commit().await?;
    Ok(StatusCode::CREATED)
}

#[utoipa::path(delete, path = "/api/v1/issues/{key}/links/{id}", security(("bearer" = [])),
    params(("key" = String, Path), ("id" = Uuid, Path)), responses((status = 204), (status = 404)))]
pub async fn delete_link(
    State(state): State<AppState>,
    ctx: OrgContext,
    Path((key, id)): Path<(String, Uuid)>,
) -> ApiResult<StatusCode> {
    let (mut tx, issue, _) = open(&state, &ctx, &key, ProjectPermission::EditIssue, false).await?;
    let done = store::delete_link(tx.conn(), ctx.tenant_id, issue.id, id).await?;
    tx.commit().await?;
    if done {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound("link"))
    }
}

// --- history & watching ------------------------------------------------------

#[utoipa::path(get, path = "/api/v1/issues/{key}/history", security(("bearer" = [])), params(("key" = String, Path)),
    responses((status = 200, body = HistoryList)))]
pub async fn history(State(state): State<AppState>, ctx: OrgContext, Path(key): Path<String>) -> ApiResult<Json<HistoryList>> {
    let (mut tx, issue, _) = open(&state, &ctx, &key, ProjectPermission::Browse, false).await?;
    let rows = store::history(tx.conn(), ctx.tenant_id, issue.id).await?;
    tx.commit().await?;
    Ok(Json(HistoryList { items: rows.into_iter().map(Into::into).collect() }))
}

#[utoipa::path(put, path = "/api/v1/issues/{key}/watch", security(("bearer" = [])), params(("key" = String, Path)), responses((status = 204)))]
pub async fn watch(State(state): State<AppState>, ctx: OrgContext, Path(key): Path<String>) -> ApiResult<StatusCode> {
    let (mut tx, issue, _) = open(&state, &ctx, &key, ProjectPermission::Browse, false).await?;
    store::watch(tx.conn(), ctx.tenant_id, issue.id, ctx.user_id).await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(delete, path = "/api/v1/issues/{key}/watch", security(("bearer" = [])), params(("key" = String, Path)), responses((status = 204)))]
pub async fn unwatch(State(state): State<AppState>, ctx: OrgContext, Path(key): Path<String>) -> ApiResult<StatusCode> {
    let (mut tx, issue, _) = open(&state, &ctx, &key, ProjectPermission::Browse, false).await?;
    store::unwatch(tx.conn(), ctx.tenant_id, issue.id, ctx.user_id).await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

// --- votes -------------------------------------------------------------------

/// Vote for an issue: anyone who can see it, once. Resolved issues take no new votes.
#[utoipa::path(put, path = "/api/v1/issues/{key}/vote", security(("bearer" = [])), params(("key" = String, Path)),
    responses((status = 204), (status = 404), (status = 409, description = "The issue is resolved")))]
pub async fn vote(State(state): State<AppState>, ctx: OrgContext, Path(key): Path<String>) -> ApiResult<StatusCode> {
    let (mut tx, issue, _) = open(&state, &ctx, &key, ProjectPermission::Browse, false).await?;
    if issue.status_category == "done" {
        let (voted, _) = store::vote_state(tx.conn(), ctx.tenant_id, issue.id, ctx.user_id).await?;
        if !voted {
            return Err(ApiError::Conflict("Voting is closed: this issue is resolved.".into()));
        }
    }
    store::vote(tx.conn(), ctx.tenant_id, issue.id, ctx.user_id).await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Take back a vote, whatever the issue's state.
#[utoipa::path(delete, path = "/api/v1/issues/{key}/vote", security(("bearer" = [])), params(("key" = String, Path)), responses((status = 204)))]
pub async fn unvote(State(state): State<AppState>, ctx: OrgContext, Path(key): Path<String>) -> ApiResult<StatusCode> {
    let (mut tx, issue, _) = open(&state, &ctx, &key, ProjectPermission::Browse, false).await?;
    store::unvote(tx.conn(), ctx.tenant_id, issue.id, ctx.user_id).await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(get, path = "/api/v1/issues/{key}/voters", security(("bearer" = [])), params(("key" = String, Path)),
    responses((status = 200, body = VoterList), (status = 404)))]
pub async fn voters(State(state): State<AppState>, ctx: OrgContext, Path(key): Path<String>) -> ApiResult<Json<VoterList>> {
    let (mut tx, issue, _) = open(&state, &ctx, &key, ProjectPermission::Browse, false).await?;
    let rows = store::voters(tx.conn(), ctx.tenant_id, issue.id).await?;
    tx.commit().await?;
    Ok(Json(VoterList { items: rows.into_iter().map(|(user_id, voted_at)| VoterDto { user_id, voted_at }).collect() }))
}

// --- router ------------------------------------------------------------------

#[derive(OpenApi)]
#[openapi(
    info(title = "Issues API", version = "1.0.0", description = "Issues, comments, links, history and watching."),
    paths(
        meta,
        create,
        list,
        get_one,
        update,
        remove,
        list_comments,
        add_comment,
        edit_comment,
        delete_comment,
        list_links,
        add_link,
        clone_issue,
        vote,
        unvote,
        voters,
        delete_link,
        history,
        transitions,
        watch,
        unwatch,
        crate::http_fields::list,
        crate::http_automation::list,
        crate::http_automation::create,
        crate::http_automation::update,
        crate::http_automation::delete,
        crate::http_automation::runs,
        crate::http_fields::create,
        crate::http_fields::update,
        crate::http_fields::archive,
        crate::attachments::list,
        crate::attachments::upload,
        crate::attachments::download,
        crate::attachments::remove
    ),
    components(schemas(
        IssueDto,
        IssueDetail,
        IssueRef,
        IssuePage,
        CreateIssueRequest,
        UpdateIssueRequest,
        CommentDto,
        CommentList,
        CommentRequest,
        LinkDto,
        LinkedIssue,
        LinkList,
        CreateLinkRequest,
        CloneIssueRequest,
        crate::http_automation::AutomationRuleDto,
        crate::http_automation::AutomationRuleList,
        crate::http_automation::CreateAutomationRuleRequest,
        crate::http_automation::UpdateAutomationRuleRequest,
        crate::http_automation::AutomationRunDto,
        crate::http_automation::AutomationRunList,
        crate::automation::RuleDefinition,
        crate::automation::Trigger,
        crate::automation::Conditions,
        crate::automation::Action,
        crate::automation::Assignee,
        VoterDto,
        VoterList,
        HistoryEntry,
        HistoryList,
        IssueMeta,
        MetaType,
        StatusRef,
        TransitionDto,
        TransitionList,
        MetaLinkType,
        CustomFieldDto,
        CustomFieldList,
        CreateCustomFieldRequest,
        UpdateCustomFieldRequest,
        AttachmentDto,
        AttachmentList
    ))
)]
pub struct ApiDoc;

pub fn router(state: AppState) -> Router {
    let api = Router::new()
        .route("/issues", get(list).post(create))
        .route("/issues/meta", get(meta))
        .route("/issues/{key}", get(get_one).patch(update).delete(remove))
        .route("/issues/{key}/comments", get(list_comments).post(add_comment))
        .route("/issues/{key}/comments/{id}", patch(edit_comment).delete(delete_comment))
        .route("/issues/{key}/links", get(list_links).post(add_link))
        .route("/issues/{key}/clone", axum::routing::post(clone_issue))
        .route("/issues/{key}/links/{id}", delete(delete_link))
        .route("/issues/{key}/history", get(history))
        .route("/issues/{key}/transitions", get(transitions))
        .route("/issues/{key}/watch", put(watch).delete(unwatch))
        .route("/issues/{key}/vote", put(vote).delete(unvote))
        .route("/issues/{key}/voters", get(voters))
        .merge(crate::http_fields::routes())
        .merge(crate::http_automation::routes())
        .merge(crate::attachments::routes());

    let origins: Vec<_> = state.allowed_origins.iter().filter_map(|o| o.parse().ok()).collect();
    let cors = CorsLayer::new()
        .allow_origin(AllowOrigin::list(origins))
        .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE, Method::PATCH])
        .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE, header::IF_MATCH])
        .expose_headers([header::ETAG]);

    Router::new()
        .nest("/api/v1", api)
        .route("/openapi.json", get(|| async { Json(ApiDoc::openapi()) }))
        .layer(cors)
        .with_state(state)
}
