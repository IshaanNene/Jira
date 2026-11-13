//! `/api/v1/search/issues`: full-text issue search across the projects the caller can
//! browse in their current workspace.

use auth_middleware::{OrgContext, VisibleProjects};
use axum::{
    extract::{Query, State},
    http::{header, Method},
    routing::get,
    Json, Router,
};
use error_handling::{ApiError, ApiResult};
use platform_common::pagination::{decode_cursor, encode_cursor};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tower_http::cors::{AllowOrigin, CorsLayer};
use utoipa::{IntoParams, OpenApi, ToSchema};
use uuid::Uuid;

use crate::{
    index::{Filters, Hit, MARK_END, MARK_START},
    AppState,
};

const DEFAULT_LIMIT: u32 = 20;
const MAX_LIMIT: u32 = 50;
const MAX_QUERY_CHARS: usize = 200;

/// A run of text, `matched` when it matched the query.
#[derive(Debug, Serialize, ToSchema, PartialEq, Eq)]
pub struct TextPart {
    pub text: String,
    pub matched: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct Snippet {
    /// description | comments
    pub field: String,
    pub parts: Vec<TextPart>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SearchHit {
    pub id: Uuid,
    pub key: String,
    pub project_id: Uuid,
    pub project_key: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub status: String,
    pub status_category: String,
    pub priority: String,
    pub summary: String,
    /// The summary split into matched and unmatched runs, when it matched.
    pub summary_parts: Option<Vec<TextPart>>,
    /// Where else the query matched.
    pub snippet: Option<Snippet>,
    pub assignee_id: Option<Uuid>,
    pub updated_at: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SearchResults {
    pub items: Vec<SearchHit>,
    pub total: u64,
    /// At least `total` matched (counting stops at 1000).
    pub total_is_lower_bound: bool,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct SearchQuery {
    /// Words to look for in key, summary, description, comments and labels. Empty lists
    /// recently updated issues.
    #[serde(default)]
    pub q: String,
    pub project_id: Option<Uuid>,
    /// todo | in_progress | done
    pub status_category: Option<String>,
    /// A user id, or `me`.
    pub assignee: Option<String>,
    pub limit: Option<u32>,
    pub cursor: Option<String>,
}

/// Split a highlighted fragment into runs. Markers are private-use characters that the
/// indexer strips from content, so nothing in a document can forge a match.
pub fn parts(fragment: &str) -> Vec<TextPart> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut matched = false;
    for c in fragment.chars() {
        if c == MARK_START || c == MARK_END {
            if !current.is_empty() {
                out.push(TextPart { text: std::mem::take(&mut current), matched });
            }
            matched = c == MARK_START;
        } else {
            current.push(c);
        }
    }
    if !current.is_empty() {
        out.push(TextPart { text: current, matched });
    }
    // Adjacent runs of the same kind (e.g. two matched words) read as one.
    out.dedup_by(|b, a| {
        if a.matched == b.matched {
            a.text.push_str(&b.text);
            true
        } else {
            false
        }
    });
    out
}

fn hit(h: Hit) -> SearchHit {
    let mut summary_parts = None;
    let mut snippet = None;
    for (field, fragment) in h.fragments {
        if field == "summary" {
            summary_parts.get_or_insert_with(|| parts(&fragment));
        } else if snippet.is_none() {
            snippet = Some(Snippet { field, parts: parts(&fragment) });
        }
    }
    let d = h.doc;
    SearchHit {
        id: d.issue_id,
        key: d.key,
        project_id: d.project_id,
        project_key: d.project_key,
        kind: d.kind,
        status: d.status,
        status_category: d.status_category,
        priority: d.priority,
        summary: d.summary,
        summary_parts,
        snippet,
        assignee_id: d.assignee_id,
        updated_at: d.updated_at,
    }
}

#[utoipa::path(get, path = "/api/v1/search/issues", params(SearchQuery), security(("bearer" = [])),
    responses((status = 200, body = SearchResults), (status = 422), (status = 503)))]
pub async fn search_issues(
    State(state): State<AppState>,
    ctx: OrgContext,
    Query(q): Query<SearchQuery>,
) -> ApiResult<Json<SearchResults>> {
    if q.q.chars().count() > MAX_QUERY_CHARS {
        return Err(ApiError::invalid("q", "too_long", "Search for at most 200 characters."));
    }
    if let Some(c) = &q.status_category {
        if !["todo", "in_progress", "done"].contains(&c.as_str()) {
            return Err(ApiError::invalid("status_category", "invalid", "Use todo, in_progress or done."));
        }
    }
    let assignee_id = match q.assignee.as_deref() {
        None | Some("") => None,
        Some("me") => Some(ctx.user_id),
        Some(raw) => Some(Uuid::parse_str(raw).map_err(|_| ApiError::invalid("assignee", "invalid", "Not a user id."))?),
    };
    let invalid_cursor = || ApiError::invalid("cursor", "invalid_cursor", "Not a valid cursor.");
    let after: Option<Value> = match q.cursor.as_deref() {
        Some(raw) => {
            let v: Value = decode_cursor(raw).map_err(|_| invalid_cursor())?;
            // Only the sort values this endpoint hands out: a short array of scalars.
            let ok = v.as_array().is_some_and(|a| (2..=3).contains(&a.len()) && a.iter().all(|x| x.is_number() || x.is_string()));
            if !ok {
                return Err(invalid_cursor());
            }
            Some(v)
        }
        None => None,
    };
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);

