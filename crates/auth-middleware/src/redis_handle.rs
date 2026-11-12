//! A Redis connection that may not exist yet. Services start (and keep serving) while
//! Redis is unavailable; features that need it degrade as documented and pick the
//! connection up as soon as it is established.

use std::{sync::Arc, time::Duration};

use redis::aio::ConnectionManager;
use tokio::sync::OnceCell;

#[derive(Clone, Default)]
pub struct RedisHandle(Arc<OnceCell<ConnectionManager>>);

impl RedisHandle {
    /// Returns immediately; connects in the background with capped backoff. Once
    /// connected, `ConnectionManager` handles reconnection itself.
    pub fn connect_in_background(url: &str) -> anyhow::Result<Self> {
        let client = redis::Client::open(url)?;
        let handle = Self::default();
        let cell = Arc::clone(&handle.0);
        tokio::spawn(async move {
            let mut delay = Duration::from_millis(200);
            loop {
                match tokio::time::timeout(Duration::from_secs(3), ConnectionManager::new(client.clone())).await {
                    Ok(Ok(manager)) => {
                        let _ = cell.set(manager);
                        tracing::info!("redis connected");
                        return;
                    }
                    Ok(Err(err)) => tracing::warn!(error = %err, "redis unavailable, retrying"),
                    Err(_) => tracing::warn!("redis connect timed out, retrying"),
                }
                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(Duration::from_secs(10));
            }
        });
        Ok(handle)
    }

    pub fn get(&self) -> Option<ConnectionManager> {
        self.0.get().cloned()
    }

    pub fn is_connected(&self) -> bool {
        self.0.initialized()
    }
}
