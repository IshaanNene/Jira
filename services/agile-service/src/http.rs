//! Public REST API: boards, the backlog, sprints and sprint reports.

use std::collections::{HashMap, HashSet};

use auth_middleware::{OrgContext, ProjectPermission, ProjectRole};
use axum::{
    extract::{Path, Query, State},
    http::{
        header::{self, ETAG},
        HeaderMap, HeaderValue, Method, StatusCode,
    },
    routing::{get, post},
    Json, Router,
};
use error_handling::{require_if_match, ApiError, ApiResult};
use event_contracts::{
    envelope::EventPayload,
    outbox,
    payloads::{BoardUpdatedV1, IssueRankedV1, SprintCompletedV1, SprintCreatedV1, SprintStartedV1},
    Actor, EventEnvelope,
};
use serde::{Deserialize, Deserializer, Serialize};
use tenant_context::TenantTx;
use time::{Date, Duration, OffsetDateTime};
use tower_http::cors::{AllowOrigin, CorsLayer};
use utoipa::{OpenApi, ToSchema};
use uuid::Uuid;

use crate::{
    domain::{burndown, column_for, default_columns, validate_columns, BurndownSample, Column, IssueFact, ScopeChange},
    rank,
    sources::{Card, OFF_BOARD_TYPES},
    store, AppState,
};

time::serde::format_description!(iso_date, Date, "[year]-[month]-[day]");

/// Distinguishes "field absent" (leave unchanged) from "field: null" (clear it).
fn nullable<'de, T: Deserialize<'de>, D: Deserializer<'de>>(d: D) -> Result<Option<Option<T>>, D::Error> {
    Option::<T>::deserialize(d).map(Some)
}

fn nullable_date<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Option<Date>>, D::Error> {
    iso_date::option::deserialize(d).map(Some)
}

const BOARD_NOT_FOUND: ApiError = ApiError::NotFound("board");
const SPRINT_NOT_FOUND: ApiError = ApiError::NotFound("sprint");
/// Finished issues stay on a board this long.
const DONE_WINDOW_DAYS: i64 = 14;
const MAX_MOVE: usize = 100;
const DEFAULT_SPRINT_DAYS: i64 = 14;

// --- DTOs ----------------------------------------------------------------------

