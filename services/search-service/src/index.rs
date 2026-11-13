//! The OpenSearch index of issues, spoken to over its REST API.
//!
//! Isolation: one shared index, every document carries `tenant_id`, is stored under the
//! id `{tenant}:{issue}` with the tenant as its shard routing, and every query and delete
//! is built here with a `tenant_id` filter taken from the verified token or event —
//! callers cannot pass a raw query through.

use api_contracts::issue::v1::Document;
use error_handling::{ApiError, ApiResult};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use time::OffsetDateTime;
use uuid::Uuid;

/// Text kept per field; longer descriptions and comment threads are truncated.
const MAX_TEXT: usize = 32 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct IssueDoc {
    pub tenant_id: Uuid,
    pub issue_id: Uuid,
    pub key: String,
    pub project_id: Uuid,
    pub project_key: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub status: String,
    pub status_category: String,
    pub priority: String,
    pub summary: String,
    pub description: String,
    pub comments: String,
    pub assignee_id: Option<Uuid>,
    pub reporter_id: Option<Uuid>,
    pub labels: Vec<String>,
    pub created_at: String,
    pub updated_at: String,
    pub resolved_at: Option<String>,
    pub version: i64,
    pub indexed_at: String,
}

/// Highlight markers: private-use characters that cannot occur in indexed text (see
/// [`clean`]), so a fragment can be split into plain text and matches without ever
/// treating document content as markup.
pub const MARK_START: char = '\u{E000}';
pub const MARK_END: char = '\u{E001}';

/// Control and private-use characters out; length bounded on a character boundary.
pub fn clean(text: &str) -> String {
    let mut out: String = text
        .chars()
        .map(|c| if (c.is_control() && c != '\n' && c != '\t') || ('\u{E000}'..='\u{F8FF}').contains(&c) { ' ' } else { c })
        .collect();
    if out.len() > MAX_TEXT {
        let mut end = MAX_TEXT;
        while !out.is_char_boundary(end) {
            end -= 1;
        }
        out.truncate(end);
    }
    out
}

fn optional_uuid(raw: &str) -> Option<Uuid> {
    Uuid::parse_str(raw).ok()
}

fn optional(raw: String) -> Option<String> {
    (!raw.is_empty()).then_some(raw)
}

fn now() -> String {
    OffsetDateTime::now_utc().format(&time::format_description::well_known::Rfc3339).unwrap_or_default()
}

impl IssueDoc {
    /// `None` when issue-service sent something that is not a valid document.
    pub fn from_document(tenant_id: Uuid, d: Document) -> Option<Self> {
        Some(Self {
            tenant_id,
            issue_id: Uuid::parse_str(&d.id).ok()?,
            key: d.key,
            project_id: Uuid::parse_str(&d.project_id).ok()?,
            project_key: d.project_key,
            kind: d.r#type,
            status: d.status,
            status_category: d.status_category,
            priority: d.priority,
            summary: clean(&d.summary),
            description: clean(&d.description_text),
            comments: clean(&d.comments_text),
            assignee_id: optional_uuid(&d.assignee_id),
            reporter_id: optional_uuid(&d.reporter_id),
            labels: d.labels.iter().map(|l| clean(l)).collect(),
            created_at: d.created_at,
            updated_at: d.updated_at,
            resolved_at: optional(d.resolved_at),
            version: d.version,
            indexed_at: now(),
        })
    }
}

/// Outcome of writing one document.
#[derive(Debug, PartialEq, Eq)]
pub enum Written {
    Indexed,
    /// The index already holds a newer version of this issue.
    Stale,
}

/// What a search may match: always one tenant, optionally narrowed.
#[derive(Debug, Clone, Default)]
pub struct Filters {
    /// `None` = every project of the tenant; otherwise only these (may be empty).
    pub projects: Option<Vec<Uuid>>,
    pub status_category: Option<String>,
    pub assignee_id: Option<Uuid>,
}

#[derive(Debug, Clone)]
pub struct Hit {
    pub doc: IssueDoc,
    /// Matching fragments with [`MARK_START`]/[`MARK_END`] around matched terms.
    pub fragments: Vec<(String, String)>,
    pub sort: Value,
}

