use rdkafka::ClientConfig;

/// Connection settings, from this service's own environment.
#[derive(Debug, Clone)]
pub struct KafkaSettings {
    pub brokers: String,
    pub client_id: String,
}

impl KafkaSettings {
    /// `KAFKA_BROKERS` (comma-separated). `None` when it is not set: the service then runs
    /// without the event bus (the outbox holds events, nothing is consumed).
    pub fn from_env(client_id: &str) -> Option<Self> {
        let brokers = std::env::var("KAFKA_BROKERS").ok().filter(|b| !b.trim().is_empty())?;
        Some(Self { brokers, client_id: client_id.to_owned() })
    }

    pub fn new(brokers: impl Into<String>, client_id: impl Into<String>) -> Self {
        Self { brokers: brokers.into(), client_id: client_id.into() }
    }

    pub(crate) fn base(&self) -> ClientConfig {
        let mut config = ClientConfig::new();
        config.set("bootstrap.servers", &self.brokers).set("client.id", &self.client_id);
        config
    }

    /// Idempotent producer: no duplicates or reordering from producer retries.
    pub(crate) fn producer(&self) -> ClientConfig {
        let mut config = self.base();
        config
            .set("enable.idempotence", "true")
            .set("acks", "all")
            .set("linger.ms", "5")
            .set("compression.type", "lz4")
            .set("message.timeout.ms", "30000");
        config
    }
}
