//! The local copy of issue cards that boards and backlogs read.
//!
//! Filled per project the first time one of its boards loads (every issue id of the project
//! from issue-service, a page at a time, then their cards), and kept current by the
//! `agile-service.cards` consumer: an issue event makes it fetch that issue's card again,
//! and its children's when the issue's key is someone's parent. Fetching the current card
//! rather than applying the event means events may arrive late, twice or out of order.
//!
//! A copy is only replaced by the same or a newer issue version, and a deleted issue stays
//! as a tombstone, so a backfill racing a deletion cannot bring the card back. Issues never
//! move between projects.

use std::{collections::HashSet, sync::Arc};

use async_trait::async_trait;
use error_handling::ApiResult;
use event_bus::{Event, Handler, HandlerError};
use serde::Deserialize;
use sqlx::PgConnection;
use tenant_context::DbRouter;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::sources::{Card, Issues, OFF_BOARD_TYPES};

pub const CONSUMER: &str = "agile-service.cards";
/// Issue ids per backfill page (issue-service's maximum).
const PAGE: u32 = 1000;
/// Most cards one board shows; beyond this the board says it is truncated.
pub const MAX_BOARD_CARDS: i64 = 5000;
/// Most children refreshed when their parent changes.
const MAX_CHILDREN: i64 = 500;

const COLUMNS: &str = "issue_id AS id, key, number, issue_type, status, status_category, priority, summary, assignee_id, \
                       story_points, resolved_at, parent_key, parent_summary, version, labels";

pub async fn upsert(conn: &mut PgConnection, tenant_id: Uuid, project_id: Uuid, c: &Card) -> ApiResult<()> {
    sqlx::query(
        "INSERT INTO cards (tenant_id, issue_id, project_id, key, number, issue_type, status, status_category, priority, \
                            summary, assignee_id, story_points, resolved_at, parent_key, parent_summary, labels, version) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17) \
         ON CONFLICT (tenant_id, issue_id) DO UPDATE SET \
             project_id = EXCLUDED.project_id, key = EXCLUDED.key, number = EXCLUDED.number, \
             issue_type = EXCLUDED.issue_type, status = EXCLUDED.status, status_category = EXCLUDED.status_category, \
             priority = EXCLUDED.priority, summary = EXCLUDED.summary, assignee_id = EXCLUDED.assignee_id, \
             story_points = EXCLUDED.story_points, resolved_at = EXCLUDED.resolved_at, parent_key = EXCLUDED.parent_key, \
             parent_summary = EXCLUDED.parent_summary, labels = EXCLUDED.labels, version = EXCLUDED.version, \
             updated_at = now() \
         WHERE NOT cards.deleted AND cards.version <= EXCLUDED.version",
    )
    .bind(tenant_id)
    .bind(c.id)
    .bind(project_id)
    .bind(&c.key)
    .bind(c.number)
    .bind(&c.issue_type)
    .bind(&c.status)
    .bind(&c.status_category)
    .bind(&c.priority)
    .bind(&c.summary)
    .bind(c.assignee_id)
    .bind(c.story_points)
    .bind(c.resolved_at)
    .bind(&c.parent_key)
    .bind(&c.parent_summary)
    .bind(&c.labels)
    .bind(c.version)
    .execute(conn)
    .await?;
    Ok(())
}

pub async fn tombstone(conn: &mut PgConnection, tenant_id: Uuid, project_id: Uuid, issue_id: Uuid) -> ApiResult<()> {
    sqlx::query(
        "INSERT INTO cards (tenant_id, issue_id, project_id, deleted) VALUES ($1, $2, $3, true) \
         ON CONFLICT (tenant_id, issue_id) DO UPDATE SET deleted = true, updated_at = now()",
    )
    .bind(tenant_id)
    .bind(issue_id)
    .bind(project_id)
    .execute(conn)
    .await?;
    Ok(())
}

