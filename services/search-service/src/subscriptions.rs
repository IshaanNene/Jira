//! Subscriptions to saved filters, and the runner that turns them into digests.
//!
//! `/api/v1/search/filters/{id}/subscription` is the caller's own subscription to a filter they
//! can see. When it is due, the filter runs as the subscriber, with their role and project
//! access at that moment (checked again with user-service and the permission service), and
//! a `filter.digest` event leaves through the outbox for notification-service to email. A
//! subscriber who left the workspace, or a filter they can no longer see, ends the
//! subscription instead.

use std::time::Duration as StdDuration;

use auth_middleware::OrgContext;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use error_handling::{ApiError, ApiResult};
use event_contracts::{
    outbox,
    payloads::{DigestIssueV1, FilterDigestV1, MAX_DIGEST_ISSUES},
    Actor, EventEnvelope,
};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgConnection, PgPool};
use time::{Duration, OffsetDateTime, Time};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{
    http::{compile, visible},
    index::Filters,
    AppState, SERVICE_NAME,
};

/// Subscriptions one person can hold in a workspace.
const MAX_PER_PERSON: i64 = 20;
/// Due subscriptions claimed per pass.
const BATCH: i64 = 20;
/// How long a claimed subscription is held; a run that fails is retried after this.
const LEASE_SECONDS: f64 = 300.0;
const IDLE: StdDuration = StdDuration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Frequency {
    Daily,
    Weekly,
}

impl Frequency {
    fn as_str(self) -> &'static str {
        match self {
            Frequency::Daily => "daily",
            Frequency::Weekly => "weekly",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "daily" => Some(Frequency::Daily),
            "weekly" => Some(Frequency::Weekly),
            _ => None,
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PutSubscription {
    pub frequency: Frequency,
    /// UTC hour, 0 to 23.
    pub hour: u8,
    /// ISO weekday (1 = Monday) for weekly subscriptions; must be absent for daily ones.
    pub weekday: Option<u8>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct Subscription {
    pub filter_id: Uuid,
    pub frequency: Frequency,
    pub hour: u8,
    pub weekday: Option<u8>,
    #[serde(with = "time::serde::rfc3339")]
    pub next_run_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_sent_at: Option<OffsetDateTime>,
}

/// The first run strictly after `now`: at `hour`:00 UTC, every day or on `weekday`.
pub fn next_run(now: OffsetDateTime, frequency: Frequency, hour: u8, weekday: Option<u8>) -> OffsetDateTime {
    let at = Time::from_hms(hour.min(23), 0, 0).expect("a valid hour");
    let mut run = now.date().with_time(at).assume_utc();
    match (frequency, weekday) {
        (Frequency::Weekly, Some(day)) => {
            let today = now.weekday().number_from_monday();
            run += Duration::days(i64::from((day + 7 - today) % 7));
            if run <= now {
                run += Duration::weeks(1);
            }
        }
        _ => {
            if run <= now {
                run += Duration::days(1);
            }
        }
    }
    run
}

fn validate(body: &PutSubscription) -> ApiResult<()> {
    if body.hour > 23 {
        return Err(ApiError::invalid("hour", "out_of_range", "Choose an hour from 0 to 23 (UTC)."));
    }
    match (body.frequency, body.weekday) {
        (Frequency::Weekly, Some(1..=7)) | (Frequency::Daily, None) => Ok(()),
        (Frequency::Weekly, _) => Err(ApiError::invalid("weekday", "required", "Choose a day of the week (1 = Monday).")),
        (Frequency::Daily, Some(_)) => Err(ApiError::invalid("weekday", "not_allowed", "A daily subscription runs every day.")),
    }
}

#[derive(FromRow)]
struct Row {
    frequency: String,
    hour: i16,
    weekday: Option<i16>,
    last_sent_at: Option<OffsetDateTime>,
    next_run_at: OffsetDateTime,
}

fn dto(filter_id: Uuid, r: Row) -> Subscription {
    Subscription {
        filter_id,
        frequency: Frequency::parse(&r.frequency).unwrap_or(Frequency::Daily),
        hour: u8::try_from(r.hour).unwrap_or_default(),
        weekday: r.weekday.and_then(|d| u8::try_from(d).ok()),
        next_run_at: r.next_run_at,
        last_sent_at: r.last_sent_at,
    }
}

/// The filter, if the caller can see it (their own, or shared).
async fn visible_filter(conn: &mut PgConnection, ctx: &OrgContext, id: Uuid) -> ApiResult<()> {
    let found: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM saved_filters WHERE tenant_id = $1 AND id = $2 AND (shared OR owner_id = $3)")
            .bind(ctx.tenant_id)
            .bind(id)
            .bind(ctx.user_id)
            .fetch_optional(conn)
            .await?;
    found.map(|_| ()).ok_or(ApiError::NotFound("filter"))
}

async fn current(conn: &mut PgConnection, ctx: &OrgContext, filter_id: Uuid) -> ApiResult<Option<Row>> {
    Ok(sqlx::query_as(
        "SELECT s.frequency, s.hour, s.weekday, s.last_sent_at, q.next_run_at \
         FROM filter_subscriptions s JOIN subscription_schedule q ON q.subscription_id = s.id \
         WHERE s.tenant_id = $1 AND s.filter_id = $2 AND s.user_id = $3",
    )
    .bind(ctx.tenant_id)
    .bind(filter_id)
    .bind(ctx.user_id)
    .fetch_optional(conn)
    .await?)
}

#[utoipa::path(get, path = "/api/v1/search/filters/{id}/subscription", security(("bearer" = [])), params(("id" = Uuid, Path)),
    responses((status = 200, body = Subscription), (status = 404, description = "No such filter, or not subscribed")))]
pub async fn get_subscription(
    State(state): State<AppState>,
    ctx: OrgContext,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Subscription>> {
    let mut tx = state.db.begin(ctx.tenant_id).await?;
    visible_filter(tx.conn(), &ctx, id).await?;
    let row = current(tx.conn(), &ctx, id).await?.ok_or(ApiError::NotFound("subscription"))?;
    tx.commit().await?;
    Ok(Json(dto(id, row)))
}

#[utoipa::path(put, path = "/api/v1/search/filters/{id}/subscription", request_body = PutSubscription, security(("bearer" = [])),
    params(("id" = Uuid, Path)),
    responses((status = 200, body = Subscription), (status = 404), (status = 409, description = "Too many subscriptions"), (status = 422)))]