/// What the caller may do on this board.
#[derive(Debug, Serialize, ToSchema)]
pub struct BoardPermissions {
    /// Rename, change columns, delete.
    pub manage_board: bool,
    /// Create, start, complete and delete sprints.
    pub manage_sprints: bool,
    /// Reorder issues and move them between sprints and the backlog.
    pub move_issues: bool,
    /// Change an issue's status by dragging its card (the workflow still decides).
    pub transition_issues: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct BoardDto {
    pub id: Uuid,
    pub project_id: Uuid,
    pub project_key: String,
    pub name: String,
    /// scrum | kanban
    pub kind: String,
    pub columns: Vec<Column>,
    /// Send back in `If-Match` when editing.
    pub version: i64,
    pub permissions: BoardPermissions,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct BoardList {
    pub items: Vec<BoardDto>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct BoardQuery {
    pub project_key: String,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateBoardRequest {
    pub project_key: String,
    pub name: String,
    /// scrum | kanban
    pub kind: String,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateBoardRequest {
    pub name: Option<String>,
    pub columns: Option<Vec<Column>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct Commitment {
    pub issues: u32,
    pub points: f64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SprintDto {
    pub id: Uuid,
    pub board_id: Uuid,
    pub sequence: i32,
    pub name: String,
    pub goal: String,
    /// future | active | closed
    pub state: String,
    #[serde(with = "iso_date::option")]
    #[schema(value_type = Option<String>, format = Date)]
    pub start_date: Option<Date>,
    #[serde(with = "iso_date::option")]
    #[schema(value_type = Option<String>, format = Date)]
    pub end_date: Option<Date>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub started_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub completed_at: Option<OffsetDateTime>,
    /// What the sprint contained when it started.
    pub commitment: Option<Commitment>,
    pub version: i64,
}

impl From<&store::Sprint> for SprintDto {
    fn from(s: &store::Sprint) -> Self {
        Self {
            id: s.id,
            board_id: s.board_id,
            sequence: s.sequence,
            name: s.name.clone(),
            goal: s.goal.clone(),
            state: s.state.clone(),
            start_date: s.start_date,
            end_date: s.end_date,
            started_at: s.started_at,
            completed_at: s.completed_at,
            commitment: s.commitment.as_ref().and_then(|c| serde_json::from_value(c.clone()).ok()),
            version: s.version,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SprintList {
    pub items: Vec<SprintDto>,
}

#[derive(Debug, Default, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateSprintRequest {
    /// Omit for "<project key> Sprint <n>".
    pub name: Option<String>,
    #[serde(default)]
    pub goal: String,
    #[serde(default, with = "iso_date::option")]
    #[schema(value_type = Option<String>, format = Date)]
    pub start_date: Option<Date>,
    #[serde(default, with = "iso_date::option")]
    #[schema(value_type = Option<String>, format = Date)]
    pub end_date: Option<Date>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateSprintRequest {
    pub name: Option<String>,
    pub goal: Option<String>,
    #[serde(default, deserialize_with = "nullable_date")]
    #[schema(value_type = Option<String>, format = Date)]
    pub start_date: Option<Option<Date>>,
    #[serde(default, deserialize_with = "nullable_date")]
    #[schema(value_type = Option<String>, format = Date)]
    pub end_date: Option<Option<Date>>,
}

#[derive(Debug, Default, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct StartSprintRequest {
    pub name: Option<String>,
    pub goal: Option<String>,
    /// Planned last day. Omit to keep the planned date, or two weeks from today.
    #[serde(default, with = "iso_date::option")]
    #[schema(value_type = Option<String>, format = Date)]
    pub end_date: Option<Date>,
}

#[derive(Debug, Default, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CompleteSprintRequest {
    /// A planned sprint of the same board to receive unfinished issues; omit for the backlog.
    pub move_incomplete_to: Option<Uuid>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct CardDto {
    pub id: Uuid,
    pub key: String,
    #[serde(rename = "type")]
    pub issue_type: String,
    pub status: String,
    pub status_category: String,
    pub priority: String,
    pub summary: String,
    pub assignee_id: Option<Uuid>,
    pub story_points: Option<f64>,
    pub parent_key: Option<String>,
    pub parent_summary: Option<String>,
    pub labels: Vec<String>,
    /// The issue's version, for `If-Match` when changing its status from the board.
    pub version: i64,
    pub sprint_id: Option<Uuid>,
}

impl CardDto {
    fn new(card: &Card, sprint_id: Option<Uuid>) -> Self {
        Self {
            id: card.id,
            key: card.key.clone(),
            issue_type: card.issue_type.clone(),
            status: card.status.clone(),
            status_category: card.status_category.clone(),
            priority: card.priority.clone(),
            summary: card.summary.clone(),
            assignee_id: card.assignee_id,
            story_points: card.story_points,
            parent_key: card.parent_key.clone(),
            parent_summary: card.parent_summary.clone(),
            labels: card.labels.clone(),
            version: card.version,
            sprint_id,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ColumnView {
    #[serde(flatten)]
    pub column: Column,
    pub issues: Vec<CardDto>,
}

/// An epic the board's issues can be filtered by; issues belong to it through `parent_key`.
#[derive(Debug, Serialize, ToSchema)]
pub struct EpicDto {
    pub id: Uuid,
    pub key: String,
    pub summary: String,
    pub status_category: String,
    /// The epic's child issues in the project, of any status.
    pub issues: i64,
    /// How many of them are done.
    pub done: i64,
    /// Their story points in all, and of those done.
    pub points: f64,
    pub done_points: f64,
}

impl EpicDto {
    fn new(card: &Card, progress: Option<&crate::cards::EpicProgress>) -> Self {
        Self {
            id: card.id,
            key: card.key.clone(),
            summary: card.summary.clone(),
            status_category: card.status_category.clone(),
            issues: progress.map_or(0, |p| p.issues),
            done: progress.map_or(0, |p| p.done),
            points: progress.map_or(0.0, |p| p.points),
            done_points: progress.map_or(0.0, |p| p.done_points),
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct BoardView {
    pub board: BoardDto,
    /// Scrum boards: the running sprint, or null when none is running (the board is then empty).
    pub sprint: Option<SprintDto>,
    pub columns: Vec<ColumnView>,
    /// Issues whose status no column shows.
    pub unmapped: Vec<CardDto>,
    /// The project has more open issues than one board can load.
    pub truncated: bool,
    /// The project's unfinished (and recently finished) epics, in creation order.
    pub epics: Vec<EpicDto>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SprintGroup {
    pub sprint: SprintDto,
    pub issues: Vec<CardDto>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct BacklogView {
    pub board: BoardDto,
    /// The running sprint first, then planned sprints in order.
    pub sprints: Vec<SprintGroup>,
    /// Unfinished issues that are in no planned or running sprint, in rank order.
    pub backlog: Vec<CardDto>,
    pub truncated: bool,
    /// The project's unfinished (and recently finished) epics, in creation order.
    pub epics: Vec<EpicDto>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct MoveRequest {
    /// Issues to move, in the order they should end up in.
    pub issue_ids: Vec<Uuid>,
    /// Omit to leave sprint membership alone; a sprint id to plan the issues into it;
    /// null to return them to the backlog.
    #[serde(default, deserialize_with = "nullable")]
    #[schema(value_type = Option<Uuid>)]
    pub sprint_id: Option<Option<Uuid>>,
    /// Place the issues directly after this issue…
    pub after_id: Option<Uuid>,
    /// …or directly before this one. With neither, they go to the bottom.
    pub before_id: Option<Uuid>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ReportIssue {
    pub id: Uuid,
    pub key: String,
    pub summary: String,
    #[serde(rename = "type")]
    pub issue_type: String,
    pub story_points: Option<f64>,
}

/// How a sprint went. Stored when the sprint is completed; computed live while it runs.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SprintOutcome {
    pub completed: Vec<ReportIssue>,
    pub incomplete: Vec<ReportIssue>,
    pub completed_points: f64,
    pub incomplete_points: f64,
    pub burndown: Vec<BurndownSample>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SprintReport {
    pub sprint: SprintDto,
    pub outcome: SprintOutcome,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct VelocityEntry {
    pub sprint_id: Uuid,
    pub name: String,
    pub committed_issues: u32,
    pub committed_points: f64,
    pub completed_issues: u32,
    pub completed_points: f64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct VelocityReport {
    /// Completed sprints, oldest first (at most the last ten).
    pub sprints: Vec<VelocityEntry>,
}

// --- helpers -------------------------------------------------------------------

fn etag(version: i64) -> HeaderMap {
    let mut headers = HeaderMap::new();
    if let Ok(v) = HeaderValue::from_str(&format!("\"{version}\"")) {
        headers.insert(ETAG, v);
    }
    headers
}

fn columns_of(board: &store::Board) -> ApiResult<Vec<Column>> {
    serde_json::from_value(board.columns.clone())
        .map_err(|e| ApiError::internal(anyhow::anyhow!("stored board columns are invalid: {e}")))
}

fn board_dto(board: &store::Board, role: ProjectRole) -> ApiResult<BoardDto> {
    Ok(BoardDto {
        id: board.id,
        project_id: board.project_id,
        project_key: board.project_key.clone(),
        name: board.name.clone(),
        kind: board.kind.clone(),
        columns: columns_of(board)?,
        version: board.version,
        permissions: BoardPermissions {
            manage_board: role.allows(ProjectPermission::ManageProject),
            manage_sprints: role.allows(ProjectPermission::ManageSprints),
            move_issues: role.allows(ProjectPermission::EditIssue),
            transition_issues: role.allows(ProjectPermission::TransitionIssue),
        },
    })
}

fn validate_name(name: &str, what: &str) -> ApiResult<String> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 80 {
        return Err(ApiError::invalid("name", "invalid_name", &format!("The {what} name must be between 1 and 80 characters.")));
    }
    Ok(name.to_owned())
}

fn validate_goal(goal: &str) -> ApiResult<String> {
    let goal = goal.trim();
    if goal.chars().count() > 500 {
        return Err(ApiError::invalid("goal", "too_long", "The sprint goal must be 500 characters or fewer."));
    }
    Ok(goal.to_owned())
}

fn validate_dates(start: Option<Date>, end: Option<Date>) -> ApiResult<()> {
    match (start, end) {
        (Some(start), Some(end)) if end < start => {
            Err(ApiError::invalid("end_date", "before_start", "The end date cannot be before the start date."))
        }
        _ => Ok(()),
    }
}

fn duplicate(what: &'static str) -> impl Fn(sqlx::Error) -> ApiError {
    move |err| match &err {
        sqlx::Error::Database(db) if db.is_unique_violation() => ApiError::Conflict(what.into()),
        _ => err.into(),
    }
}

/// A caller without any role on the project must not learn that the board exists.
fn hide(err: ApiError, not_found: ApiError) -> ApiError {
    match err {
        ApiError::NotFound(_) => not_found,
        other => other,
    }
}

async fn open_board(
    state: &AppState,
    ctx: &OrgContext,
    id: Uuid,
    permission: ProjectPermission,
) -> ApiResult<(store::Board, ProjectRole)> {
    let mut tx = state.db.begin(ctx.tenant_id).await?;
    let board = store::board(tx.conn(), ctx.tenant_id, id).await?.ok_or(BOARD_NOT_FOUND)?;
    tx.commit().await?;
    let role =
        state.policy.snapshot(ctx).await?.require(ctx, board.project_id, permission).map_err(|e| hide(e, BOARD_NOT_FOUND))?;
    Ok((board, role))
}

async fn open_sprint(
    state: &AppState,
    ctx: &OrgContext,
    id: Uuid,
    permission: ProjectPermission,
) -> ApiResult<(store::Sprint, store::Board, ProjectRole)> {
    let mut tx = state.db.begin(ctx.tenant_id).await?;
    let sprint = store::sprint(tx.conn(), ctx.tenant_id, id, false).await?.ok_or(SPRINT_NOT_FOUND)?;
    let board = store::board(tx.conn(), ctx.tenant_id, sprint.board_id).await?.ok_or(SPRINT_NOT_FOUND)?;
    tx.commit().await?;
    let role =
        state.policy.snapshot(ctx).await?.require(ctx, board.project_id, permission).map_err(|e| hide(e, SPRINT_NOT_FOUND))?;
    Ok((sprint, board, role))
}

async fn emit<P: EventPayload + Serialize>(
    tx: &mut TenantTx,
    ctx: &OrgContext,
    board: &store::Board,
    payload: P,
) -> ApiResult<()> {
    outbox::enqueue(
        tx.conn(),
        &EventEnvelope::new(Some(ctx.tenant_id), board.id, board.version, Actor::User(ctx.user_id), payload),
    )
    .await?;
    Ok(())
}

/// The cards of a project together with their order.
struct Loaded {
    cards: HashMap<Uuid, Card>,
    placements: Vec<store::Placement>,
    truncated: bool,
    epics: Vec<EpicDto>,
}

/// The project's open cards from the local copy (filled on a project's first load), making
/// sure each has a rank: issues the board has not seen before are appended to the bottom in
/// creation order.
async fn load(state: &AppState, ctx: &OrgContext, board: &store::Board) -> ApiResult<Loaded> {
    crate::cards::ensure_synced(&state.db, state.issues.as_ref(), ctx.tenant_id, board.project_id).await?;
    let since = OffsetDateTime::now_utc() - Duration::days(DONE_WINDOW_DAYS);
    let mut tx = state.db.begin(ctx.tenant_id).await?;
    let mut open =
        crate::cards::open(tx.conn(), ctx.tenant_id, board.project_id, since, crate::cards::MAX_BOARD_CARDS + 1).await?;
    let truncated = open.len() as i64 > crate::cards::MAX_BOARD_CARDS;
    open.truncate(crate::cards::MAX_BOARD_CARDS as usize);
    let mut cards: HashMap<Uuid, Card> = open.into_iter().map(|c| (c.id, c)).collect();

    let mut placements = store::placements(tx.conn(), ctx.tenant_id, board.project_id).await?;
    let placed: HashSet<Uuid> = placements.iter().map(|p| p.issue_id).collect();
    let mut unranked: Vec<&Card> = cards.values().filter(|c| !placed.contains(&c.id)).collect();
    if !unranked.is_empty() {
        unranked.sort_by_key(|c| c.number);
        store::lock_ranks(tx.conn(), board.project_id).await?;
        let last = store::neighbour_rank(tx.conn(), ctx.tenant_id, board.project_id, None, true, &[]).await?;
        for (card, rank) in unranked.iter().zip(rank::spread(last.as_deref(), None, unranked.len())) {
            store::place_if_absent(tx.conn(), ctx.tenant_id, board.project_id, card.id, &rank).await?;
        }
        placements = store::placements(tx.conn(), ctx.tenant_id, board.project_id).await?;
    }

    // Issues planned into a sprint stay visible there however long ago they were finished.
    let extra: Vec<Uuid> =
        placements.iter().filter(|p| p.in_open_sprint() && !cards.contains_key(&p.issue_id)).map(|p| p.issue_id).collect();
    if !extra.is_empty() {
        for card in crate::cards::by_ids(tx.conn(), ctx.tenant_id, board.project_id, &extra).await? {
            cards.insert(card.id, card);
        }
    }
    let epic_cards = crate::cards::epics(tx.conn(), ctx.tenant_id, board.project_id, since).await?;
    let keys: Vec<String> = epic_cards.iter().map(|c| c.key.clone()).collect();
    let progress: HashMap<String, crate::cards::EpicProgress> =
        crate::cards::epic_progress(tx.conn(), ctx.tenant_id, board.project_id, &keys)
            .await?
            .into_iter()
            .map(|p| (p.key.clone(), p))
            .collect();
    let epics = epic_cards.iter().map(|c| EpicDto::new(c, progress.get(&c.key))).collect();
    tx.commit().await?;
    Ok(Loaded { cards, placements, truncated, epics })
}

async fn board_sprints(state: &AppState, ctx: &OrgContext, board_id: Uuid) -> ApiResult<Vec<store::Sprint>> {
    let mut tx = state.db.begin(ctx.tenant_id).await?;
    let sprints = store::sprints(tx.conn(), ctx.tenant_id, board_id).await?;
    tx.commit().await?;
    Ok(sprints)
}

fn points(cards: &[&Card]) -> f64 {
    cards.iter().filter_map(|c| c.story_points).sum()
}

fn report_issue(card: &Card) -> ReportIssue {
    ReportIssue {
        id: card.id,
        key: card.key.clone(),
        summary: card.summary.clone(),
        issue_type: card.issue_type.clone(),
        story_points: card.story_points,
    }
}

/// Outcome of a sprint from its current members, its scope log and the issues' present state.
fn outcome(
    started_at: OffsetDateTime,
    until: OffsetDateTime,
    members: &[Uuid],
    log: &[(Uuid, bool, OffsetDateTime)],
    cards: &HashMap<Uuid, Card>,
) -> SprintOutcome {
    let present: Vec<&Card> = members.iter().filter_map(|id| cards.get(id)).collect();
    let (completed, incomplete): (Vec<&Card>, Vec<&Card>) = present.into_iter().partition(|c| c.done());
    let changes: Vec<ScopeChange> =
        log.iter().map(|(issue_id, added, at)| ScopeChange { issue_id: *issue_id, added: *added, at: *at }).collect();
    let facts: HashMap<Uuid, IssueFact> = cards
        .iter()
        .map(|(id, c)| (*id, IssueFact { points: c.story_points, done: c.done(), resolved_at: c.resolved_at }))
        .collect();
    SprintOutcome {
        completed_points: points(&completed),
        incomplete_points: points(&incomplete),
        completed: completed.into_iter().map(report_issue).collect(),
        incomplete: incomplete.into_iter().map(report_issue).collect(),
        burndown: burndown(started_at, until, &changes, &facts),
    }
}

/// Members, scope log and the cards of every issue either mentions.
async fn sprint_material(
    state: &AppState,
    ctx: &OrgContext,
    sprint: &store::Sprint,
) -> ApiResult<(Vec<Uuid>, Vec<(Uuid, bool, OffsetDateTime)>, HashMap<Uuid, Card>)> {
    let mut tx = state.db.begin(ctx.tenant_id).await?;
    let members = store::sprint_issue_ids(tx.conn(), ctx.tenant_id, sprint.id).await?;
    let log = store::scope_log(tx.conn(), ctx.tenant_id, sprint.id).await?;
    tx.commit().await?;
    let mut ids: Vec<Uuid> = members.iter().copied().chain(log.iter().map(|(id, _, _)| *id)).collect();
    ids.sort_unstable();
    ids.dedup();
    let cards = if ids.is_empty() { vec![] } else { state.issues.cards_by_id(ctx.tenant_id, sprint.project_id, &ids).await? };
    Ok((members, log, cards.into_iter().map(|c| (c.id, c)).collect()))
}

// --- boards --------------------------------------------------------------------

#[utoipa::path(get, path = "/api/v1/boards", security(("bearer" = [])), params(("project_key" = String, Query)),
    responses((status = 200, body = BoardList), (status = 404)))]
pub async fn list_boards(
    State(state): State<AppState>,
    ctx: OrgContext,
    Query(q): Query<BoardQuery>,
) -> ApiResult<Json<BoardList>> {
    let project = state.projects.project(ctx.tenant_id, &q.project_key).await?;
    let role = state.policy.snapshot(&ctx).await?.require(&ctx, project.id, ProjectPermission::Browse)?;
    let mut tx = state.db.begin(ctx.tenant_id).await?;
    let rows = store::boards(tx.conn(), ctx.tenant_id, project.id).await?;
    tx.commit().await?;
    Ok(Json(BoardList { items: rows.iter().map(|b| board_dto(b, role)).collect::<ApiResult<_>>()? }))
}

/// New boards start with To Do / In Progress / Done columns that follow status categories.
#[utoipa::path(post, path = "/api/v1/boards", request_body = CreateBoardRequest, security(("bearer" = [])),
    responses((status = 201, body = BoardDto), (status = 403), (status = 404), (status = 409), (status = 422)))]
pub async fn create_board(
    State(state): State<AppState>,
    ctx: OrgContext,
    Json(body): Json<CreateBoardRequest>,
) -> ApiResult<(StatusCode, HeaderMap, Json<BoardDto>)> {
    let project = state.projects.project(ctx.tenant_id, &body.project_key).await?;
    let role = state.policy.snapshot(&ctx).await?.require(&ctx, project.id, ProjectPermission::ManageProject)?;
    if project.archived {
        return Err(ApiError::Conflict("This project is archived.".into()));
    }
    let name = validate_name(&body.name, "board")?;
    if !matches!(body.kind.as_str(), "scrum" | "kanban") {
        return Err(ApiError::invalid("kind", "invalid_kind", "The board type must be scrum or kanban."));
    }
    let columns = serde_json::to_value(default_columns()).map_err(ApiError::internal)?;
    let mut tx = state.db.begin(ctx.tenant_id).await?;
    let board = store::insert_board(
        tx.conn(),
        ctx.tenant_id,
        platform_common::new_id(),
        project.id,
        &project.key,
        &name,
        &body.kind,
        &columns,
        ctx.user_id,
    )
    .await
    .map_err(duplicate("This project already has a board with that name."))?;
    emit(&mut tx, &ctx, &board, BoardUpdatedV1 { board_id: board.id, project_id: board.project_id, change: "created".into() })
        .await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, etag(board.version), Json(board_dto(&board, role)?)))
}

#[utoipa::path(get, path = "/api/v1/boards/{id}", security(("bearer" = [])), params(("id" = Uuid, Path)),
    responses((status = 200, body = BoardDto), (status = 404)))]
pub async fn get_board(
    State(state): State<AppState>,
    ctx: OrgContext,
    Path(id): Path<Uuid>,
) -> ApiResult<(HeaderMap, Json<BoardDto>)> {
    let (board, role) = open_board(&state, &ctx, id, ProjectPermission::Browse).await?;
    Ok((etag(board.version), Json(board_dto(&board, role)?)))
}

#[utoipa::path(patch, path = "/api/v1/boards/{id}", request_body = UpdateBoardRequest, security(("bearer" = [])),
    params(("id" = Uuid, Path), ("If-Match" = String, Header)),
    responses((status = 200, body = BoardDto), (status = 403), (status = 404), (status = 409), (status = 422), (status = 428)))]
pub async fn update_board(
    State(state): State<AppState>,
    ctx: OrgContext,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Json(body): Json<UpdateBoardRequest>,
) -> ApiResult<(HeaderMap, Json<BoardDto>)> {
    let expected = require_if_match(&headers)?;
    let (board, role) = open_board(&state, &ctx, id, ProjectPermission::ManageProject).await?;
    let name = body.name.as_deref().map(|n| validate_name(n, "board")).transpose()?.unwrap_or_else(|| board.name.clone());
    let (columns, change) = match body.columns {
        Some(columns) => (serde_json::to_value(validate_columns(columns)?).map_err(ApiError::internal)?, "columns_changed"),
        None => (board.columns.clone(), "renamed"),
    };
    let mut tx = state.db.begin(ctx.tenant_id).await?;
    let updated = store::update_board(tx.conn(), ctx.tenant_id, id, expected, &name, &columns)
        .await
        .map_err(duplicate("This project already has a board with that name."))?;
    let Some(updated) = updated else {
        let current = store::board(tx.conn(), ctx.tenant_id, id).await?.ok_or(BOARD_NOT_FOUND)?;
        return Err(ApiError::VersionConflict { current: serde_json::to_value(board_dto(&current, role)?).ok() });
    };
    emit(&mut tx, &ctx, &updated, BoardUpdatedV1 { board_id: id, project_id: updated.project_id, change: change.into() }).await?;
    tx.commit().await?;
    Ok((etag(updated.version), Json(board_dto(&updated, role)?)))
}

/// Planned sprints are removed with the board (their issues return to the backlog);
/// completed sprints are kept. A board with a running sprint cannot be deleted.
#[utoipa::path(delete, path = "/api/v1/boards/{id}", security(("bearer" = [])), params(("id" = Uuid, Path)),
    responses((status = 204), (status = 403), (status = 404), (status = 409)))]
pub async fn delete_board(State(state): State<AppState>, ctx: OrgContext, Path(id): Path<Uuid>) -> ApiResult<StatusCode> {
    let (board, _) = open_board(&state, &ctx, id, ProjectPermission::ManageProject).await?;
    let mut tx = state.db.begin(ctx.tenant_id).await?;
    if store::sprints(tx.conn(), ctx.tenant_id, id).await?.iter().any(|s| s.state == "active") {
        return Err(ApiError::Conflict("Complete the running sprint before deleting this board.".into()));
    }
    store::delete_board(tx.conn(), ctx.tenant_id, id).await?;
    emit(&mut tx, &ctx, &board, BoardUpdatedV1 { board_id: id, project_id: board.project_id, change: "deleted".into() }).await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

/// The board itself: columns with their cards in rank order. A Scrum board shows the
/// running sprint; a Kanban board shows every open issue and those finished in the last
/// two weeks.
#[utoipa::path(get, path = "/api/v1/boards/{id}/view", security(("bearer" = [])), params(("id" = Uuid, Path)),
    responses((status = 200, body = BoardView), (status = 404)))]
pub async fn view(State(state): State<AppState>, ctx: OrgContext, Path(id): Path<Uuid>) -> ApiResult<Json<BoardView>> {
    let (board, role) = open_board(&state, &ctx, id, ProjectPermission::Browse).await?;
    let columns = columns_of(&board)?;
    let scrum = board.kind == "scrum";
    let active = if scrum { board_sprints(&state, &ctx, id).await?.into_iter().find(|s| s.state == "active") } else { None };
    let mut out: Vec<ColumnView> = columns.iter().cloned().map(|column| ColumnView { column, issues: vec![] }).collect();
    let mut unmapped = Vec::new();
    let mut truncated = false;
    let mut epics = Vec::new();

    if !scrum || active.is_some() {
        let loaded = load(&state, &ctx, &board).await?;
        truncated = loaded.truncated;
        epics = loaded.epics;
        let active_id = active.as_ref().map(|s| s.id);
        for placement in &loaded.placements {
            let Some(card) = loaded.cards.get(&placement.issue_id) else { continue };
            if scrum && placement.sprint_id != active_id {
                continue;
            }
            let dto = CardDto::new(card, placement.sprint_id.filter(|_| placement.in_open_sprint()));
            match column_for(&columns, &card.status, &card.status_category) {
                Some(index) => out[index].issues.push(dto),
                None => unmapped.push(dto),
            }
        }
    }
    Ok(Json(BoardView {
        board: board_dto(&board, role)?,
        sprint: active.as_ref().map(SprintDto::from),
        columns: out,
        unmapped,
        truncated,
        epics,
    }))
}

/// Planning view: running and planned sprints with their issues, then the backlog.
#[utoipa::path(get, path = "/api/v1/boards/{id}/backlog", security(("bearer" = [])), params(("id" = Uuid, Path)),
    responses((status = 200, body = BacklogView), (status = 404)))]
pub async fn backlog(State(state): State<AppState>, ctx: OrgContext, Path(id): Path<Uuid>) -> ApiResult<Json<BacklogView>> {
    let (board, role) = open_board(&state, &ctx, id, ProjectPermission::Browse).await?;
    let loaded = load(&state, &ctx, &board).await?;
    let mut groups: Vec<SprintGroup> = board_sprints(&state, &ctx, id)
        .await?
        .iter()
        .filter(|s| s.state != "closed")
        .map(|s| SprintGroup { sprint: SprintDto::from(s), issues: vec![] })
        .collect();
    let mut backlog = Vec::new();
    for placement in &loaded.placements {
        let Some(card) = loaded.cards.get(&placement.issue_id) else { continue };
        if placement.in_open_sprint() {
            if let Some(group) = groups.iter_mut().find(|g| Some(g.sprint.id) == placement.sprint_id) {
                group.issues.push(CardDto::new(card, placement.sprint_id));
            }
        } else if !card.done() {
            backlog.push(CardDto::new(card, None));
        }
    }
    Ok(Json(BacklogView {
        board: board_dto(&board, role)?,
        sprints: groups,
        backlog,
        truncated: loaded.truncated,
        epics: loaded.epics,
    }))
}

/// Reorder issues and/or move them between sprints and the backlog. Order is shared by
/// every board of the project.
#[utoipa::path(post, path = "/api/v1/boards/{id}/move", request_body = MoveRequest, security(("bearer" = [])),
    params(("id" = Uuid, Path)),
    responses((status = 204), (status = 403), (status = 404), (status = 422)))]
pub async fn move_issues(
    State(state): State<AppState>,
    ctx: OrgContext,
    Path(id): Path<Uuid>,
    Json(body): Json<MoveRequest>,
) -> ApiResult<StatusCode> {
    let (board, _) = open_board(&state, &ctx, id, ProjectPermission::EditIssue).await?;
    let ids = &body.issue_ids;
    let unique: HashSet<&Uuid> = ids.iter().collect();
    if ids.is_empty() || ids.len() > MAX_MOVE || unique.len() != ids.len() {
        return Err(ApiError::invalid(
            "issue_ids",
            "invalid_selection",
            &format!("Choose between 1 and {MAX_MOVE} different issues."),
        ));
    }
    let anchor = match (body.after_id, body.before_id) {
        (Some(_), Some(_)) => {
            return Err(ApiError::invalid("before_id", "ambiguous_position", "Give either after_id or before_id."))
        }
        (after, before) => after.or(before),
    };
    if anchor.is_some_and(|a| unique.contains(&a)) {
        return Err(ApiError::invalid("issue_ids", "invalid_position", "An issue cannot be placed next to itself."));
    }

    // The issues must exist in this board's project; the ids came from the client.
    let cards = state.issues.cards_by_id(ctx.tenant_id, board.project_id, ids).await?;
    if cards.len() != ids.len() {
        return Err(ApiError::invalid("issue_ids", "unknown_issue", "One or more issues were not found in this project."));
    }
    if cards.iter().any(|c| OFF_BOARD_TYPES.contains(&c.issue_type.as_str())) {
        return Err(ApiError::invalid(
            "issue_ids",
            "not_plannable",
            "Epics and sub-tasks cannot be ranked or planned into sprints.",
        ));
    }

    let mut tx = state.db.begin(ctx.tenant_id).await?;
    if let Some(Some(sprint_id)) = body.sprint_id {
        let target = store::sprint(tx.conn(), ctx.tenant_id, sprint_id, false).await?;
        match target {
            Some(s) if s.board_id == board.id && s.state != "closed" => {}
            Some(s) if s.board_id == board.id => {
                return Err(ApiError::invalid("sprint_id", "sprint_closed", "That sprint is already completed."))
            }
            _ => return Err(ApiError::invalid("sprint_id", "unknown_sprint", "That sprint does not belong to this board.")),
        }
    }
    store::lock_ranks(tx.conn(), board.project_id).await?;
    let (lo, hi) = match anchor {
        None => (store::neighbour_rank(tx.conn(), ctx.tenant_id, board.project_id, None, true, ids).await?, None),
        Some(anchor_id) => {
            let anchor_rank = store::placements_of(tx.conn(), ctx.tenant_id, board.project_id, &[anchor_id])
                .await?
                .pop()
                .map(|p| p.rank)
                .ok_or_else(|| {
                    ApiError::invalid(
                        "after_id",
                        "unknown_position",
                        "The issue to place next to was not found. Refresh and try again.",
                    )
                })?;
            let after = body.after_id.is_some();
            // `after`: between the anchor and whatever follows it; otherwise between
            // whatever precedes the anchor and the anchor.
            let other =
                store::neighbour_rank(tx.conn(), ctx.tenant_id, board.project_id, Some(&anchor_rank), !after, ids).await?;
            if after {
                (Some(anchor_rank), other)
            } else {
                (other, Some(anchor_rank))
            }
        }
    };
    let before: HashMap<Uuid, store::Placement> = store::placements_of(tx.conn(), ctx.tenant_id, board.project_id, ids)
        .await?
        .into_iter()
        .map(|p| (p.issue_id, p))
        .collect();
    let ranks = rank::spread(lo.as_deref(), hi.as_deref(), ids.len());
    for (issue_id, rank) in ids.iter().zip(&ranks) {
        store::place(tx.conn(), ctx.tenant_id, board.project_id, *issue_id, rank, body.sprint_id).await?;
    }
    crate::rebalance::queue_if_long(tx.conn(), ctx.tenant_id, board.project_id, &ranks).await?;
    if let Some(target) = body.sprint_id {
        // Scope changes feed the burndown chart.
        let mut left: HashMap<Uuid, Vec<Uuid>> = HashMap::new();
        let mut joined = Vec::new();
        for issue_id in ids {
            let previous = before.get(issue_id).filter(|p| p.in_open_sprint()).and_then(|p| p.sprint_id);
            if previous == target {
                continue;
            }
            if let Some(previous) = previous {
                left.entry(previous).or_default().push(*issue_id);
            }
            if target.is_some() {
                joined.push(*issue_id);
            }
        }
        for (sprint_id, issues) in &left {
            store::log_scope(tx.conn(), ctx.tenant_id, *sprint_id, issues, false, ctx.user_id).await?;
        }
        if let Some(sprint_id) = target {
            store::log_scope(tx.conn(), ctx.tenant_id, sprint_id, &joined, true, ctx.user_id).await?;
        }
    }
    let ranked = IssueRankedV1 {
        board_id: board.id,
        project_id: board.project_id,
        issue_ids: ids.clone(),
        sprint_id: body.sprint_id.flatten(),
    };
    emit(&mut tx, &ctx, &board, ranked).await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

// --- sprints -------------------------------------------------------------------

#[utoipa::path(get, path = "/api/v1/boards/{id}/sprints", security(("bearer" = [])), params(("id" = Uuid, Path)),
    responses((status = 200, body = SprintList), (status = 404)))]
pub async fn list_sprints(State(state): State<AppState>, ctx: OrgContext, Path(id): Path<Uuid>) -> ApiResult<Json<SprintList>> {
    open_board(&state, &ctx, id, ProjectPermission::Browse).await?;
    let sprints = board_sprints(&state, &ctx, id).await?;
    Ok(Json(SprintList { items: sprints.iter().map(SprintDto::from).collect() }))
}

#[utoipa::path(post, path = "/api/v1/boards/{id}/sprints", request_body = CreateSprintRequest, security(("bearer" = [])),
    params(("id" = Uuid, Path)),
    responses((status = 201, body = SprintDto), (status = 403), (status = 404), (status = 409), (status = 422)))]
