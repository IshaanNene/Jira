//! Redis side of realtime (ADR-0008). Each channel has a capped stream
//! `rt:{tenant}:{channel}` that serves as a short replay buffer (its ids are the client
//! cursors); a single Pub/Sub channel tells every instance about new entries, and each
//! instance fans them out to its own connections through a broadcast channel.

use std::{sync::Arc, time::Duration};

use auth_middleware::RedisHandle;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use uuid::Uuid;

use crate::channels::{Channel, StreamId};

const FANOUT: &str = "rt:fanout";
/// Entries kept per channel. A client further behind than this resyncs.
const STREAM_LENGTH: usize = 1000;
const STREAM_TTL_SECS: i64 = 24 * 3600;
pub const MAX_REPLAY: usize = 500;

fn stream_key(tenant: Uuid, channel: &Channel) -> String {
    format!("rt:{tenant}:{channel}")
}

/// Something every connection on this instance may need to hear.
#[derive(Debug, Clone)]
pub enum Live {
    Entry(Entry),
    /// The fanout connection was interrupted: entries may have been missed.
    Resync,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    #[serde(rename = "t")]
    pub tenant: Uuid,
    #[serde(rename = "c")]
    pub channel: String,
    #[serde(rename = "i")]
    pub id: String,
    #[serde(rename = "d")]
    pub data: String,
}

pub enum Replay {
    /// Everything after the cursor, oldest first.
    Entries(Vec<(StreamId, String)>),
    /// The cursor is older than the buffer: the client must refetch.
    Resync,
}

pub struct Hub {
    redis: RedisHandle,
    live: broadcast::Sender<Arc<Live>>,
}

