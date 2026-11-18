//! SQL for boards, sprints and issue placements. Always inside a tenant-scoped transaction.

use error_handling::ApiResult;
use serde_json::Value;
use sqlx::{FromRow, PgConnection};
use time::{Date, OffsetDateTime};
use uuid::Uuid;

// --- boards ------------------------------------------------------------------

#[derive(Debug, Clone, FromRow)]
pub struct Board {
    pub id: Uuid,
    pub project_id: Uuid,
    pub project_key: String,
    pub name: String,
    pub kind: String,
    pub columns: Value,
    pub quick_filters: Value,
    pub version: i64,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

const BOARD: &str = "id, project_id, project_key, name, kind, columns, quick_filters, version, created_at, updated_at";

pub async fn boards(conn: &mut PgConnection, tenant_id: Uuid, project_id: Uuid) -> ApiResult<Vec<Board>> {
    Ok(sqlx::query_as(&format!(
        "SELECT {BOARD} FROM boards WHERE tenant_id = $1 AND project_id = $2 AND deleted_at IS NULL ORDER BY created_at LIMIT 100"
    ))
    .bind(tenant_id)
    .bind(project_id)
    .fetch_all(conn)
    .await?)
}

pub async fn board(conn: &mut PgConnection, tenant_id: Uuid, id: Uuid) -> ApiResult<Option<Board>> {
    Ok(sqlx::query_as(&format!("SELECT {BOARD} FROM boards WHERE tenant_id = $1 AND id = $2 AND deleted_at IS NULL"))
        .bind(tenant_id)
        .bind(id)
        .fetch_optional(conn)
        .await?)
}

#[allow(clippy::too_many_arguments)]
pub async fn insert_board(
    conn: &mut PgConnection,
    tenant_id: Uuid,
    id: Uuid,
    project_id: Uuid,
    project_key: &str,
    name: &str,
    kind: &str,
    columns: &Value,
    by: Uuid,
) -> Result<Board, sqlx::Error> {
    sqlx::query_as(&format!(
        "INSERT INTO boards (id, tenant_id, project_id, project_key, name, kind, columns, created_by) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8) RETURNING {BOARD}"
    ))
    .bind(id)
    .bind(tenant_id)
    .bind(project_id)
    .bind(project_key)
    .bind(name)
    .bind(kind)
    .bind(columns)
    .bind(by)
    .fetch_one(conn)
    .await
}

/// `None` when the row is no longer at `expected` version.
pub async fn update_board(
    conn: &mut PgConnection,
    tenant_id: Uuid,
    id: Uuid,
    expected: i64,
    name: &str,
    columns: &Value,
    quick_filters: &Value,
) -> Result<Option<Board>, sqlx::Error> {
    sqlx::query_as(&format!(
        "UPDATE boards SET name = $4, columns = $5, quick_filters = $6, version = version + 1, updated_at = now() \
         WHERE tenant_id = $1 AND id = $2 AND version = $3 AND deleted_at IS NULL RETURNING {BOARD}"
    ))
    .bind(tenant_id)
    .bind(id)
    .bind(expected)
    .bind(name)
    .bind(columns)
    .bind(quick_filters)
    .fetch_optional(conn)
    .await
}

/// Hide the board, drop its unstarted sprints (their issues return to the backlog) and
/// keep closed sprints for history.
pub async fn delete_board(conn: &mut PgConnection, tenant_id: Uuid, id: Uuid) -> ApiResult<()> {
    sqlx::query("DELETE FROM sprints WHERE tenant_id = $1 AND board_id = $2 AND state = 'future'")
        .bind(tenant_id)
        .bind(id)
        .execute(&mut *conn)
        .await?;
    sqlx::query("UPDATE boards SET deleted_at = now() WHERE tenant_id = $1 AND id = $2")
        .bind(tenant_id)
        .bind(id)
        .execute(conn)
        .await?;
    Ok(())
}

// --- sprints -----------------------------------------------------------------

#[derive(Debug, Clone, FromRow)]
pub struct Sprint {
    pub id: Uuid,
    pub board_id: Uuid,
    pub project_id: Uuid,
    pub sequence: i32,
    pub name: String,
    pub goal: String,
    pub state: String,
    pub start_date: Option<Date>,
    pub end_date: Option<Date>,
    pub started_at: Option<OffsetDateTime>,
    pub completed_at: Option<OffsetDateTime>,
    pub commitment: Option<Value>,
    pub report: Option<Value>,
    pub version: i64,
}

const SPRINT: &str = "id, board_id, project_id, sequence, name, goal, state, start_date, end_date, started_at, completed_at, \
     commitment, report, version";

/// Active first, then planned ones in creation order, then closed ones newest first.
pub async fn sprints(conn: &mut PgConnection, tenant_id: Uuid, board_id: Uuid) -> ApiResult<Vec<Sprint>> {
    Ok(sqlx::query_as(&format!(
        "SELECT {SPRINT} FROM sprints WHERE tenant_id = $1 AND board_id = $2 \
         ORDER BY CASE state WHEN 'active' THEN 0 WHEN 'future' THEN 1 ELSE 2 END, \
                  CASE WHEN state = 'closed' THEN -sequence ELSE sequence END LIMIT 500"
    ))
    .bind(tenant_id)
    .bind(board_id)
    .fetch_all(conn)
    .await?)
}

pub async fn sprint(conn: &mut PgConnection, tenant_id: Uuid, id: Uuid, lock: bool) -> ApiResult<Option<Sprint>> {
    let suffix = if lock { " FOR UPDATE" } else { "" };
    Ok(sqlx::query_as(&format!("SELECT {SPRINT} FROM sprints WHERE tenant_id = $1 AND id = $2{suffix}"))
        .bind(tenant_id)
        .bind(id)
        .fetch_optional(conn)
        .await?)
}

/// Appends a sprint to the board. `name = None` uses "<project key> Sprint <n>".
#[allow(clippy::too_many_arguments)]
pub async fn insert_sprint(
    conn: &mut PgConnection,
    tenant_id: Uuid,
    board: &Board,
    name: Option<&str>,
    goal: &str,
    start_date: Option<Date>,
    end_date: Option<Date>,
    by: Uuid,
) -> Result<Sprint, sqlx::Error> {
    sqlx::query_as(&format!(
        "INSERT INTO sprints (id, tenant_id, board_id, project_id, sequence, name, goal, start_date, end_date, created_by) \
         SELECT $1, $2, $3, $4, n, COALESCE($5, $6 || ' Sprint ' || n), $7, $8, $9, $10 \
         FROM (SELECT COALESCE(max(sequence), 0) + 1 AS n FROM sprints WHERE tenant_id = $2 AND board_id = $3) next \
         RETURNING {SPRINT}"
    ))
    .bind(platform_common::new_id())
    .bind(tenant_id)
    .bind(board.id)
    .bind(board.project_id)
    .bind(name)
    .bind(&board.project_key)
    .bind(goal)
    .bind(start_date)
    .bind(end_date)
    .bind(by)
    .fetch_one(conn)
    .await
}

/// Writes every mutable column of a sprint the caller holds a row lock on.
pub async fn save_sprint(conn: &mut PgConnection, tenant_id: Uuid, s: &Sprint) -> Result<Sprint, sqlx::Error> {
    sqlx::query_as(&format!(
        "UPDATE sprints SET name = $3, goal = $4, state = $5, start_date = $6, end_date = $7, started_at = $8, \
                completed_at = $9, commitment = $10, report = $11, version = version + 1, updated_at = now() \
         WHERE tenant_id = $1 AND id = $2 RETURNING {SPRINT}"
    ))
    .bind(tenant_id)
    .bind(s.id)
    .bind(&s.name)
    .bind(&s.goal)
    .bind(&s.state)
    .bind(s.start_date)
    .bind(s.end_date)
    .bind(s.started_at)
    .bind(s.completed_at)
    .bind(&s.commitment)
    .bind(&s.report)
    .fetch_one(conn)
    .await
}

pub async fn delete_sprint(conn: &mut PgConnection, tenant_id: Uuid, id: Uuid) -> ApiResult<()> {
    sqlx::query("DELETE FROM sprints WHERE tenant_id = $1 AND id = $2").bind(tenant_id).bind(id).execute(conn).await?;
    Ok(())
}

/// Closed sprints of a board, most recently completed first.
pub async fn closed_sprints(conn: &mut PgConnection, tenant_id: Uuid, board_id: Uuid, limit: i64) -> ApiResult<Vec<Sprint>> {
    Ok(sqlx::query_as(&format!(
        "SELECT {SPRINT} FROM sprints WHERE tenant_id = $1 AND board_id = $2 AND state = 'closed' \
         ORDER BY completed_at DESC LIMIT $3"
    ))
    .bind(tenant_id)
    .bind(board_id)
    .bind(limit)
    .fetch_all(conn)
    .await?)
}

// --- placements ----------------------------------------------------------------

#[derive(Debug, Clone, FromRow)]
pub struct Placement {
    pub issue_id: Uuid,
    pub rank: String,
    pub sprint_id: Option<Uuid>,
    /// State of that sprint, when the issue is in one.
    pub sprint_state: Option<String>,
}

impl Placement {
    /// In a sprint that is planned or running.
    pub fn in_open_sprint(&self) -> bool {
        matches!(self.sprint_state.as_deref(), Some("future" | "active"))
    }
}

const PLACEMENT: &str = "p.issue_id, p.rank, p.sprint_id, s.state AS sprint_state \
     FROM issue_placements p LEFT JOIN sprints s ON s.id = p.sprint_id";

/// Every placed issue of a project in board order.
pub async fn placements(conn: &mut PgConnection, tenant_id: Uuid, project_id: Uuid) -> ApiResult<Vec<Placement>> {
    Ok(sqlx::query_as(&format!("SELECT {PLACEMENT} WHERE p.tenant_id = $1 AND p.project_id = $2 ORDER BY p.rank, p.issue_id"))
        .bind(tenant_id)
        .bind(project_id)
        .fetch_all(conn)
        .await?)
}

pub async fn placements_of(
    conn: &mut PgConnection,
    tenant_id: Uuid,
    project_id: Uuid,
    issue_ids: &[Uuid],
) -> ApiResult<Vec<Placement>> {
    Ok(sqlx::query_as(&format!("SELECT {PLACEMENT} WHERE p.tenant_id = $1 AND p.project_id = $2 AND p.issue_id = ANY($3)"))
        .bind(tenant_id)
        .bind(project_id)
        .bind(issue_ids)
        .fetch_all(conn)
        .await?)
}

/// Issues planned into a sprint, in board order.
pub async fn sprint_issue_ids(conn: &mut PgConnection, tenant_id: Uuid, sprint_id: Uuid) -> ApiResult<Vec<Uuid>> {
    let rows: Vec<(Uuid,)> =
        sqlx::query_as("SELECT issue_id FROM issue_placements WHERE tenant_id = $1 AND sprint_id = $2 ORDER BY rank, issue_id")
            .bind(tenant_id)
            .bind(sprint_id)
            .fetch_all(conn)
            .await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

/// Serialises rank changes within one project for the rest of the transaction, so two
/// concurrent moves can never compute the same rank from the same neighbours.
pub async fn lock_ranks(conn: &mut PgConnection, project_id: Uuid) -> ApiResult<()> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1::text, 0))").bind(project_id).execute(conn).await?;
    Ok(())
}