pub async fn create_sprint(
    State(state): State<AppState>,
    ctx: OrgContext,
    Path(id): Path<Uuid>,
    Json(body): Json<CreateSprintRequest>,
) -> ApiResult<(StatusCode, HeaderMap, Json<SprintDto>)> {
    let (board, _) = open_board(&state, &ctx, id, ProjectPermission::ManageSprints).await?;
    if board.kind != "scrum" {
        return Err(ApiError::Conflict("Kanban boards do not have sprints.".into()));
    }
    let name = body.name.as_deref().map(|n| validate_name(n, "sprint")).transpose()?;
    let goal = validate_goal(&body.goal)?;
    validate_dates(body.start_date, body.end_date)?;
    let mut tx = state.db.begin(ctx.tenant_id).await?;
    // The sequence number is read and written in one statement; a concurrent create
    // loses on the unique index and is told to retry.
    let sprint = store::insert_sprint(
        tx.conn(),
        ctx.tenant_id,
        &board,
        name.as_deref(),
        &goal,
        body.start_date,
        body.end_date,
        ctx.user_id,
    )
    .await
    .map_err(duplicate("Another sprint was created at the same time. Try again."))?;
    let created =
        SprintCreatedV1 { sprint_id: sprint.id, board_id: board.id, project_id: board.project_id, name: sprint.name.clone() };
    emit(&mut tx, &ctx, &board, created).await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, etag(sprint.version), Json(SprintDto::from(&sprint))))
}

