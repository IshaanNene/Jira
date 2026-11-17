//! Rewrites a project's ranks when they have grown long.
//!
//! Moving an issue between two neighbours takes their midpoint, so many moves into the same
//! gap make ranks longer one character at a time. When a move produces a rank longer than
//! [`LONG_RANK`], the project is queued (`rank_rebalance`); a per-database runner claims queued
//! projects with a lease and, under the same per-project lock as moves, gives every placement
//! a fresh evenly spaced rank in the same order. The order never changes, so nothing is
//! announced; a move that waited for the lock reads the new neighbours.

use std::time::Duration;

use error_handling::ApiResult;
use sqlx::{PgConnection, PgPool};
use tenant_context::DbRouter;
use uuid::Uuid;

use crate::{rank, store};

/// Ranks longer than this queue their project for rebalancing.
pub const LONG_RANK: usize = 12;
/// Projects claimed per pass.
const BATCH: i64 = 10;
/// How long a claimed project is left to one runner.
const LEASE_SECS: f64 = 300.0;
const IDLE: Duration = Duration::from_secs(30);

/// Queue the project if any of these new ranks is long. Called inside the tenant's transaction.
pub async fn queue_if_long(conn: &mut PgConnection, tenant_id: Uuid, project_id: Uuid, ranks: &[String]) -> ApiResult<()> {
    if ranks.iter().all(|r| r.len() <= LONG_RANK) {
        return Ok(());
    }
    sqlx::query("INSERT INTO rank_rebalance (tenant_id, project_id) VALUES ($1, $2) ON CONFLICT DO NOTHING")
        .bind(tenant_id)
        .bind(project_id)
        .execute(conn)
        .await?;
    Ok(())
}

/// Give every placement of the project a fresh rank, keeping the order; returns how many.
pub async fn rebalance(db: &DbRouter, tenant_id: Uuid, project_id: Uuid) -> ApiResult<usize> {
    let mut tx = db.begin(tenant_id).await?;
    store::lock_ranks(tx.conn(), project_id).await?;
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT issue_id FROM issue_placements WHERE tenant_id = $1 AND project_id = $2 ORDER BY rank, issue_id",
    )
    .bind(tenant_id)
    .bind(project_id)
    .fetch_all(tx.conn())
    .await?;
    let ranks = rank::spread(None, None, ids.len());
    sqlx::query(
        "UPDATE issue_placements p SET rank = v.rank FROM unnest($3::uuid[], $4::text[]) AS v(issue_id, rank) \
         WHERE p.tenant_id = $1 AND p.project_id = $2 AND p.issue_id = v.issue_id",
    )
    .bind(tenant_id)
    .bind(project_id)
    .bind(&ids)
    .bind(&ranks)
    .execute(tx.conn())
    .await?;
    sqlx::query("DELETE FROM rank_rebalance WHERE tenant_id = $1 AND project_id = $2")
        .bind(tenant_id)
        .bind(project_id)
        .execute(tx.conn())
        .await?;
    tx.commit().await?;
    metrics::counter!("agile_rank_rebalances_total").increment(1);
    tracing::info!(%project_id, placements = ids.len(), "rebalanced ranks");
    Ok(ids.len())
}

/// Claim queued projects of this database and rebalance them; returns how many were claimed.
pub async fn run_due(db: &DbRouter, pool: &PgPool) -> ApiResult<usize> {
    let due: Vec<(Uuid, Uuid)> = sqlx::query_as(
        "UPDATE rank_rebalance SET leased_until = now() + make_interval(secs => $2) \
         WHERE (tenant_id, project_id) IN (SELECT tenant_id, project_id FROM rank_rebalance \
             WHERE leased_until IS NULL OR leased_until < now() ORDER BY queued_at LIMIT $1 FOR UPDATE SKIP LOCKED) \
         RETURNING tenant_id, project_id",
    )
    .bind(BATCH)
    .bind(LEASE_SECS)
    .fetch_all(pool)
    .await?;
    for (tenant_id, project_id) in &due {
        if let Err(error) = rebalance(db, *tenant_id, *project_id).await {
            // The lease runs out and another pass tries again.
            tracing::warn!(%project_id, ?error, "rank rebalance failed");
        }
    }
    Ok(due.len())
}

pub fn spawn(db: std::sync::Arc<DbRouter>) {
    for pool in db.pools() {
        let db = std::sync::Arc::clone(&db);
        tokio::spawn(async move {
            loop {
                match run_due(&db, &pool).await {
                    Ok(n) if n as i64 == BATCH => continue,
                    Ok(_) => tokio::time::sleep(IDLE).await,
                    Err(error) => {
                        tracing::warn!(?error, "rank rebalance pass failed");
                        tokio::time::sleep(IDLE).await;
                    }
                }
            }
        });
    }
}