pub async fn put_subscription(
    State(state): State<AppState>,
    ctx: OrgContext,
    Path(id): Path<Uuid>,
    Json(body): Json<PutSubscription>,
) -> ApiResult<Json<Subscription>> {
    validate(&body)?;
    let mut tx = state.db.begin(ctx.tenant_id).await?;
    visible_filter(tx.conn(), &ctx, id).await?;
    // Serialize one person's subscriptions so the limit cannot be raced past.
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext('filter_subscriptions:' || $1::text || ':' || $2::text))")
        .bind(ctx.tenant_id)
        .bind(ctx.user_id)
        .execute(tx.conn())
        .await?;
    if current(tx.conn(), &ctx, id).await?.is_none() {
        let (held,): (i64,) = sqlx::query_as("SELECT count(*) FROM filter_subscriptions WHERE tenant_id = $1 AND user_id = $2")
            .bind(ctx.tenant_id)
            .bind(ctx.user_id)
            .fetch_one(tx.conn())
            .await?;
        if held >= MAX_PER_PERSON {
            return Err(ApiError::Conflict(format!("You have {MAX_PER_PERSON} filter subscriptions. Remove one first.")));
        }
    }
    let (sub_id,): (Uuid,) = sqlx::query_as(
        "INSERT INTO filter_subscriptions (tenant_id, id, filter_id, user_id, frequency, hour, weekday) \
         VALUES ($1, $2, $3, $4, $5, $6, $7) \
         ON CONFLICT (tenant_id, filter_id, user_id) \
         DO UPDATE SET frequency = EXCLUDED.frequency, hour = EXCLUDED.hour, weekday = EXCLUDED.weekday \
         RETURNING id",
    )
    .bind(ctx.tenant_id)
    .bind(platform_common::new_id())
    .bind(id)
    .bind(ctx.user_id)
    .bind(body.frequency.as_str())
    .bind(i16::from(body.hour))
    .bind(body.weekday.map(i16::from))
    .fetch_one(tx.conn())
    .await?;
    let next = next_run(OffsetDateTime::now_utc(), body.frequency, body.hour, body.weekday);
    sqlx::query(
        "INSERT INTO subscription_schedule (subscription_id, tenant_id, next_run_at) VALUES ($1, $2, $3) \
         ON CONFLICT (subscription_id) DO UPDATE SET next_run_at = EXCLUDED.next_run_at, leased_until = NULL",
    )
    .bind(sub_id)
    .bind(ctx.tenant_id)
    .bind(next)
    .execute(tx.conn())
    .await?;
    let row = current(tx.conn(), &ctx, id).await?.ok_or(ApiError::NotFound("subscription"))?;
    tx.commit().await?;
    Ok(Json(dto(id, row)))
}

#[utoipa::path(delete, path = "/api/v1/search/filters/{id}/subscription", security(("bearer" = [])), params(("id" = Uuid, Path)),
    responses((status = 204), (status = 404)))]
