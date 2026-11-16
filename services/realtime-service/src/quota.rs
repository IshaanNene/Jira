//! Live connections per workspace and per person, across every instance.
//!
//! Each open stream holds a lease in two Redis sorted sets, `rt:conn:{tenant}` and
//! `rt:conn:{tenant}:<user>` (one hash slot), scored by when the lease runs out. A stream
//! renews its lease while open and gives it back when it closes; a lease of an instance
//! that died runs out on its own. Taking a lease drops the expired ones and refuses when a
//! set is full, atomically (one script), so concurrent connects cannot overshoot.
//!
//! Without Redis the per-instance cap (`REALTIME_MAX_CONNECTIONS`) still applies, and
//! streams are allowed: they cannot replay or fan out anyway until Redis is back.

use std::time::Duration;

use auth_middleware::RedisHandle;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    Workspace,
    Person,
}

const ACQUIRE: &str = r"
redis.call('ZREMRANGEBYSCORE', KEYS[1], '-inf', ARGV[1])
redis.call('ZREMRANGEBYSCORE', KEYS[2], '-inf', ARGV[1])
if redis.call('ZCARD', KEYS[1]) >= tonumber(ARGV[4]) then return 1 end
if redis.call('ZCARD', KEYS[2]) >= tonumber(ARGV[5]) then return 2 end
redis.call('ZADD', KEYS[1], ARGV[2], ARGV[3])
redis.call('ZADD', KEYS[2], ARGV[2], ARGV[3])
redis.call('PEXPIREAT', KEYS[1], ARGV[2])
redis.call('PEXPIREAT', KEYS[2], ARGV[2])
return 0
";

const RENEW: &str = r"
redis.call('ZADD', KEYS[1], 'XX', ARGV[1], ARGV[2])
redis.call('ZADD', KEYS[2], 'XX', ARGV[1], ARGV[2])
redis.call('PEXPIREAT', KEYS[1], ARGV[1])
redis.call('PEXPIREAT', KEYS[2], ARGV[1])
return 0
";

pub struct Quotas {
    redis: RedisHandle,
    pub per_workspace: usize,
    pub per_person: usize,
    /// How long a lease lasts without renewal; streams renew at a third of it.
    pub lease: Duration,
}

/// One stream's place in its workspace's and person's quota.
#[derive(Debug, Clone)]
pub struct Lease {
    tenant: Uuid,
    user: Uuid,
    id: Uuid,
}

fn keys(tenant: Uuid, user: Uuid) -> [String; 2] {
    [format!("rt:conn:{{{tenant}}}"), format!("rt:conn:{{{tenant}}}:{user}")]
}

fn now_ms() -> i64 {
    (time::OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000) as i64
}

impl Quotas {
    pub fn new(redis: RedisHandle, per_workspace: usize, per_person: usize, lease: Duration) -> Self {
        Self { redis, per_workspace, per_person, lease }
    }

    fn until(&self) -> i64 {
        now_ms() + i64::try_from(self.lease.as_millis()).unwrap_or(i64::MAX / 2)
    }

    /// Take a place, or say whose quota is full. `Ok(None)`: Redis is unavailable and the
    /// stream goes ahead uncounted.
    pub async fn acquire(&self, tenant: Uuid, user: Uuid) -> Result<Option<Lease>, Refusal> {
        let Some(mut conn) = self.redis.get() else {
            metrics::counter!("realtime_quota_unchecked_total").increment(1);
            return Ok(None);
        };
        let lease = Lease { tenant, user, id: Uuid::now_v7() };
        let [workspace, person] = keys(tenant, user);
        let result: Result<i64, _> = redis::Script::new(ACQUIRE)
            .key(workspace)
            .key(person)
            .arg(now_ms())
            .arg(self.until())
            .arg(lease.id.to_string())
            .arg(self.per_workspace)
            .arg(self.per_person)
            .invoke_async(&mut conn)
            .await;
        match result {
            Ok(0) => Ok(Some(lease)),
            Ok(1) => Err(Refusal::Workspace),
            Ok(_) => Err(Refusal::Person),
            Err(error) => {
                tracing::warn!(%error, "connection quota unavailable; allowing the stream");
                metrics::counter!("realtime_quota_unchecked_total").increment(1);
                Ok(None)
            }
        }
    }

    pub async fn renew(&self, lease: &Lease) {
        let Some(mut conn) = self.redis.get() else { return };
        let [workspace, person] = keys(lease.tenant, lease.user);
        let result: Result<i64, _> = redis::Script::new(RENEW)
            .key(workspace)
            .key(person)
            .arg(self.until())
            .arg(lease.id.to_string())
            .invoke_async(&mut conn)
            .await;
        if let Err(error) = result {
            tracing::warn!(%error, "could not renew a connection lease");
        }
    }

    pub async fn release(&self, lease: &Lease) {
        let Some(mut conn) = self.redis.get() else { return };
        let [workspace, person] = keys(lease.tenant, lease.user);
        let id = lease.id.to_string();
        let result: Result<(), _> =
            redis::pipe().zrem(&workspace, &id).ignore().zrem(&person, &id).ignore().query_async(&mut conn).await;
        if let Err(error) = result {
            tracing::warn!(%error, "could not release a connection lease; it runs out on its own");
        }
    }
}
