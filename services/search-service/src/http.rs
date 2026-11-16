//! `/api/v1/search/issues`: full-text issue search across the projects the caller can
//! browse in their current workspace.

use std::collections::HashMap;

use auth_middleware::{OrgContext, OrgPermission, VisibleProjects};
use axum::{
    extract::{Query, State},
    http::{header, Method},
    routing::get,
    Json, Router,
};
use error_handling::{ApiError, ApiResult, FieldError};
use platform_common::pagination::{decode_cursor, encode_cursor};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tower_http::cors::{AllowOrigin, CorsLayer};
use utoipa::{IntoParams, OpenApi, ToSchema};
use uuid::Uuid;

use crate::{
    filters,
    index::{Filters, Hit, Hits, MARK_END, MARK_START},
    mql::{self, MqlError, SortValue},
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
    /// The workflow's name for the status; absent for issues indexed before names were.
    pub status_name: Option<String>,
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
        status_name: (!d.status_name.is_empty()).then_some(d.status_name),
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

    let Some(projects) = visible(&state, &ctx, q.project_id).await? else {
        return Ok(Json(SearchResults::empty()));
    };
    let filters = Filters { projects, status_category: q.status_category, assignee_id };
    let found = state.index.search(ctx.tenant_id, &q.q, &filters, limit, after.as_ref()).await?;
    Ok(Json(SearchResults::from_hits(found, limit)))
}

/// The projects a search may cover: `Some(None)` = all of the tenant's, `Some(Some(ids))`
/// = only these, `None` = nothing at all. Never wider than what the caller may browse; a
/// project they cannot see simply has no results (indistinguishable from an empty one).
pub(crate) async fn visible(state: &AppState, ctx: &OrgContext, project: Option<Uuid>) -> ApiResult<Option<Option<Vec<Uuid>>>> {
    let projects = match (state.policy.snapshot(ctx).await?.visible_projects(ctx), project) {
        (VisibleProjects::All, None) => None,
        (VisibleProjects::All, Some(p)) => Some(vec![p]),
        (VisibleProjects::Only(ids), None) => Some(ids),
        (VisibleProjects::Only(ids), Some(p)) => Some(if ids.contains(&p) { vec![p] } else { vec![] }),
    };
    Ok(if projects.as_ref().is_some_and(Vec::is_empty) { None } else { Some(projects) })
}

impl SearchResults {
    fn empty() -> Self {
        Self { items: vec![], total: 0, total_is_lower_bound: false, next_cursor: None }
    }

    fn from_hits(found: Hits, limit: u32) -> Self {
        let next_cursor = (found.hits.len() as u32 == limit).then(|| found.hits.last().map(|h| encode_cursor(&h.sort))).flatten();
        Self {
            items: found.hits.into_iter().map(hit).collect(),
            total: found.total,
            total_is_lower_bound: found.total_is_lower_bound,
            next_cursor,
        }
    }
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct MqlQuery {
    /// An MQL query, e.g. `project = PAY AND assignee = currentUser() ORDER BY updated DESC`.
    /// Empty lists recently updated issues.
    #[serde(default)]
    pub mql: String,
    pub limit: Option<u32>,
    pub cursor: Option<String>,
}

fn mql_error(e: MqlError) -> ApiError {
    ApiError::Validation(vec![FieldError::new("mql", "invalid_query", e.message).at(e.start, e.end)])
}

/// Parse and compile MQL for the caller. Naming people by email or name needs the member
/// directory, which guests cannot see.
pub(crate) async fn compile(state: &AppState, ctx: &OrgContext, text: &str) -> ApiResult<mql::Compiled> {
    let parsed = mql::parse(text).map_err(mql_error)?;
    let people = if ctx.require(OrgPermission::ViewMembers).is_ok() {
        let terms = mql::person_terms(&parsed);
        Some(if terms.is_empty() { HashMap::new() } else { state.people.find(ctx.tenant_id, &terms).await? })
    } else {
        None
    };
    mql::compile(&parsed, &mql::Context { current_user: ctx.user_id, people: people.as_ref() }).map_err(mql_error)
}

/// Most issue ids returned for one project; a board shows at most this many issues.
pub const MAX_IDS: u32 = 5000;

#[derive(Debug, Deserialize, IntoParams)]
pub struct IdsQuery {
    /// An MQL query; `ORDER BY` is ignored.
    pub mql: String,
    /// The project the board or backlog belongs to.
    pub project: Uuid,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct MatchingIds {
    /// Issues of the project that match and that the caller can see, in no particular order.
    pub ids: Vec<Uuid>,
    /// More than `MAX_IDS` issues match; only that many are listed.
    pub truncated: bool,
}

#[utoipa::path(get, path = "/api/v1/search/issues/ids", params(IdsQuery), security(("bearer" = [])),
    responses((status = 200, body = MatchingIds),
        (status = 422, description = "The query is invalid; `errors[0].span` is the [start, end) character range at fault"),
        (status = 503)))]
