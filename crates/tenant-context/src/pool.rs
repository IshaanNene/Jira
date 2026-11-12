use std::{collections::HashMap, time::Duration};

use sqlx::{
    postgres::{PgConnectOptions, PgPoolOptions},
    PgPool,
};

pub const DEFAULT_DESTINATION: &str = "shared/default";

#[derive(Debug, Clone)]
pub struct PoolSettings {
    pub max_connections: u32,
    pub acquire_timeout: Duration,
    pub statement_timeout: Duration,
}

impl Default for PoolSettings {
    fn default() -> Self {
        Self { max_connections: 10, acquire_timeout: Duration::from_secs(3), statement_timeout: Duration::from_secs(5) }
    }
}

impl PoolSettings {
    pub fn from_env() -> anyhow::Result<Self> {
        let d = Self::default();
        Ok(Self {
            max_connections: platform_common::config::parse_or("DATABASE_MAX_CONNECTIONS", d.max_connections)?,
            acquire_timeout: d.acquire_timeout,
            statement_timeout: Duration::from_millis(platform_common::config::parse_or(
                "DATABASE_STATEMENT_TIMEOUT_MS",
                5_000_u64,
            )?),
        })
    }
}

/// Bounded pool with acquire and statement timeouts. Connects lazily so a service can
/// start (and report not-ready) while its database is still coming up.
pub fn connect_pool(url: &str, settings: &PoolSettings) -> anyhow::Result<PgPool> {
    let options: PgConnectOptions = url.parse()?;
    let options = options.options([("statement_timeout", settings.statement_timeout.as_millis().to_string())]);
    Ok(PgPoolOptions::new()
        .max_connections(settings.max_connections)
        .acquire_timeout(settings.acquire_timeout)
        .idle_timeout(Duration::from_secs(300))
        .connect_lazy_with(options))
}

/// `shared/default` -> `SHARED_DEFAULT`. Used to look up `DATABASE_URL__<KEY>`.
pub fn destination_key(destination: &str) -> String {
    destination.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_uppercase() } else { '_' }).collect()
}

/// Build the destination -> pool map from this service's own configuration:
/// `DATABASE_URL` serves [`DEFAULT_DESTINATION`]; each `DATABASE_URL__<KEY>` serves the
/// destination whose [`destination_key`] is `<KEY>`. Connection strings therefore never
/// come from the tenant registry or from a request.
pub fn pools_from_env(settings: &PoolSettings) -> anyhow::Result<HashMap<String, PgPool>> {
    let mut pools = HashMap::new();
    let default_url = platform_common::config::required("DATABASE_URL")?;
    pools.insert(destination_key(DEFAULT_DESTINATION), connect_pool(&default_url, settings)?);
    for (name, value) in std::env::vars() {
        if let Some(key) = name.strip_prefix("DATABASE_URL__") {
            if !key.is_empty() && !value.trim().is_empty() {
                pools.insert(key.to_owned(), connect_pool(&value, settings)?);
            }
        }
    }
    Ok(pools)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn destination_keys_are_env_safe() {
        assert_eq!(destination_key("shared/default"), "SHARED_DEFAULT");
        assert_eq!(destination_key("dedicated-db/eu-1/acme"), "DEDICATED_DB_EU_1_ACME");
    }
}