/// The neighbouring rank above (`above = true`: the greatest rank below `rank`) or below
/// it, ignoring the issues being moved.
pub async fn neighbour_rank(
    conn: &mut PgConnection,
    tenant_id: Uuid,
    project_id: Uuid,
    rank: Option<&str>,
    above: bool,
    moving: &[Uuid],
) -> ApiResult<Option<String>> {
    let sql = if above {
        "SELECT max(rank) FROM issue_placements WHERE tenant_id = $1 AND project_id = $2 AND ($3::text IS NULL OR rank < $3) AND NOT (issue_id = ANY($4))"
    } else {
        "SELECT min(rank) FROM issue_placements WHERE tenant_id = $1 AND project_id = $2 AND ($3::text IS NULL OR rank > $3) AND NOT (issue_id = ANY($4))"
    };
    let (found,): (Option<String>,) =
        sqlx::query_as(sql).bind(tenant_id).bind(project_id).bind(rank).bind(moving).fetch_one(conn).await?;
    Ok(found)
}

/// Give an issue its first rank; an existing placement is left alone.
pub async fn place_if_absent(
    conn: &mut PgConnection,
    tenant_id: Uuid,
    project_id: Uuid,
    issue_id: Uuid,
    rank: &str,
) -> ApiResult<()> {
    sqlx::query(
        "INSERT INTO issue_placements (tenant_id, issue_id, project_id, rank) VALUES ($1, $2, $3, $4) \
         ON CONFLICT (tenant_id, issue_id) DO NOTHING",
    )
    .bind(tenant_id)
    .bind(issue_id)
    .bind(project_id)
    .bind(rank)
    .execute(conn)
    .await?;
    Ok(())
}