pub async fn delete_subscription(State(state): State<AppState>, ctx: OrgContext, Path(id): Path<Uuid>) -> ApiResult<StatusCode> {
    let mut tx = state.db.begin(ctx.tenant_id).await?;
    let gone = sqlx::query("DELETE FROM filter_subscriptions WHERE tenant_id = $1 AND filter_id = $2 AND user_id = $3")
        .bind(ctx.tenant_id)
        .bind(id)
        .bind(ctx.user_id)
        .execute(tx.conn())
        .await?
        .rows_affected();
    tx.commit().await?;
    if gone == 0 {
        return Err(ApiError::NotFound("subscription"));
    }
    Ok(StatusCode::NO_CONTENT)
}

#[derive(FromRow)]
struct Due {
    user_id: Uuid,
    frequency: String,
    hour: i16,
    weekday: Option<i16>,
    filter_id: Uuid,
    name: String,
    query: String,
    shared: bool,
    owner_id: Uuid,
}

/// What one run did.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// A digest was queued for sending.
    Sent,
    /// The subscriber can no longer receive it; the subscription was removed.
    Ended,
    /// The filter no longer runs for this person (e.g. a person it names left); tried again
    /// at the next run.
    Skipped,
    /// The subscription was removed in the meantime.
    Gone,
}

async fn reschedule(conn: &mut PgConnection, subscription_id: Uuid, next: OffsetDateTime) -> sqlx::Result<()> {
    sqlx::query("UPDATE subscription_schedule SET next_run_at = $2, leased_until = NULL WHERE subscription_id = $1")
        .bind(subscription_id)
        .bind(next)
        .execute(conn)
        .await?;
    Ok(())
}

/// Run one claimed subscription.
pub async fn run_one(state: &AppState, tenant_id: Uuid, subscription_id: Uuid) -> ApiResult<Outcome> {
    let mut tx = state.db.begin(tenant_id).await?;
    let due: Option<Due> = sqlx::query_as(
        "SELECT s.user_id, s.frequency, s.hour, s.weekday, f.id AS filter_id, f.name, f.query, f.shared, f.owner_id \
         FROM filter_subscriptions s JOIN saved_filters f ON f.id = s.filter_id AND f.tenant_id = s.tenant_id \
         WHERE s.tenant_id = $1 AND s.id = $2",
    )
    .bind(tenant_id)
    .bind(subscription_id)
    .fetch_optional(tx.conn())
    .await?;
    let Some(due) = due else {
        tx.commit().await?;
        return Ok(Outcome::Gone);
    };
    let end = |mut tx: tenant_context::TenantTx| async move {
        sqlx::query("DELETE FROM filter_subscriptions WHERE tenant_id = $1 AND id = $2")
            .bind(tenant_id)
            .bind(subscription_id)
            .execute(tx.conn())
            .await?;
        tx.commit().await?;
        Ok::<_, ApiError>(Outcome::Ended)
    };
    // The filter was made private by its owner, or the subscriber left the workspace.
    if !due.shared && due.owner_id != due.user_id {
        return end(tx).await;
    }
    let Some(role) = state.people.active_role(tenant_id, due.user_id).await? else {
        return end(tx).await;
    };
    let ctx = OrgContext {
        user_id: due.user_id,
        session_id: Uuid::nil(),
        tenant_id,
        organization_id: Uuid::nil(),
        role,
        email: String::new(),
        name: String::new(),
    };

    let frequency = Frequency::parse(&due.frequency).unwrap_or(Frequency::Daily);
    let hour = u8::try_from(due.hour).unwrap_or_default();
    let weekday = due.weekday.and_then(|d| u8::try_from(d).ok());
    let now = OffsetDateTime::now_utc();
    let next = next_run(now, frequency, hour, weekday);

    let compiled = match compile(state, &ctx, &due.query).await {
        Ok(compiled) => compiled,
        Err(ApiError::Validation(_)) => {
            reschedule(tx.conn(), subscription_id, next).await?;
            tx.commit().await?;
            return Ok(Outcome::Skipped);
        }
        Err(other) => return Err(other),
    };
    let (total, total_is_lower_bound, issues) = match visible(state, &ctx, None).await? {
        None => (0, false, vec![]),
        Some(projects) => {
            let filters = Filters { projects, ..Filters::default() };
            let limit = u32::try_from(MAX_DIGEST_ISSUES).unwrap_or(20);
            let found = state.index.search_compiled(tenant_id, &compiled, &filters, limit, None).await?;
            let issues = found
                .hits
                .into_iter()
                .map(|h| DigestIssueV1 { issue_id: h.doc.issue_id, key: h.doc.key, summary: h.doc.summary, status: h.doc.status })
                .collect();
            (found.total, found.total_is_lower_bound, issues)
        }
    };
    let digest = EventEnvelope::new(
        Some(tenant_id),
        subscription_id,
        1,
        Actor::System(SERVICE_NAME.into()),
        FilterDigestV1 {
            subscription_id,
            filter_id: due.filter_id,
            recipient_id: due.user_id,
            filter_name: due.name,
            query: due.query,
            total,
            total_is_lower_bound,
            issues,
        },
    );
    outbox::enqueue(tx.conn(), &digest).await?;
    sqlx::query("UPDATE filter_subscriptions SET last_sent_at = $3 WHERE tenant_id = $1 AND id = $2")
        .bind(tenant_id)
        .bind(subscription_id)
        .bind(now)
        .execute(tx.conn())
        .await?;
    reschedule(tx.conn(), subscription_id, next).await?;
    tx.commit().await?;
    Ok(Outcome::Sent)
}