impl Hub {
    /// Starts the fanout listener for this instance.
    pub fn start(redis_url: &str, redis: RedisHandle) -> anyhow::Result<Arc<Self>> {
        let client = redis::Client::open(redis_url)?;
        let (live, _) = broadcast::channel(4096);
        let hub = Arc::new(Self { redis, live });
        tokio::spawn(listen(client, hub.live.clone()));
        Ok(hub)
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Arc<Live>> {
        self.live.subscribe()
    }

    fn conn(&self) -> anyhow::Result<redis::aio::ConnectionManager> {
        self.redis.get().ok_or_else(|| anyhow::anyhow!("redis is not connected"))
    }

    /// Append to the channel's buffer and tell every instance.
    pub async fn publish(&self, tenant: Uuid, channel: &Channel, data: &str) -> anyhow::Result<()> {
        let mut conn = self.conn()?;
        let key = stream_key(tenant, channel);
        let id: String = redis::cmd("XADD")
            .arg(&key)
            .arg("MAXLEN")
            .arg("~")
            .arg(STREAM_LENGTH)
            .arg("*")
            .arg("m")
            .arg(data)
            .query_async(&mut conn)
            .await?;
        let entry = Entry { tenant, channel: channel.to_string(), id, data: data.to_owned() };
        redis::pipe()
            .cmd("EXPIRE")
            .arg(&key)
            .arg(STREAM_TTL_SECS)
            .ignore()
            .cmd("PUBLISH")
            .arg(FANOUT)
            .arg(serde_json::to_string(&entry)?)
            .ignore()
            .query_async::<()>(&mut conn)
            .await?;
        Ok(())
    }

    /// The newest id in the channel's buffer (`0-0` when it is empty).
    pub async fn tip(&self, tenant: Uuid, channel: &Channel) -> anyhow::Result<StreamId> {
        let mut conn = self.conn()?;
        let last: Vec<(String, Vec<String>)> = redis::cmd("XREVRANGE")
            .arg(stream_key(tenant, channel))
            .arg("+")
            .arg("-")
            .arg("COUNT")
            .arg(1)
            .query_async(&mut conn)
            .await?;
        Ok(last.first().and_then(|(id, _)| StreamId::parse(id)).unwrap_or(StreamId(0, 0)))
    }

    /// Entries after `cursor`, or `Resync` if some of them have already been trimmed.
    pub async fn replay(&self, tenant: Uuid, channel: &Channel, cursor: StreamId) -> anyhow::Result<Replay> {
        let mut conn = self.conn()?;
        let key = stream_key(tenant, channel);
        // Redis 7 records the newest id it has trimmed away: a cursor older than that has
        // missed something.
        let info: Result<redis::Value, _> = redis::cmd("XINFO").arg("STREAM").arg(&key).query_async(&mut conn).await;
        let trimmed = match info {
            Ok(value) => max_deleted(&value).unwrap_or(StreamId(0, 0)),
            // No such key: never written, or expired after a day of silence.
            Err(_) => return Ok(if cursor == StreamId(0, 0) { Replay::Entries(vec![]) } else { Replay::Resync }),
        };
        if cursor < trimmed {
            return Ok(Replay::Resync);
        }
        let rows: Vec<(String, Vec<String>)> = redis::cmd("XRANGE")
            .arg(&key)
            .arg(format!("({cursor}"))
            .arg("+")
            .arg("COUNT")
            .arg(MAX_REPLAY + 1)
            .query_async(&mut conn)
            .await?;
        if rows.len() > MAX_REPLAY {
            return Ok(Replay::Resync);
        }
        Ok(Replay::Entries(
            rows.into_iter()
                .filter_map(|(id, fields)| {
                    let id = StreamId::parse(&id)?;
                    // fields = ["m", data]
                    let data = fields.into_iter().nth(1)?;
                    Some((id, data))
                })
                .collect(),
        ))
    }
}

/// `max-deleted-entry-id` from an `XINFO STREAM` reply (a flat key/value array).
fn max_deleted(value: &redis::Value) -> Option<StreamId> {
    let redis::Value::Array(items) = value else { return None };
    let text = |v: &redis::Value| match v {
        redis::Value::BulkString(b) => String::from_utf8(b.clone()).ok(),
        redis::Value::SimpleString(s) => Some(s.clone()),
        _ => None,
    };
    items
        .chunks(2)
        .find(|pair| text(&pair[0]).as_deref() == Some("max-deleted-entry-id"))
        .and_then(|pair| pair.get(1))
        .and_then(text)
        .and_then(|id| StreamId::parse(&id))
}

/// Holds the instance's Pub/Sub subscription, reconnecting as needed. After a reconnect
/// every connection is told to resync, since entries may have been missed meanwhile.
async fn listen(client: redis::Client, live: broadcast::Sender<Arc<Live>>) {
    let mut delay = Duration::from_millis(200);
    let mut connected_before = false;
    loop {
        match client.get_async_pubsub().await {
            Ok(mut pubsub) => match pubsub.subscribe(FANOUT).await {
                Ok(()) => {
                    delay = Duration::from_millis(200);
                    if connected_before {
                        let _ = live.send(Arc::new(Live::Resync));
                    }
                    connected_before = true;
                    tracing::info!("realtime fanout subscribed");
                    let mut messages = pubsub.on_message();
                    while let Some(message) = messages.next().await {
                        let Ok(raw) = message.get_payload::<String>() else { continue };
                        match serde_json::from_str::<Entry>(&raw) {
                            // No receivers is fine: nobody is connected to this instance.
                            Ok(entry) => drop(live.send(Arc::new(Live::Entry(entry)))),
                            Err(error) => tracing::warn!(%error, "malformed fanout message"),
                        }
                    }
                    tracing::warn!("realtime fanout connection closed; reconnecting");
                }
                Err(error) => tracing::warn!(%error, "realtime fanout subscribe failed"),
            },
            Err(error) => tracing::warn!(%error, "realtime fanout unavailable"),
        }
        tokio::time::sleep(delay).await;
        delay = (delay * 2).min(Duration::from_secs(10));
    }
}