/// Copy these issues' current cards; ones issue-service no longer has become tombstones.
pub async fn refresh(db: &DbRouter, issues: &dyn Issues, tenant_id: Uuid, project_id: Uuid, ids: &[Uuid]) -> ApiResult<()> {
    if ids.is_empty() {
        return Ok(());
    }
    let found = issues.cards_by_id(tenant_id, project_id, ids).await?;
    let present: HashSet<Uuid> = found.iter().map(|c| c.id).collect();
    let mut tx = db.begin(tenant_id).await?;
    for card in &found {
        upsert(tx.conn(), tenant_id, project_id, card).await?;
    }
    for id in ids.iter().filter(|id| !present.contains(id)) {
        tombstone(tx.conn(), tenant_id, project_id, *id).await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Copy every card of the project once; afterwards events keep it current. Two first loads
/// at once both copy, which is harmless.
pub async fn ensure_synced(db: &DbRouter, issues: &dyn Issues, tenant_id: Uuid, project_id: Uuid) -> ApiResult<()> {
    let mut tx = db.begin(tenant_id).await?;
    let synced: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM card_projects WHERE tenant_id = $1 AND project_id = $2)")
        .bind(tenant_id)
        .bind(project_id)
        .fetch_one(tx.conn())
        .await?;
    tx.commit().await?;
    if synced {
        return Ok(());
    }
    let mut after = None;
    let mut copied = 0usize;
    loop {
        let ids = issues.issue_ids(tenant_id, project_id, after, PAGE).await?;
        refresh(db, issues, tenant_id, project_id, &ids).await?;
        copied += ids.len();
        match ids.last() {
            Some(last) if ids.len() == PAGE as usize => after = Some(*last),
            _ => break,
        }
    }
    let mut tx = db.begin(tenant_id).await?;
    sqlx::query("INSERT INTO card_projects (tenant_id, project_id) VALUES ($1, $2) ON CONFLICT DO NOTHING")
        .bind(tenant_id)
        .bind(project_id)
        .execute(tx.conn())
        .await?;
    tx.commit().await?;
    metrics::counter!("agile_card_backfills_total").increment(1);
    tracing::info!(%project_id, copied, "copied a project's cards");
    Ok(())
}

/// The project's board cards: unfinished, or finished at or after `done_since`, in issue
/// number order; at most `limit`.
pub async fn open(
    conn: &mut PgConnection,
    tenant_id: Uuid,
    project_id: Uuid,
    done_since: OffsetDateTime,
    limit: i64,
) -> ApiResult<Vec<Card>> {
    let off_board: Vec<String> = OFF_BOARD_TYPES.iter().map(|t| (*t).to_owned()).collect();
    Ok(sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM cards WHERE tenant_id = $1 AND project_id = $2 AND NOT deleted \
         AND issue_type <> ALL($3) AND (status_category <> 'done' OR resolved_at >= $4) ORDER BY number LIMIT $5"
    ))
    .bind(tenant_id)
    .bind(project_id)
    .bind(&off_board)
    .bind(done_since)
    .bind(limit)
    .fetch_all(conn)
    .await?)
}

/// These issues' cards, any type or status, as far as they exist in the project.
pub async fn by_ids(conn: &mut PgConnection, tenant_id: Uuid, project_id: Uuid, ids: &[Uuid]) -> ApiResult<Vec<Card>> {
    Ok(sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM cards WHERE tenant_id = $1 AND project_id = $2 AND issue_id = ANY($3) AND NOT deleted"
    ))
    .bind(tenant_id)
    .bind(project_id)
    .bind(ids)
    .fetch_all(conn)
    .await?)
}

/// A deleted project's cards go with it.
pub async fn forget_project(conn: &mut PgConnection, tenant_id: Uuid, project_id: Uuid) -> ApiResult<()> {
    sqlx::query("DELETE FROM cards WHERE tenant_id = $1 AND project_id = $2")
        .bind(tenant_id)
        .bind(project_id)
        .execute(&mut *conn)
        .await?;
    sqlx::query("DELETE FROM card_projects WHERE tenant_id = $1 AND project_id = $2")
        .bind(tenant_id)
        .bind(project_id)
        .execute(conn)
        .await?;
    Ok(())
}

/// The issue an event is about.
#[derive(Debug, Deserialize)]
struct About {
    issue_id: Uuid,
    project_id: Uuid,
    key: String,
}

pub struct CardSync {
    pub db: Arc<DbRouter>,
    pub issues: Arc<dyn Issues>,
}

#[async_trait]
impl Handler for CardSync {
    async fn handle(&self, event: &Event) -> Result<(), HandlerError> {
        let deleted = match event.event_type() {
            "issue.created" | "issue.updated" | "issue.transitioned" | "issue.assigned" => false,
            "issue.deleted" => true,
            _ => return Ok(()),
        };
        let tenant_id = event.tenant_id()?;
        let about: About = event.payload()?;
        if deleted {
            let mut tx = self.db.begin(tenant_id).await?;
            tombstone(tx.conn(), tenant_id, about.project_id, about.issue_id).await?;
            tx.commit().await?;
        } else {
            refresh(&self.db, self.issues.as_ref(), tenant_id, about.project_id, &[about.issue_id]).await?;
        }
        // Children show the parent's key and summary (and lose them when it is deleted).
        let mut tx = self.db.begin(tenant_id).await?;
        let children: Vec<(Uuid, Uuid)> = sqlx::query_as(
            "SELECT project_id, issue_id FROM cards WHERE tenant_id = $1 AND parent_key = $2 AND NOT deleted \
             ORDER BY project_id LIMIT $3",
        )
        .bind(tenant_id)
        .bind(&about.key)
        .bind(MAX_CHILDREN)
        .fetch_all(tx.conn())
        .await?;
        tx.commit().await?;
        let mut projects: Vec<Uuid> = children.iter().map(|(p, _)| *p).collect();
        projects.dedup();
        for project in projects {
            let ids: Vec<Uuid> = children.iter().filter(|(p, _)| *p == project).map(|(_, id)| *id).collect();
            refresh(&self.db, self.issues.as_ref(), tenant_id, project, &ids).await?;
        }
        Ok(())
    }
}