/// Claim what is due on this database and run it; returns how many were claimed.
pub async fn run_due(state: &AppState, pool: &PgPool) -> ApiResult<usize> {
    let claimed: Vec<(Uuid, Uuid)> = sqlx::query_as(
        "UPDATE subscription_schedule SET leased_until = now() + make_interval(secs => $2) \
         WHERE subscription_id IN ( \
           SELECT subscription_id FROM subscription_schedule \
           WHERE next_run_at <= now() AND (leased_until IS NULL OR leased_until < now()) \
           ORDER BY next_run_at LIMIT $1 FOR UPDATE SKIP LOCKED) \
         RETURNING subscription_id, tenant_id",
    )
    .bind(BATCH)
    .bind(LEASE_SECONDS)
    .fetch_all(pool)
    .await?;
    for (subscription_id, tenant_id) in &claimed {
        match run_one(state, *tenant_id, *subscription_id).await {
            Ok(outcome) => tracing::info!(%tenant_id, %subscription_id, ?outcome, "filter subscription ran"),
            // The lease runs out and the subscription is tried again.
            Err(error) => tracing::warn!(%tenant_id, %subscription_id, %error, "filter subscription failed; will retry"),
        }
    }
    Ok(claimed.len())
}

/// One runner loop per database.
pub fn spawn(state: AppState, pools: Vec<PgPool>) {
    for pool in pools {
        let state = state.clone();
        tokio::spawn(async move {
            loop {
                match run_due(&state, &pool).await {
                    Ok(n) if n as i64 == BATCH => continue,
                    Ok(_) => tokio::time::sleep(IDLE).await,
                    Err(error) => {
                        tracing::warn!(%error, "filter subscription pass failed");
                        tokio::time::sleep(IDLE).await;
                    }
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    #[test]
    fn runs_are_always_in_the_future_at_the_chosen_hour() {
        // A Wednesday.
        let now = datetime!(2026-11-18 10:30 UTC);
        assert_eq!(next_run(now, Frequency::Daily, 11, None), datetime!(2026-11-18 11:00 UTC));
        assert_eq!(next_run(now, Frequency::Daily, 10, None), datetime!(2026-11-19 10:00 UTC));
        assert_eq!(next_run(now, Frequency::Weekly, 9, Some(5)), datetime!(2026-11-20 09:00 UTC), "Friday this week");
        assert_eq!(next_run(now, Frequency::Weekly, 9, Some(3)), datetime!(2026-11-25 09:00 UTC), "today has passed");
        assert_eq!(next_run(now, Frequency::Weekly, 12, Some(3)), datetime!(2026-11-18 12:00 UTC), "later today");
        assert_eq!(next_run(now, Frequency::Weekly, 0, Some(1)), datetime!(2026-11-23 00:00 UTC), "next Monday");
        let exactly = datetime!(2026-11-18 11:00 UTC);
        assert_eq!(next_run(exactly, Frequency::Daily, 11, None), datetime!(2026-11-19 11:00 UTC), "never now");
    }

    #[test]
    fn options_are_validated() {
        let put = |frequency, hour, weekday| PutSubscription { frequency, hour, weekday };
        assert!(validate(&put(Frequency::Daily, 8, None)).is_ok());
        assert!(validate(&put(Frequency::Weekly, 8, Some(7))).is_ok());
        assert!(validate(&put(Frequency::Daily, 24, None)).is_err());
        assert!(validate(&put(Frequency::Weekly, 8, None)).is_err());
        assert!(validate(&put(Frequency::Weekly, 8, Some(0))).is_err());
        assert!(validate(&put(Frequency::Daily, 8, Some(1))).is_err());
    }
}
