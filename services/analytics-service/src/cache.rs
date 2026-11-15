//! A short-lived Redis cache of chart answers.
//!
//! Dashboards are opened far more often than their numbers change, and every chart is a
//! ClickHouse query. An answer is reused only when everything it depends on is the same:
//! the tenant, the query, the projects the viewer can see, the UTC day, and the tenant's
//! data generation, which the ingester bumps after every write. New events therefore show
//! up at once, and the time-to-live only bounds how long an unused answer lingers.
//!
//! The cache is best effort: without Redis, or when Redis is slow, charts are computed.

use std::time::Duration;

use auth_middleware::RedisHandle;
use sha2::{Digest, Sha256};
use time::{Date, OffsetDateTime};
use uuid::Uuid;

use crate::widgets::{WidgetData, WidgetQuery};

/// How long an answer is kept when its tenant's data does not change.
const TTL: Duration = Duration::from_secs(300);
/// How long a cache read or write may take before the chart is computed instead.
const TIMEOUT: Duration = Duration::from_millis(100);
/// Answers larger than this are not cached.
const MAX_BYTES: usize = 1 << 20;

#[derive(Clone, Default)]
pub struct ChartCache {
    redis: RedisHandle,
}

fn generation_key(tenant: Uuid) -> String {
    format!("analytics:generation:{tenant}")
}

/// The cache key. Only a digest of the query and scope is stored in the key.
pub fn key(tenant: Uuid, generation: i64, today: Date, query: &WidgetQuery, visible: Option<&[Uuid]>) -> String {
    let mut scope: Option<Vec<Uuid>> = visible.map(<[Uuid]>::to_vec);
    if let Some(ids) = scope.as_mut() {
        ids.sort_unstable();
        ids.dedup();
    }
    let mut digest = Sha256::new();
    digest.update(serde_json::to_vec(query).unwrap_or_default());
    digest.update(b"\0");
    digest.update(serde_json::to_vec(&scope).unwrap_or_default());
    format!("analytics:chart:{tenant}:{generation}:{today}:{}", hex::encode(digest.finalize()))
}

impl ChartCache {
    pub fn new(redis: RedisHandle) -> Self {
        Self { redis }
    }

    async fn generation(&self, tenant: Uuid) -> Option<i64> {
        let mut conn = self.redis.get()?;
        let mut cmd = redis::cmd("GET");
        cmd.arg(generation_key(tenant));
        match tokio::time::timeout(TIMEOUT, cmd.query_async::<Option<i64>>(&mut conn)).await {
            Ok(Ok(generation)) => Some(generation.unwrap_or(0)),
            _ => None,
        }
    }

    /// The cached answer, or `compute`'s, which is then cached.
    pub async fn get_or_compute<F, Fut, E>(
        &self,
        tenant: Uuid,
        query: &WidgetQuery,
        visible: Option<&[Uuid]>,
        compute: F,
    ) -> Result<WidgetData, E>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<WidgetData, E>>,
    {
        let Some(generation) = self.generation(tenant).await else {
            return compute().await;
        };
        let key = key(tenant, generation, OffsetDateTime::now_utc().date(), query, visible);
        let Some(mut conn) = self.redis.get() else {
            return compute().await;
        };
        let mut get = redis::cmd("GET");
        get.arg(&key);
        if let Ok(Ok(Some(bytes))) = tokio::time::timeout(TIMEOUT, get.query_async::<Option<Vec<u8>>>(&mut conn)).await {
            if let Ok(answer) = serde_json::from_slice(&bytes) {
                return Ok(answer);
            }
        }
        let answer = compute().await?;
        if let Ok(bytes) = serde_json::to_vec(&answer) {
            if bytes.len() <= MAX_BYTES {
                let mut set = redis::cmd("SET");
                set.arg(&key).arg(bytes).arg("EX").arg(TTL.as_secs());
                let _ = tokio::time::timeout(TIMEOUT, set.query_async::<()>(&mut conn)).await;
            }
        }
        Ok(answer)
    }

    /// The tenant's data changed: answers computed before no longer match any key.
    pub async fn invalidate(&self, tenant: Uuid) {
        let Some(mut conn) = self.redis.get() else { return };
        let mut cmd = redis::cmd("INCR");
        cmd.arg(generation_key(tenant));
        if !matches!(tokio::time::timeout(TIMEOUT, cmd.query_async::<i64>(&mut conn)).await, Ok(Ok(_))) {
            tracing::warn!(tenant_id = %tenant, "could not mark cached charts stale");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_separate_everything_an_answer_depends_on() {
        let (tenant, other, a, b) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
        let day = Date::from_calendar_date(2026, time::Month::March, 2).unwrap();
        let q = WidgetQuery::CumulativeFlow { projects: vec![], days: 30 };
        let base = key(tenant, 1, day, &q, Some(&[a, b]));
        assert_eq!(base, key(tenant, 1, day, &q, Some(&[b, a, a])), "visible projects in any order");
        assert!(base.starts_with(&format!("analytics:chart:{tenant}:1:")));
        assert_ne!(base, key(other, 1, day, &q, Some(&[a, b])));
        assert_ne!(base, key(tenant, 2, day, &q, Some(&[a, b])));
        assert_ne!(base, key(tenant, 1, day.next_day().unwrap(), &q, Some(&[a, b])));
        assert_ne!(base, key(tenant, 1, day, &WidgetQuery::CumulativeFlow { projects: vec![], days: 31 }, Some(&[a, b])));
        assert_ne!(base, key(tenant, 1, day, &q, Some(&[a])));
        assert_ne!(base, key(tenant, 1, day, &q, None), "seeing everything is not seeing these two");
        assert_ne!(key(tenant, 1, day, &q, None), key(tenant, 1, day, &q, Some(&[])));
    }
}