#[utoipa::path(get, path = "/api/v1/sprints/{id}", security(("bearer" = [])), params(("id" = Uuid, Path)),
    responses((status = 200, body = SprintDto), (status = 404)))]
pub async fn get_sprint(
    State(state): State<AppState>,
    ctx: OrgContext,
    Path(id): Path<Uuid>,
) -> ApiResult<(HeaderMap, Json<SprintDto>)> {
    let (sprint, _, _) = open_sprint(&state, &ctx, id, ProjectPermission::Browse).await?;
    Ok((etag(sprint.version), Json(SprintDto::from(&sprint))))
}

#[utoipa::path(patch, path = "/api/v1/sprints/{id}", request_body = UpdateSprintRequest, security(("bearer" = [])),
    params(("id" = Uuid, Path), ("If-Match" = String, Header)),
    responses((status = 200, body = SprintDto), (status = 403), (status = 404), (status = 409), (status = 422), (status = 428)))]
pub async fn update_sprint(
    State(state): State<AppState>,
    ctx: OrgContext,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Json(body): Json<UpdateSprintRequest>,
) -> ApiResult<(HeaderMap, Json<SprintDto>)> {
    let expected = require_if_match(&headers)?;
    open_sprint(&state, &ctx, id, ProjectPermission::ManageSprints).await?;
    let mut tx = state.db.begin(ctx.tenant_id).await?;
    let mut sprint = store::sprint(tx.conn(), ctx.tenant_id, id, true).await?.ok_or(SPRINT_NOT_FOUND)?;
    if sprint.version != expected {
        return Err(ApiError::VersionConflict { current: serde_json::to_value(SprintDto::from(&sprint)).ok() });
    }
    if sprint.state == "closed" && (body.start_date.is_some() || body.end_date.is_some()) {
        return Err(ApiError::Conflict("The dates of a completed sprint cannot be changed.".into()));
    }
    if let Some(name) = &body.name {
        sprint.name = validate_name(name, "sprint")?;
    }
    if let Some(goal) = &body.goal {
        sprint.goal = validate_goal(goal)?;
    }
    if let Some(start) = body.start_date {
        sprint.start_date = start;
    }
    if let Some(end) = body.end_date {
        sprint.end_date = end;
    }
    validate_dates(sprint.start_date, sprint.end_date)?;
    let saved = store::save_sprint(tx.conn(), ctx.tenant_id, &sprint).await?;
    tx.commit().await?;
    Ok((etag(saved.version), Json(SprintDto::from(&saved))))
}