#[derive(Debug)]
pub struct Hits {
    pub total: u64,
    /// The real total is at least `total` (counting stops at 1000).
    pub total_is_lower_bound: bool,
    pub hits: Vec<Hit>,
}

#[derive(Clone)]
pub struct Index {
    http: reqwest::Client,
    base: String,
    name: String,
    /// Make writes visible to search before returning (tests).
    pub refresh: bool,
}

fn unavailable(error: impl std::fmt::Display) -> ApiError {
    tracing::warn!(%error, "search index request failed");
    ApiError::DependencyUnavailable("search")
}

impl Index {
    pub fn new(base: &str, name: &str) -> anyhow::Result<Self> {
        let http = reqwest::Client::builder().timeout(std::time::Duration::from_secs(10)).build()?;
        Ok(Self { http, base: base.trim_end_matches('/').to_owned(), name: name.to_owned(), refresh: false })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    fn url(&self, path: &str) -> String {
        format!("{}/{}{}", self.base, self.name, path)
    }

    fn refresh_param(&self) -> &'static str {
        if self.refresh {
            "&refresh=wait_for"
        } else {
            ""
        }
    }

    async fn checked(response: Result<reqwest::Response, reqwest::Error>) -> ApiResult<Value> {
        let response = response.map_err(unavailable)?;
        let status = response.status();
        let body: Value = response.json().await.unwrap_or(Value::Null);
        if status.is_server_error() {
            return Err(unavailable(format!("{status}: {body}")));
        }
        if !status.is_success() {
            return Err(ApiError::internal(anyhow::anyhow!("search index refused a request ({status}): {body}")));
        }
        Ok(body)
    }

    /// Whether the index exists and answers.
    pub async fn ready(&self) -> bool {
        matches!(self.http.head(self.url("")).send().await, Ok(r) if r.status().is_success())
    }

    /// Create the index with its mapping if it does not exist yet.
    pub async fn ensure(&self, shards: u32, replicas: u32) -> ApiResult<()> {
        let head = self.http.head(self.url("")).send().await.map_err(unavailable)?;
        if head.status().is_success() {
            return Ok(());
        }
        let keyword = json!({ "type": "keyword" });
        let date = json!({ "type": "date" });
        let body = json!({
            "settings": {
                "number_of_shards": shards,
                "number_of_replicas": replicas,
                "analysis": { "normalizer": { "lower": { "type": "custom", "filter": ["lowercase"] } } }
            },
            "mappings": {
                "dynamic": "strict",
                "_routing": { "required": true },
                "properties": {
                    "tenant_id": keyword, "issue_id": keyword, "project_id": keyword, "project_key": keyword,
                    "key": { "type": "keyword", "normalizer": "lower" },
                    "type": keyword, "status": keyword, "status_category": keyword, "priority": keyword,
                    "summary": { "type": "search_as_you_type", "analyzer": "english" },
                    "description": { "type": "text", "analyzer": "english" },
                    "comments": { "type": "text", "analyzer": "english" },
                    "assignee_id": keyword, "reporter_id": keyword, "labels": keyword,
                    "created_at": date, "updated_at": date, "resolved_at": date, "indexed_at": date,
                    "version": { "type": "long" }
                }
            }
        });
        let response = self.http.put(self.url("")).json(&body).send().await.map_err(unavailable)?;
        let status = response.status();
        let body: Value = response.json().await.unwrap_or(Value::Null);
        // Another instance may have created it a moment ago.
        if status.is_success() || body["error"]["type"] == "resource_already_exists_exception" {
            Ok(())
        } else {
            Err(unavailable(format!("could not create index ({status}): {body}")))
        }
    }

    fn doc_id(tenant_id: Uuid, issue_id: Uuid) -> String {
        format!("{tenant_id}:{issue_id}")
    }

