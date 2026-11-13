//! `/organizations/current/audit`: the workspace audit log, for admins and owners.

use std::collections::{BTreeMap, HashMap};

use auth_middleware::{OrgContext, OrgPermission};
use axum::{
    extract::{Query, State},
    http::{header, HeaderValue},
    response::{IntoResponse, Response},
    Json,
};
use error_handling::{ApiError, ApiResult};
use platform_common::pagination::{decode_cursor, encode_cursor};
use serde::{Deserialize, Serialize};
use sqlx::PgConnection;
use time::{macros::format_description, Date, OffsetDateTime};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::{
    audit::{self, Row, ROW_COLUMNS},
    AppState,
};

const DEFAULT_LIMIT: i64 = 50;
const MAX_LIMIT: i64 = 200;
const EXPORT_LIMIT: i64 = 50_000;

/// Groups of actions that can be filtered on (the part before the dot).
pub const CATEGORIES: &[&str] =
    &["organization", "member", "project", "version", "workflow", "board", "sprint", "issue", "comment"];

#[derive(Debug, Serialize, ToSchema)]
pub struct AuditActor {
    /// user | system | automation
    #[serde(rename = "type")]
    pub kind: String,
    pub id: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AuditEntryDto {
    pub seq: i64,
    pub id: Uuid,
    #[serde(with = "time::serde::rfc3339")]
    pub occurred_at: OffsetDateTime,
    pub actor: AuditActor,
    /// e.g. `member.role_changed`, `project.access_granted`
    pub action: String,
    pub resource_type: String,
    pub resource_id: String,
    pub resource_label: String,
    pub details: BTreeMap<String, String>,
    pub request_id: Option<String>,
}

impl From<Row> for AuditEntryDto {
    fn from(r: Row) -> Self {
        Self {
            seq: r.seq,
            id: r.id,
            occurred_at: r.occurred_at,
            actor: AuditActor { kind: r.actor_type, id: r.actor_id },
            action: r.action,
            resource_type: r.resource_type,
            resource_id: r.resource_id,
            resource_label: r.resource_label,
            details: r.details.0,
            request_id: r.request_id,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AuditPage {
    pub items: Vec<AuditEntryDto>,
    /// Display names of the people the entries mention (actors, members, grantees).
    pub people: HashMap<String, String>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AuditVerification {
    /// Entries checked and found intact, from the first.
    pub entries: i64,
    pub intact: bool,
    /// Sequence number of the first entry that does not fit the chain.
    pub broken_at: Option<i64>,
    pub reason: Option<String>,
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct AuditQuery {
    /// organization | member | project | version | workflow | board | sprint | issue | comment
    pub category: Option<String>,
    /// Only actions by this person (user id).
    pub actor: Option<Uuid>,
    /// First day, `YYYY-MM-DD` (UTC).
    pub from: Option<String>,
    /// Last day, inclusive.
    pub to: Option<String>,
    pub limit: Option<i64>,
    pub cursor: Option<String>,
}

struct Filters {
    category: Option<String>,
    actor: Option<String>,
    from: Option<OffsetDateTime>,
    until: Option<OffsetDateTime>,
}

fn day(field: &str, raw: Option<&str>) -> ApiResult<Option<Date>> {
    raw.filter(|s| !s.is_empty())
        .map(|s| {
            Date::parse(s, format_description!("[year]-[month]-[day]"))
                .map_err(|_| ApiError::invalid(field, "invalid_date", "Use a date like 2026-11-13."))
        })
        .transpose()
}

impl Filters {
    fn from_query(q: &AuditQuery) -> ApiResult<Self> {
        if let Some(c) = &q.category {
            if !CATEGORIES.contains(&c.as_str()) {
                return Err(ApiError::invalid("category", "invalid", "Not a category of the audit log."));
            }
        }
        let from = day("from", q.from.as_deref())?.map(|d| d.midnight().assume_utc());
        let until = day("to", q.to.as_deref())?.and_then(|d| d.next_day()).map(|d| d.midnight().assume_utc());
        Ok(Self { category: q.category.clone(), actor: q.actor.map(|a| a.to_string()), from, until })
    }
}

async fn rows(conn: &mut PgConnection, tenant_id: Uuid, f: &Filters, before_seq: Option<i64>, limit: i64) -> ApiResult<Vec<Row>> {
    // The category is one of a fixed list, so it is safe in a LIKE pattern; it is still bound.
    Ok(sqlx::query_as::<_, Row>(&format!(
        "SELECT {ROW_COLUMNS} FROM audit_log WHERE tenant_id = $1 \
           AND ($2::text IS NULL OR action LIKE $2 || '.%') \
           AND ($3::text IS NULL OR actor_id = $3) \
           AND ($4::timestamptz IS NULL OR occurred_at >= $4) \
           AND ($5::timestamptz IS NULL OR occurred_at < $5) \
           AND ($6::bigint IS NULL OR seq < $6) \
         ORDER BY seq DESC LIMIT $7"
    ))
    .bind(tenant_id)
    .bind(&f.category)
    .bind(&f.actor)
    .bind(f.from)
    .bind(f.until)
    .bind(before_seq)
    .bind(limit)
    .fetch_all(conn)
    .await?)
}

/// Names of the people the rows mention.
async fn people(conn: &mut PgConnection, rows: &[Row]) -> ApiResult<HashMap<String, String>> {
    let mut ids: Vec<Uuid> = Vec::new();
    for r in rows {
        if r.actor_type == "user" {
            ids.extend(Uuid::parse_str(&r.actor_id).ok());
        }
        if r.resource_type == "user" {
            ids.extend(Uuid::parse_str(&r.resource_id).ok());
        }
        ids.extend(r.details.0.get("user_id").and_then(|v| Uuid::parse_str(v).ok()));
    }
    ids.sort();
    ids.dedup();
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let found: Vec<(Uuid, String)> = sqlx::query_as("SELECT user_id, display_name FROM user_profiles WHERE user_id = ANY($1)")
        .bind(&ids)
        .fetch_all(conn)
        .await?;
    Ok(found.into_iter().map(|(id, name)| (id.to_string(), name)).collect())
}

#[utoipa::path(get, path = "/api/v1/organizations/current/audit", params(AuditQuery), security(("bearer" = [])),
    responses((status = 200, body = AuditPage), (status = 403), (status = 422)))]
pub async fn list(State(state): State<AppState>, ctx: OrgContext, Query(q): Query<AuditQuery>) -> ApiResult<Json<AuditPage>> {
    ctx.require(OrgPermission::ViewAuditLog)?;
    let filters = Filters::from_query(&q)?;
    let before = q
        .cursor
        .as_deref()
        .map(decode_cursor::<i64>)
        .transpose()
        .map_err(|_| ApiError::invalid("cursor", "invalid_cursor", "The cursor is not valid."))?;
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);

    let mut tx = state.db.begin(ctx.tenant_id).await?;
    let mut found = rows(tx.conn(), ctx.tenant_id, &filters, before, limit + 1).await?;
    let more = found.len() as i64 > limit;
    found.truncate(limit as usize);
    let people = people(tx.conn(), &found).await?;
    tx.commit().await?;
    let next_cursor = more.then(|| found.last().map(|r| encode_cursor(&r.seq))).flatten();
    Ok(Json(AuditPage { items: found.into_iter().map(AuditEntryDto::from).collect(), people, next_cursor }))
}

/// A spreadsheet cell: quoted, and never starting with a character a spreadsheet would
/// treat as a formula.
fn cell(value: &str) -> String {
    let safe = if value.starts_with(['=', '+', '-', '@', '\t', '\r']) { format!("'{value}") } else { value.to_owned() };
    format!("\"{}\"", safe.replace('"', "\"\""))
}

#[utoipa::path(get, path = "/api/v1/organizations/current/audit/export", params(AuditQuery), security(("bearer" = [])),
    responses((status = 200, description = "CSV of the matching entries, newest first (at most 50 000)", content_type = "text/csv"),
        (status = 403), (status = 422)))]
pub async fn export(State(state): State<AppState>, ctx: OrgContext, Query(q): Query<AuditQuery>) -> ApiResult<Response> {
    ctx.require(OrgPermission::ViewAuditLog)?;
    let filters = Filters::from_query(&q)?;
    let mut tx = state.db.begin(ctx.tenant_id).await?;
    let found = rows(tx.conn(), ctx.tenant_id, &filters, None, EXPORT_LIMIT).await?;
    let names = people(tx.conn(), &found).await?;
    tx.commit().await?;

    let mut csv = String::from(
        "seq,occurred_at,actor_type,actor_id,actor_name,action,resource_type,resource_id,resource_label,details,request_id,hash\r\n",
    );
    for r in &found {
        let at = r.occurred_at.format(&time::format_description::well_known::Rfc3339).unwrap_or_default();
        let actor_name = names.get(&r.actor_id).map(String::as_str).unwrap_or("");
        let details = serde_json::to_string(&r.details.0).map_err(ApiError::internal)?;
        let fields = [
            r.seq.to_string(),
            at,
            r.actor_type.clone(),
            r.actor_id.clone(),
            actor_name.to_owned(),
            r.action.clone(),
            r.resource_type.clone(),
            r.resource_id.clone(),
            r.resource_label.clone(),
            details,
            r.request_id.clone().unwrap_or_default(),
            hex::encode(&r.hash),
        ];
        csv.push_str(&fields.iter().map(|f| cell(f)).collect::<Vec<_>>().join(","));
        csv.push_str("\r\n");
    }
    let today = OffsetDateTime::now_utc().date();
    let mut response = csv.into_response();
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/csv; charset=utf-8"));
    if let Ok(v) = HeaderValue::from_str(&format!("attachment; filename=\"audit-log-{today}.csv\"")) {
        headers.insert(header::CONTENT_DISPOSITION, v);
    }
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

#[utoipa::path(get, path = "/api/v1/organizations/current/audit/verify", security(("bearer" = [])),
    responses((status = 200, body = AuditVerification), (status = 403)))]
pub async fn verify(State(state): State<AppState>, ctx: OrgContext) -> ApiResult<Json<AuditVerification>> {
    ctx.require(OrgPermission::ViewAuditLog)?;
    let mut tx = state.db.begin(ctx.tenant_id).await?;
    let result = audit::verify(tx.conn(), ctx.tenant_id).await?;
    tx.commit().await?;
    Ok(Json(AuditVerification {
        entries: result.entries,
        intact: result.broken.is_none(),
        broken_at: result.broken.map(|(seq, _)| seq),
        reason: result.broken.map(|(_, why)| why.to_owned()),
    }))
}

#[cfg(test)]
mod tests {
    use super::cell;

    #[test]
    fn csv_cells_are_quoted_and_cannot_be_formulas() {
        assert_eq!(cell("plain"), "\"plain\"");
        assert_eq!(cell("say \"hi\""), "\"say \"\"hi\"\"\"");
        assert_eq!(cell("=HYPERLINK(\"x\")"), "\"'=HYPERLINK(\"\"x\"\")\"");
        assert_eq!(cell("-1"), "\"'-1\"");
    }
}