/// Set an issue's rank and, when `sprint` is given, its sprint (`Some(None)` = backlog).
pub async fn place(
    conn: &mut PgConnection,
    tenant_id: Uuid,
    project_id: Uuid,
    issue_id: Uuid,
    rank: &str,
    sprint: Option<Option<Uuid>>,
) -> ApiResult<()> {
    sqlx::query(
        "INSERT INTO issue_placements (tenant_id, issue_id, project_id, rank, sprint_id) VALUES ($1, $2, $3, $4, $6) \
         ON CONFLICT (tenant_id, issue_id) DO UPDATE SET rank = EXCLUDED.rank, updated_at = now(), \
             sprint_id = CASE WHEN $5 THEN EXCLUDED.sprint_id ELSE issue_placements.sprint_id END",
    )
    .bind(tenant_id)
    .bind(issue_id)
    .bind(project_id)
    .bind(rank)
    .bind(sprint.is_some())
    .bind(sprint.flatten())
    .execute(conn)
    .await?;
    Ok(())
}

/// Move issues to another sprint (or the backlog) without changing their order.
pub async fn set_sprint(conn: &mut PgConnection, tenant_id: Uuid, issue_ids: &[Uuid], sprint_id: Option<Uuid>) -> ApiResult<()> {
    sqlx::query("UPDATE issue_placements SET sprint_id = $3, updated_at = now() WHERE tenant_id = $1 AND issue_id = ANY($2)")
        .bind(tenant_id)
        .bind(issue_ids)
        .bind(sprint_id)
        .execute(conn)
        .await?;
    Ok(())
}