    // Never wider than what the caller may browse; a project they cannot see simply has
    // no results (indistinguishable from an empty one).
    let projects = match (state.policy.snapshot(&ctx).await?.visible_projects(&ctx), q.project_id) {
        (VisibleProjects::All, None) => None,
        (VisibleProjects::All, Some(p)) => Some(vec![p]),
        (VisibleProjects::Only(ids), None) => Some(ids),
        (VisibleProjects::Only(ids), Some(p)) => Some(if ids.contains(&p) { vec![p] } else { vec![] }),
    };
    if projects.as_ref().is_some_and(Vec::is_empty) {
        return Ok(Json(SearchResults { items: vec![], total: 0, total_is_lower_bound: false, next_cursor: None }));
    }
    let filters = Filters { projects, status_category: q.status_category, assignee_id };
    let found = state.index.search(ctx.tenant_id, &q.q, &filters, limit, after.as_ref()).await?;
    let next_cursor = (found.hits.len() as u32 == limit).then(|| found.hits.last().map(|h| encode_cursor(&h.sort))).flatten();
    Ok(Json(SearchResults {
        items: found.hits.into_iter().map(hit).collect(),
        total: found.total,
        total_is_lower_bound: found.total_is_lower_bound,
        next_cursor,
    }))
}

#[derive(OpenApi)]
#[openapi(
    info(title = "Search API", version = "1.0.0", description = "Full-text search over the issues the caller can browse."),
    paths(search_issues),
    components(schemas(SearchResults, SearchHit, Snippet, TextPart))
)]
pub struct ApiDoc;

pub fn router(state: AppState) -> Router {
    let api = Router::new().route("/search/issues", get(search_issues));
    let origins: Vec<_> = state.allowed_origins.iter().filter_map(|o| o.parse().ok()).collect();
    let cors = CorsLayer::new()
        .allow_origin(AllowOrigin::list(origins))
        .allow_methods([Method::GET])
        .allow_headers([header::AUTHORIZATION]);
    Router::new()
        .nest("/api/v1", api)
        .route("/openapi.json", get(|| async { Json(ApiDoc::openapi()) }))
        .layer(cors)
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fragments_split_into_matched_runs() {
        let f = format!("Refunds {MARK_START}fail{MARK_END} {MARK_START}twice{MARK_END} <b>x</b>");
        let p = parts(&f);
        assert_eq!(
            p,
            vec![
                TextPart { text: "Refunds ".into(), matched: false },
                TextPart { text: "fail".into(), matched: true },
                TextPart { text: " ".into(), matched: false },
                TextPart { text: "twice".into(), matched: true },
                TextPart { text: " <b>x</b>".into(), matched: false },
            ]
        );
    }
}