    /// Write one document unless a newer version of it is already indexed. Comments do
    /// not bump the issue version, so an equal version overwrites (`external_gte`).
    pub async fn put(&self, doc: &IssueDoc) -> ApiResult<Written> {
        let url = self.url(&format!(
            "/_doc/{}?routing={}&version={}&version_type=external_gte{}",
            Self::doc_id(doc.tenant_id, doc.issue_id),
            doc.tenant_id,
            doc.version.max(0),
            self.refresh_param()
        ));
        let response = self.http.put(url).json(doc).send().await.map_err(unavailable)?;
        if response.status() == reqwest::StatusCode::CONFLICT {
            return Ok(Written::Stale);
        }
        Self::checked(Ok(response)).await?;
        Ok(Written::Indexed)
    }

    /// Write many documents of one tenant; returns how many were written.
    pub async fn put_many(&self, docs: &[IssueDoc]) -> ApiResult<usize> {
        if docs.is_empty() {
            return Ok(0);
        }
        let mut body = String::new();
        for doc in docs {
            let action = json!({ "index": {
                "_id": Self::doc_id(doc.tenant_id, doc.issue_id), "routing": doc.tenant_id.to_string(),
                "version": doc.version.max(0), "version_type": "external_gte"
            }});
            body.push_str(&action.to_string());
            body.push('\n');
            body.push_str(&serde_json::to_string(doc).map_err(ApiError::internal)?);
            body.push('\n');
        }
        let url = format!("{}?{}", self.url("/_bulk"), self.refresh_param().trim_start_matches('&'));
        let reply =
            Self::checked(self.http.post(url).header("content-type", "application/x-ndjson").body(body).send().await).await?;
        let mut written = 0;
        for item in reply["items"].as_array().into_iter().flatten() {
            let status = item["index"]["status"].as_u64().unwrap_or(500);
            match status {
                200 | 201 => written += 1,
                409 => {}
                _ => return Err(unavailable(format!("bulk item failed: {}", item["index"]["error"]))),
            }
        }
        Ok(written)
    }

    pub async fn remove(&self, tenant_id: Uuid, issue_id: Uuid) -> ApiResult<()> {
        let url = self.url(&format!("/_doc/{}?routing={tenant_id}{}", Self::doc_id(tenant_id, issue_id), self.refresh_param()));
        let response = self.http.delete(url).send().await.map_err(unavailable)?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(());
        }
        Self::checked(Ok(response)).await.map(|_| ())
    }

    /// Delete the tenant's documents matching `filter` (an OpenSearch filter clause built
    /// by this crate); returns how many were deleted.
    async fn remove_where(&self, tenant_id: Uuid, filter: Value) -> ApiResult<u64> {
        let url = format!(
            "{}?routing={tenant_id}&conflicts=proceed{}",
            self.url("/_delete_by_query"),
            if self.refresh { "&refresh=true" } else { "" }
        );
        let query = json!({ "query": { "bool": { "filter": [ { "term": { "tenant_id": tenant_id } }, filter ] } } });
        let reply = Self::checked(self.http.post(url).json(&query).send().await).await?;
        Ok(reply["deleted"].as_u64().unwrap_or(0))
    }

    pub async fn remove_project(&self, tenant_id: Uuid, project_id: Uuid) -> ApiResult<u64> {
        self.remove_where(tenant_id, json!({ "term": { "project_id": project_id } })).await
    }

    /// Documents not rewritten since `before` (used after a full reindex to drop issues
    /// that no longer exist).
    pub async fn remove_older_than(&self, tenant_id: Uuid, before: &str) -> ApiResult<u64> {
        self.remove_where(tenant_id, json!({ "range": { "indexed_at": { "lt": before } } })).await
    }

    pub async fn search(
        &self,
        tenant_id: Uuid,
        text: &str,
        filters: &Filters,
        limit: u32,
        after: Option<&Value>,
    ) -> ApiResult<Hits> {
        let body = query(tenant_id, text, filters, limit, after);
        let url = format!("{}?routing={tenant_id}", self.url("/_search"));
        let reply = Self::checked(self.http.post(url).json(&body).send().await).await?;
        let total = reply["hits"]["total"]["value"].as_u64().unwrap_or(0);
        let total_is_lower_bound = reply["hits"]["total"]["relation"] == "gte";
        let mut hits = Vec::new();
        for raw in reply["hits"]["hits"].as_array().into_iter().flatten() {
            let Ok(doc) = serde_json::from_value::<IssueDoc>(raw["_source"].clone()) else { continue };
            // Defence in depth: the filter already guarantees this.
            if doc.tenant_id != tenant_id {
                continue;
            }
            let mut fragments = Vec::new();
            for field in ["summary", "description", "comments"] {
                for f in raw["highlight"][field].as_array().into_iter().flatten().filter_map(Value::as_str) {
                    fragments.push((field.to_owned(), f.to_owned()));
                }
            }
            hits.push(Hit { doc, fragments, sort: raw["sort"].clone() });
        }
        Ok(Hits { total, total_is_lower_bound, hits })
    }
}