/// Only sprints that have not started can be deleted; their issues return to the backlog.
#[utoipa::path(delete, path = "/api/v1/sprints/{id}", security(("bearer" = [])), params(("id" = Uuid, Path)),
    responses((status = 204), (status = 403), (status = 404), (status = 409)))]
pub async fn delete_sprint(State(state): State<AppState>, ctx: OrgContext, Path(id): Path<Uuid>) -> ApiResult<StatusCode> {
    open_sprint(&state, &ctx, id, ProjectPermission::ManageSprints).await?;
    let mut tx = state.db.begin(ctx.tenant_id).await?;
    let sprint = store::sprint(tx.conn(), ctx.tenant_id, id, true).await?.ok_or(SPRINT_NOT_FOUND)?;
    if sprint.state != "future" {
        return Err(ApiError::Conflict("Only sprints that have not started can be deleted.".into()));
    }
    store::delete_sprint(tx.conn(), ctx.tenant_id, id).await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Start a planned sprint. It must contain at least one issue, and its board must not
/// already be running a sprint. What it contains now is recorded as the commitment.
#[utoipa::path(post, path = "/api/v1/sprints/{id}/start", request_body = StartSprintRequest, security(("bearer" = [])),
    params(("id" = Uuid, Path)),
    responses((status = 200, body = SprintDto), (status = 403), (status = 404), (status = 409), (status = 422)))]