// --- scope log -----------------------------------------------------------------

pub async fn log_scope(
    conn: &mut PgConnection,
    tenant_id: Uuid,
    sprint_id: Uuid,
    issue_ids: &[Uuid],
    added: bool,
    actor: Uuid,
) -> ApiResult<()> {
    for issue_id in issue_ids {
        sqlx::query(
            "INSERT INTO sprint_scope_events (id, tenant_id, sprint_id, issue_id, action, actor_id) VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(platform_common::new_id())
        .bind(tenant_id)
        .bind(sprint_id)
        .bind(issue_id)
        .bind(if added { "added" } else { "removed" })
        .bind(actor)
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

/// (issue, added?, at) in the order they happened.
pub async fn scope_log(
    conn: &mut PgConnection,
    tenant_id: Uuid,
    sprint_id: Uuid,
) -> ApiResult<Vec<(Uuid, bool, OffsetDateTime)>> {
    Ok(sqlx::query_as(
        "SELECT issue_id, action = 'added', at FROM sprint_scope_events WHERE tenant_id = $1 AND sprint_id = $2 ORDER BY at, id",
    )
    .bind(tenant_id)
    .bind(sprint_id)
    .fetch_all(conn)
    .await?)
}

// --- cleanup after deletions elsewhere -----------------------------------------------

/// Remove an issue's placement. Returns the sprint it was in when that sprint is running.
pub async fn remove_placement(conn: &mut PgConnection, tenant_id: Uuid, issue_id: Uuid) -> ApiResult<Option<Uuid>> {
    let sprint: Option<Option<Uuid>> =
        sqlx::query_scalar("DELETE FROM issue_placements WHERE tenant_id = $1 AND issue_id = $2 RETURNING sprint_id")
            .bind(tenant_id)
            .bind(issue_id)
            .fetch_optional(&mut *conn)
            .await?;
    let Some(Some(sprint)) = sprint else { return Ok(None) };
    let active: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM sprints WHERE tenant_id = $1 AND id = $2 AND state = 'active')")
            .bind(tenant_id)
            .bind(sprint)
            .fetch_one(&mut *conn)
            .await?;
    Ok(active.then_some(sprint))
}

/// A project was deleted: its boards go, and so does its backlog order.
pub async fn retire_project(conn: &mut PgConnection, tenant_id: Uuid, project_id: Uuid) -> ApiResult<()> {
    sqlx::query("UPDATE boards SET deleted_at = now(), updated_at = now() WHERE tenant_id = $1 AND project_id = $2 AND deleted_at IS NULL")
        .bind(tenant_id)
        .bind(project_id)
        .execute(&mut *conn)
        .await?;
    sqlx::query("DELETE FROM issue_placements WHERE tenant_id = $1 AND project_id = $2")
        .bind(tenant_id)
        .bind(project_id)
        .execute(&mut *conn)
        .await?;
    Ok(())
}