/// Does `text` look like an issue key (`PAY-12`)?
fn looks_like_key(text: &str) -> bool {
    let Some((project, number)) = text.rsplit_once('-') else { return false };
    !project.is_empty()
        && project.chars().all(|c| c.is_ascii_alphanumeric())
        && !number.is_empty()
        && number.chars().all(|c| c.is_ascii_digit())
}

/// The OpenSearch query for a search. The tenant filter is unconditional.
pub fn query(tenant_id: Uuid, text: &str, filters: &Filters, limit: u32, after: Option<&Value>) -> Value {
    let mut filter = vec![json!({ "term": { "tenant_id": tenant_id } })];
    if let Some(projects) = &filters.projects {
        filter.push(json!({ "terms": { "project_id": projects } }));
    }
    if let Some(category) = &filters.status_category {
        filter.push(json!({ "term": { "status_category": category } }));
    }
    if let Some(assignee) = filters.assignee_id {
        filter.push(json!({ "term": { "assignee_id": assignee } }));
    }
    let text = text.trim();
    let (must, sort) = if text.is_empty() {
        (json!([]), json!([{ "updated_at": "desc" }, { "issue_id": "asc" }]))
    } else {
        let mut should = vec![
            json!({ "multi_match": { "query": text, "type": "bool_prefix", "fields": ["summary^3", "summary._2gram^3", "summary._3gram^3"] } }),
            json!({ "multi_match": { "query": text, "fields": ["summary^3", "description", "comments", "labels^2"], "fuzziness": "AUTO", "prefix_length": 1 } }),
        ];
        if looks_like_key(text) {
            should.push(json!({ "term": { "key": { "value": text, "boost": 50 } } }));
        }
        (
            json!([{ "bool": { "should": should, "minimum_should_match": 1 } }]),
            json!([{ "_score": "desc" }, { "updated_at": "desc" }, { "issue_id": "asc" }]),
        )
    };
    let mut body = json!({
        "size": limit,
        "track_total_hits": 1000,
        "query": { "bool": { "filter": filter, "must": must } },
        "sort": sort,
        "highlight": {
            "pre_tags": [MARK_START.to_string()], "post_tags": [MARK_END.to_string()],
            "encoder": "default", "number_of_fragments": 1, "fragment_size": 160, "no_match_size": 0,
            "fields": { "summary": {}, "description": {}, "comments": {} }
        }
    });
    if let Some(after) = after {
        body["search_after"] = after.clone();
    }
    body
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_query_is_confined_to_the_tenant() {
        let tenant = Uuid::now_v7();
        for text in ["", "refund", "PAY-12"] {
            let q = query(tenant, text, &Filters::default(), 20, None);
            assert_eq!(q["query"]["bool"]["filter"][0], json!({ "term": { "tenant_id": tenant } }));
        }
        let project = Uuid::now_v7();
        let q = query(tenant, "x", &Filters { projects: Some(vec![project]), ..Filters::default() }, 20, None);
        assert_eq!(q["query"]["bool"]["filter"][1], json!({ "terms": { "project_id": [project] } }));
    }

    #[test]
    fn keys_are_recognised_and_text_is_sanitised() {
        assert!(looks_like_key("PAY-12") && looks_like_key("web2-3"));
        assert!(!looks_like_key("PAY-") && !looks_like_key("-12") && !looks_like_key("refunds fail"));
        assert_eq!(clean("a\u{0}b\u{E000}c\nd"), "a b c\nd");
        let long = "é".repeat(MAX_TEXT);
        assert!(clean(&long).len() <= MAX_TEXT);
    }
}