pub async fn start_sprint(
    State(state): State<AppState>,
    ctx: OrgContext,
    Path(id): Path<Uuid>,
    Json(body): Json<StartSprintRequest>,
) -> ApiResult<(HeaderMap, Json<SprintDto>)> {
    let (planned, board, _) = open_sprint(&state, &ctx, id, ProjectPermission::ManageSprints).await?;
    let name = body.name.as_deref().map(|n| validate_name(n, "sprint")).transpose()?;
    let goal = body.goal.as_deref().map(validate_goal).transpose()?;
    let now = OffsetDateTime::now_utc();
    let today = now.date();
    let end = body.end_date.or(planned.end_date.filter(|d| *d >= today)).unwrap_or(today + Duration::days(DEFAULT_SPRINT_DAYS));
    validate_dates(Some(today), Some(end))?;

    let (members, _, cards) = sprint_material(&state, &ctx, &planned).await?;
    let present: Vec<&Card> = members.iter().filter_map(|m| cards.get(m)).collect();
    if present.is_empty() {
        return Err(ApiError::invalid("sprint", "empty_sprint", "Add at least one issue before starting the sprint."));
    }
    let commitment = Commitment { issues: u32::try_from(present.len()).unwrap_or(u32::MAX), points: points(&present) };

    let mut tx = state.db.begin(ctx.tenant_id).await?;
    let mut sprint = store::sprint(tx.conn(), ctx.tenant_id, id, true).await?.ok_or(SPRINT_NOT_FOUND)?;
    if sprint.state != "future" {
        return Err(ApiError::Conflict("This sprint has already been started.".into()));
    }
    if let Some(name) = name {
        sprint.name = name;
    }
    if let Some(goal) = goal {
        sprint.goal = goal;
    }
    sprint.state = "active".into();
    sprint.start_date = Some(today);
    sprint.end_date = Some(end);
    sprint.started_at = Some(now);
    sprint.commitment = Some(serde_json::to_value(&commitment).map_err(ApiError::internal)?);
    let saved = store::save_sprint(tx.conn(), ctx.tenant_id, &sprint)
        .await
        .map_err(duplicate("Another sprint is already running on this board. Complete it first."))?;
    let started = SprintStartedV1 {
        sprint_id: id,
        board_id: board.id,
        project_id: board.project_id,
        name: saved.name.clone(),
        issue_count: commitment.issues,
    };
    emit(&mut tx, &ctx, &board, started).await?;
    tx.commit().await?;
    Ok((etag(saved.version), Json(SprintDto::from(&saved))))
}