pub async fn matching_ids(
    State(state): State<AppState>,
    ctx: OrgContext,
    Query(q): Query<IdsQuery>,
) -> ApiResult<Json<MatchingIds>> {
    let compiled = compile(&state, &ctx, &q.mql).await?;
    let Some(projects) = visible(&state, &ctx, Some(q.project)).await? else {
        return Ok(Json(MatchingIds { ids: vec![], truncated: false }));
    };
    let filters = Filters { projects, ..Filters::default() };
    let (ids, truncated) = state.index.matching_ids(ctx.tenant_id, &compiled, &filters, MAX_IDS).await?;
    Ok(Json(MatchingIds { ids, truncated }))
}

#[utoipa::path(get, path = "/api/v1/search/issues/query", params(MqlQuery), security(("bearer" = [])),
    responses((status = 200, body = SearchResults),
        (status = 422, description = "The query is invalid; `errors[0].span` is the [start, end) character range at fault"),
        (status = 503)))]
pub async fn query_issues(
    State(state): State<AppState>,
    ctx: OrgContext,
    Query(q): Query<MqlQuery>,
) -> ApiResult<Json<SearchResults>> {
    let compiled = compile(&state, &ctx, &q.mql).await?;

    let invalid_cursor = || ApiError::invalid("cursor", "invalid_cursor", "Not a valid cursor for this query.");
    let after: Option<Value> = match q.cursor.as_deref() {
        Some(raw) => {
            let v: Value = decode_cursor(raw).map_err(|_| invalid_cursor())?;
            // Exactly the sort values this query produces, each of the right kind.
            let ok = v.as_array().is_some_and(|a| {
                a.len() == compiled.sort_values.len()
                    && a.iter().zip(&compiled.sort_values).all(|(x, kind)| match kind {
                        SortValue::Number => x.is_number(),
                        SortValue::Text => x.is_string() || x.is_null(),
                    })
            });
            if !ok {
                return Err(invalid_cursor());
            }
            Some(v)
        }
        None => None,
    };
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let Some(projects) = visible(&state, &ctx, None).await? else {
        return Ok(Json(SearchResults::empty()));
    };
    let filters = Filters { projects, ..Filters::default() };
    let found = state.index.search_compiled(ctx.tenant_id, &compiled, &filters, limit, after.as_ref()).await?;
    Ok(Json(SearchResults::from_hits(found, limit)))
}

#[derive(OpenApi)]
#[openapi(
    info(
        title = "Search API",
        version = "1.0.0",
        description = "Full-text and MQL search over the issues the caller can browse."
    ),
    paths(
        search_issues,
        query_issues,
        crate::subscriptions::get_subscription,
        crate::subscriptions::put_subscription,
        crate::subscriptions::delete_subscription,
        matching_ids,
        filters::list,
        filters::create,
        filters::get_one,
        filters::update,
        filters::delete
    ),
    components(schemas(
        SearchResults,
        MatchingIds,
        crate::subscriptions::Subscription,
        crate::subscriptions::PutSubscription,
        crate::subscriptions::Frequency,
        SearchHit,
        Snippet,
        TextPart,
        filters::SavedFilter,
        filters::SavedFilterList,
        filters::CreateSavedFilter,
        filters::UpdateSavedFilter
    ))
)]
pub struct ApiDoc;

pub fn router(state: AppState) -> Router {
    let api = Router::new()
        .route("/search/issues", get(search_issues))
        .route("/search/issues/query", get(query_issues))
        .route("/search/issues/ids", get(matching_ids))
        .route("/search/filters", get(filters::list).post(filters::create))
        .route("/search/filters/{id}", get(filters::get_one).patch(filters::update).delete(filters::delete))
        .route(
            "/search/filters/{id}/subscription",
            get(crate::subscriptions::get_subscription)
                .put(crate::subscriptions::put_subscription)
                .delete(crate::subscriptions::delete_subscription),
        );
    let origins: Vec<_> = state.allowed_origins.iter().filter_map(|o| o.parse().ok()).collect();
    let cors = CorsLayer::new()
        .allow_origin(AllowOrigin::list(origins))
        .allow_methods([Method::GET, Method::POST, Method::PUT, Method::PATCH, Method::DELETE])
        .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE]);
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
