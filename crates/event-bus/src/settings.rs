use rdkafka::ClientConfig;

/// Connection settings, from this service's own environment.
#[derive(Clone)]
pub struct KafkaSettings {
    pub brokers: String,
    pub client_id: String,
    /// TLS and SASL for managed brokers; `None` is plaintext (local development only).
    pub security: Option<Security>,
}

/// `KAFKA_SECURITY_PROTOCOL` is `ssl` or `sasl_ssl`; with `sasl_ssl`, `KAFKA_SASL_MECHANISM`
/// (`SCRAM-SHA-512` by default), `KAFKA_SASL_USERNAME` and `KAFKA_SASL_PASSWORD`.
/// `KAFKA_SSL_CA_LOCATION` points at a CA bundle when the brokers' CA is not a public one.
#[derive(Clone)]
pub struct Security {
    pub protocol: String,
    pub ca_location: Option<String>,
    pub sasl: Option<(String, String, String)>,
}

// Never print the SASL password.
impl std::fmt::Debug for KafkaSettings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KafkaSettings")
            .field("brokers", &self.brokers)
            .field("client_id", &self.client_id)
            .field("security", &self.security.as_ref().map(|s| &s.protocol))
            .finish()
    }
}

fn var(name: &str) -> Option<String> {
    std::env::var(name).ok().map(|v| v.trim().to_owned()).filter(|v| !v.is_empty())
}

impl Security {
    fn from_env() -> anyhow::Result<Option<Self>> {
        Self::from_vars(var)
    }

    fn from_vars(var: impl Fn(&str) -> Option<String>) -> anyhow::Result<Option<Self>> {
        let protocol = match var("KAFKA_SECURITY_PROTOCOL").map(|p| p.to_ascii_lowercase()) {
            None => return Ok(None),
            Some(p) if p == "plaintext" => return Ok(None),
            Some(p) if p == "ssl" || p == "sasl_ssl" => p,
            Some(other) => anyhow::bail!("KAFKA_SECURITY_PROTOCOL must be plaintext, ssl or sasl_ssl, not {other}"),
        };
        let sasl = if protocol == "sasl_ssl" {
            let mechanism = var("KAFKA_SASL_MECHANISM").unwrap_or_else(|| "SCRAM-SHA-512".into());
            if !matches!(mechanism.as_str(), "SCRAM-SHA-256" | "SCRAM-SHA-512" | "PLAIN") {
                anyhow::bail!("KAFKA_SASL_MECHANISM must be SCRAM-SHA-256, SCRAM-SHA-512 or PLAIN");
            }
            let user =
                var("KAFKA_SASL_USERNAME").ok_or_else(|| anyhow::anyhow!("KAFKA_SASL_USERNAME is required with sasl_ssl"))?;
            let password =
                var("KAFKA_SASL_PASSWORD").ok_or_else(|| anyhow::anyhow!("KAFKA_SASL_PASSWORD is required with sasl_ssl"))?;
            Some((mechanism, user, password))
        } else {
            None
        };
        Ok(Some(Self { protocol, ca_location: var("KAFKA_SSL_CA_LOCATION"), sasl }))
    }
}

impl KafkaSettings {
    /// `KAFKA_BROKERS` (comma-separated). `None` when it is not set: the service then runs
    /// without the event bus (the outbox holds events, nothing is consumed).
    ///
    /// Panics on contradictory security settings: a service must not start talking plaintext
    /// to brokers it was told to reach over TLS.
    pub fn from_env(client_id: &str) -> Option<Self> {
        let brokers = var("KAFKA_BROKERS")?;
        let security = Security::from_env().unwrap_or_else(|e| panic!("invalid Kafka security settings: {e}"));
        Some(Self { brokers, client_id: client_id.to_owned(), security })
    }

    pub fn new(brokers: impl Into<String>, client_id: impl Into<String>) -> Self {
        Self { brokers: brokers.into(), client_id: client_id.into(), security: None }
    }

    pub(crate) fn base(&self) -> ClientConfig {
        let mut config = ClientConfig::new();
        config.set("bootstrap.servers", &self.brokers).set("client.id", &self.client_id);
        if let Some(security) = &self.security {
            config.set("security.protocol", &security.protocol).set("ssl.endpoint.identification.algorithm", "https");
            if let Some(ca) = &security.ca_location {
                config.set("ssl.ca.location", ca);
            }
            if let Some((mechanism, user, password)) = &security.sasl {
                config.set("sasl.mechanism", mechanism).set("sasl.username", user).set("sasl.password", password);
            }
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: std::collections::HashMap<String, String> = pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        move |name| map.get(name).cloned()
    }

    #[test]
    fn security_settings_are_complete_or_refused() {
        assert!(Security::from_vars(vars(&[])).unwrap().is_none());
        assert!(Security::from_vars(vars(&[("KAFKA_SECURITY_PROTOCOL", "PLAINTEXT")])).unwrap().is_none());
        let tls = Security::from_vars(vars(&[("KAFKA_SECURITY_PROTOCOL", "SSL")])).unwrap().unwrap();
        assert_eq!(tls.protocol, "ssl");
        assert!(tls.sasl.is_none());

        let scram = Security::from_vars(vars(&[
            ("KAFKA_SECURITY_PROTOCOL", "sasl_ssl"),
            ("KAFKA_SASL_USERNAME", "svc"),
            ("KAFKA_SASL_PASSWORD", "secret"),
        ]))
        .unwrap()
        .unwrap();
        assert_eq!(scram.sasl.as_ref().unwrap().0, "SCRAM-SHA-512");

        // Missing credentials, unknown protocols and mechanisms are refused, not downgraded.
        assert!(Security::from_vars(vars(&[("KAFKA_SECURITY_PROTOCOL", "sasl_ssl"), ("KAFKA_SASL_USERNAME", "svc")])).is_err());
        assert!(Security::from_vars(vars(&[("KAFKA_SECURITY_PROTOCOL", "sasl_plaintext")])).is_err());
        assert!(Security::from_vars(vars(&[
            ("KAFKA_SECURITY_PROTOCOL", "sasl_ssl"),
            ("KAFKA_SASL_MECHANISM", "GSSAPI"),
            ("KAFKA_SASL_USERNAME", "svc"),
            ("KAFKA_SASL_PASSWORD", "secret"),
        ]))
        .is_err());

        let settings = KafkaSettings { brokers: "b:9096".into(), client_id: "x".into(), security: Some(scram) };
        assert!(!format!("{settings:?}").contains("secret"));
    }
}