/// Complete the running sprint. Finished issues stay with it; unfinished ones go to the
/// backlog or to a planned sprint. The outcome and burndown are frozen at this moment.
#[utoipa::path(post, path = "/api/v1/sprints/{id}/complete", request_body = CompleteSprintRequest, security(("bearer" = [])),
    params(("id" = Uuid, Path)),
    responses((status = 200, body = SprintDto), (status = 403), (status = 404), (status = 409), (status = 422)))]
pub async fn complete_sprint(
    State(state): State<AppState>,
    ctx: OrgContext,
    Path(id): Path<Uuid>,
    Json(body): Json<CompleteSprintRequest>,
) -> ApiResult<(HeaderMap, Json<SprintDto>)> {
    let (running, board, _) = open_sprint(&state, &ctx, id, ProjectPermission::ManageSprints).await?;
    let not_running = || ApiError::Conflict("Only a running sprint can be completed.".into());
    let started_at = running.started_at.filter(|_| running.state == "active").ok_or_else(not_running)?;
    if body.move_incomplete_to == Some(id) {
        return Err(ApiError::invalid("move_incomplete_to", "same_sprint", "Choose a different sprint."));
    }
    let (members, log, cards) = sprint_material(&state, &ctx, &running).await?;
    let now = OffsetDateTime::now_utc();
    let result = outcome(started_at, now, &members, &log, &cards);
    let incomplete: Vec<Uuid> = result.incomplete.iter().map(|i| i.id).collect();

    let mut tx = state.db.begin(ctx.tenant_id).await?;
    let mut sprint = store::sprint(tx.conn(), ctx.tenant_id, id, true).await?.ok_or(SPRINT_NOT_FOUND)?;
    if sprint.state != "active" {
        return Err(not_running());
    }
    // The outcome was computed outside this transaction; refuse to freeze a stale one.
    if store::sprint_issue_ids(tx.conn(), ctx.tenant_id, id).await? != members {
        return Err(ApiError::Conflict("The sprint changed while it was being completed. Try again.".into()));
    }
    if let Some(target_id) = body.move_incomplete_to {
        match store::sprint(tx.conn(), ctx.tenant_id, target_id, true).await? {
            Some(target) if target.board_id == board.id && target.state == "future" => {}
            _ => {
                return Err(ApiError::invalid(
                    "move_incomplete_to",
                    "unknown_sprint",
                    "Unfinished issues can only move to a planned sprint of this board.",
                ))
            }
        }
        store::log_scope(tx.conn(), ctx.tenant_id, target_id, &incomplete, true, ctx.user_id).await?;
    }
    store::set_sprint(tx.conn(), ctx.tenant_id, &incomplete, body.move_incomplete_to).await?;
    sprint.state = "closed".into();
    sprint.completed_at = Some(now);
    sprint.report = Some(serde_json::to_value(&result).map_err(ApiError::internal)?);
    let saved = store::save_sprint(tx.conn(), ctx.tenant_id, &sprint).await?;
    let completed = SprintCompletedV1 {
        sprint_id: id,
        board_id: board.id,
        project_id: board.project_id,
        name: saved.name.clone(),
        completed_issues: u32::try_from(result.completed.len()).unwrap_or(u32::MAX),
        incomplete_issues: u32::try_from(incomplete.len()).unwrap_or(u32::MAX),
        moved_to_sprint_id: body.move_incomplete_to,
    };
    emit(&mut tx, &ctx, &board, completed).await?;
    tx.commit().await?;
    Ok((etag(saved.version), Json(SprintDto::from(&saved))))
}

