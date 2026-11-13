//! A small ClickHouse client over its HTTP interface. Values never go into SQL text: they
//! are sent as typed query parameters (`{name:Type}` in the SQL, `param_name` on the wire),
//! which ClickHouse parses as values, not as SQL.

use std::time::Duration;

use error_handling::ApiError;
use serde::{de::DeserializeOwned, Serialize};
use uuid::Uuid;

#[derive(Clone)]
pub struct ClickHouse {
    http: reqwest::Client,
    url: String,
    database: String,
    user: String,
    password: String,
}

/// Connection settings from the environment.
#[derive(Clone, Debug)]
pub struct Settings {
    pub url: String,
    pub database: String,
}

fn identifier(value: &str) -> anyhow::Result<&str> {
    let ok = !value.is_empty()
        && value.len() <= 64
        && value.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');
    if ok {
        Ok(value)
    } else {
        anyhow::bail!("{value:?} is not a plain ClickHouse identifier")
    }
}

impl ClickHouse {
    pub fn new(settings: &Settings, user: &str, password: &str) -> anyhow::Result<Self> {
        Ok(Self {
            http: reqwest::Client::builder().timeout(Duration::from_secs(15)).build()?,
            url: settings.url.trim_end_matches('/').to_owned(),
            database: identifier(&settings.database)?.to_owned(),
            user: user.to_owned(),
            password: password.to_owned(),
        })
    }

    pub fn database(&self) -> &str {
        &self.database
    }

    async fn post(&self, params: &[(String, String)], body: String) -> anyhow::Result<String> {
        let response = self
            .http
            .post(&self.url)
            .header("X-ClickHouse-User", &self.user)
            .header("X-ClickHouse-Key", &self.password)
            .query(&[("database", self.database.as_str())])
            .query(params)
            .body(body)
            .send()
            .await?;
        let status = response.status();
        let text = response.text().await?;
        if !status.is_success() {
            anyhow::bail!("ClickHouse returned {status}: {}", text.chars().take(500).collect::<String>());
        }
        Ok(text)
    }

    /// Run a statement that returns nothing (DDL).
    pub async fn execute(&self, sql: &str) -> anyhow::Result<()> {
        self.post(&[], sql.to_owned()).await.map(|_| ())
    }

    /// Insert rows as JSON. Timestamps may be RFC 3339.
    pub async fn insert<T: Serialize>(&self, table: &str, rows: &[T]) -> anyhow::Result<()> {
        if rows.is_empty() {
            return Ok(());
        }
        let mut body = String::new();
        for row in rows {
            body.push_str(&serde_json::to_string(row)?);
            body.push('\n');
        }
        let params = [
            ("query".to_owned(), format!("INSERT INTO {} FORMAT JSONEachRow", identifier(table)?)),
            ("date_time_input_format".to_owned(), "best_effort".to_owned()),
        ];
        self.post(&params, body).await.map(|_| ())
    }

    /// Run a statement with parameters that returns nothing (`INSERT … SELECT`).
    pub async fn command(&self, sql: &str, params: &Params) -> anyhow::Result<()> {
        self.post(&params.0, sql.to_owned()).await.map(|_| ())
    }

    /// Run a query and parse each row. `tenant` also becomes the `SQL_tenant_id` setting,
    /// which the reader's row policies require.
    pub async fn query<T: DeserializeOwned>(&self, tenant: Uuid, sql: &str, params: &Params) -> anyhow::Result<Vec<T>> {
        let mut all = params.0.clone();
        all.push(("SQL_tenant_id".to_owned(), tenant.to_string()));
        all.push(("output_format_json_quote_64bit_integers".to_owned(), "0".to_owned()));
        all.push(("param_tenant".to_owned(), tenant.to_string()));
        let text = self.post(&all, format!("{sql}\nFORMAT JSONEachRow")).await?;
        text.lines().filter(|l| !l.is_empty()).map(|l| Ok(serde_json::from_str(l)?)).collect()
    }

    pub async fn ping(&self) -> bool {
        self.post(&[], "SELECT 1".to_owned()).await.is_ok()
    }
}

/// Typed query parameters: `.add("from", value)` fills `{from:Type}` in the SQL.
#[derive(Default, Clone)]
pub struct Params(Vec<(String, String)>);

impl Params {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(mut self, name: &str, value: impl ToString) -> Self {
        self.0.push((format!("param_{name}"), value.to_string()));
        self
    }

    /// An `Array(UUID)` parameter.
    pub fn uuids(self, name: &str, ids: &[Uuid]) -> Self {
        let list = ids.iter().map(|id| format!("'{id}'")).collect::<Vec<_>>().join(",");
        self.add(name, format!("[{list}]"))
    }
}

/// Failures talking to ClickHouse are logged in full and reported to clients without detail.
pub fn unavailable(error: anyhow::Error) -> ApiError {
    tracing::error!(%error, "analytics query failed");
    ApiError::DependencyUnavailable("analytics store")
}

/// Create the tables and row policies. `reader` and `writer` are the users that get access.
pub async fn migrate(owner: &ClickHouse, reader: &str, writer: &str) -> anyhow::Result<()> {
    let sql = include_str!("../clickhouse/0001_facts.sql")
        .replace("{reader}", identifier(reader)?)
        .replace("{writer}", identifier(writer)?)
        .replace("{database}", owner.database());
    let without_comments: String = sql.lines().filter(|l| !l.trim_start().starts_with("--")).map(|l| format!("{l}\n")).collect();
    for statement in without_comments.split(";\n").map(str::trim).filter(|s| !s.is_empty()) {
        owner.execute(statement).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_plain_identifiers_are_spliced_into_sql() {
        assert!(identifier("analytics_reader").is_ok());
        assert!(identifier("analytics; DROP TABLE x").is_err());
        assert!(identifier("Reader").is_err());
        assert!(identifier("").is_err());
    }

    #[test]
    fn uuid_arrays_are_quoted_literals() {
        let a = Uuid::nil();
        let params = Params::new().uuids("p", &[a]).add("n", 3);
        assert_eq!(params.0, vec![("param_p".into(), format!("['{a}']")), ("param_n".into(), "3".into())]);
    }
}