/// Burndown and outcome. Live for a running sprint, frozen for a completed one.
#[utoipa::path(get, path = "/api/v1/sprints/{id}/report", security(("bearer" = [])), params(("id" = Uuid, Path)),
    responses((status = 200, body = SprintReport), (status = 404), (status = 409, description = "The sprint has not started")))]
pub async fn sprint_report(
    State(state): State<AppState>,
    ctx: OrgContext,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<SprintReport>> {
    let (sprint, _, _) = open_sprint(&state, &ctx, id, ProjectPermission::Browse).await?;
    let result = match (sprint.state.as_str(), &sprint.report, sprint.started_at) {
        ("closed", Some(report), _) => serde_json::from_value(report.clone())
            .map_err(|e| ApiError::internal(anyhow::anyhow!("stored sprint report is invalid: {e}")))?,
        ("active", _, Some(started_at)) => {
            let (members, log, cards) = sprint_material(&state, &ctx, &sprint).await?;
            outcome(started_at, OffsetDateTime::now_utc(), &members, &log, &cards)
        }
        _ => return Err(ApiError::Conflict("This sprint has not started, so there is nothing to report yet.".into())),
    };
    Ok(Json(SprintReport { sprint: SprintDto::from(&sprint), outcome: result }))
}

/// Committed versus completed work for the board's most recent completed sprints.
#[utoipa::path(get, path = "/api/v1/boards/{id}/velocity", security(("bearer" = [])), params(("id" = Uuid, Path)),
    responses((status = 200, body = VelocityReport), (status = 404)))]
pub async fn velocity(State(state): State<AppState>, ctx: OrgContext, Path(id): Path<Uuid>) -> ApiResult<Json<VelocityReport>> {
    open_board(&state, &ctx, id, ProjectPermission::Browse).await?;
    let mut tx = state.db.begin(ctx.tenant_id).await?;
    let mut closed = store::closed_sprints(tx.conn(), ctx.tenant_id, id, 10).await?;
    tx.commit().await?;
    closed.reverse();
    let sprints = closed
        .iter()
        .map(|s| {
            let commitment: Option<Commitment> = s.commitment.as_ref().and_then(|c| serde_json::from_value(c.clone()).ok());
            let report: Option<SprintOutcome> = s.report.as_ref().and_then(|r| serde_json::from_value(r.clone()).ok());
            VelocityEntry {
                sprint_id: s.id,
                name: s.name.clone(),
                committed_issues: commitment.as_ref().map_or(0, |c| c.issues),
                committed_points: commitment.as_ref().map_or(0.0, |c| c.points),
                completed_issues: report.as_ref().map_or(0, |r| u32::try_from(r.completed.len()).unwrap_or(u32::MAX)),
                completed_points: report.as_ref().map_or(0.0, |r| r.completed_points),
            }
        })
        .collect();
    Ok(Json(VelocityReport { sprints }))
}

#[derive(OpenApi)]
#[openapi(
    info(title = "Agile API", version = "1.0.0", description = "Boards, backlog ranking, sprints and sprint reports."),
    paths(
        list_boards,
        create_board,
        get_board,
        update_board,
        delete_board,
        view,
        backlog,
        move_issues,
        list_sprints,
        create_sprint,
        get_sprint,
        update_sprint,
        delete_sprint,
        start_sprint,
        complete_sprint,
        sprint_report,
        velocity
    ),
    components(schemas(
        BoardPermissions,
        BoardDto,
        BoardList,
        CreateBoardRequest,
        UpdateBoardRequest,
        Column,
        Commitment,
        SprintDto,
        SprintList,
        CreateSprintRequest,
        UpdateSprintRequest,
        StartSprintRequest,
        CompleteSprintRequest,
        CardDto,
        EpicDto,
        ColumnView,
        BoardView,
        SprintGroup,
        BacklogView,
        MoveRequest,
        ReportIssue,
        SprintOutcome,
        SprintReport,
        BurndownSample,
        VelocityEntry,
        VelocityReport
    ))
)]
pub struct ApiDoc;

pub fn router(state: AppState) -> Router {
    let api = Router::new()
        .route("/boards", get(list_boards).post(create_board))
        .route("/boards/{id}", get(get_board).patch(update_board).delete(delete_board))
        .route("/boards/{id}/view", get(view))
        .route("/boards/{id}/backlog", get(backlog))
        .route("/boards/{id}/move", post(move_issues))
        .route("/boards/{id}/sprints", get(list_sprints).post(create_sprint))
        .route("/boards/{id}/velocity", get(velocity))
        .route("/sprints/{id}", get(get_sprint).patch(update_sprint).delete(delete_sprint))
        .route("/sprints/{id}/start", post(start_sprint))
        .route("/sprints/{id}/complete", post(complete_sprint))
        .route("/sprints/{id}/report", get(sprint_report));

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
