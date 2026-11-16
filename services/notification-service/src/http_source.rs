//! Source control over HTTP: workspace administrators connect GitHub repositories
//! (`/api/v1/source-connections`), GitHub delivers to `/hooks/github/{id}`, and anyone who can
//! see an issue reads what was linked to it (`/api/v1/development?issue=KEY`).

use auth_middleware::{OrgContext, OrgPermission, ProjectPermission};
use axum::{
    body::Bytes,
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{HeaderMap, StatusCode},
    routing::{delete, get, post},
    Json, Router,
};
use error_handling::{ApiError, ApiResult};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::{
    source::{self, Connection, MAX_BODY, MAX_CONNECTIONS, MAX_LINKS_SHOWN, MAX_PROJECTS},
    AppState,
};

const NOT_FOUND: ApiError = ApiError::NotFound("connection");

#[derive(Debug, Serialize, ToSchema)]
pub struct SourceConnectionDto {
    pub id: Uuid,
    /// `github`.
    pub provider: String,
    pub name: String,
    pub project_keys: Vec<String>,
    /// Where the provider delivers, relative to the API's address: `/hooks/github/{id}`.
    pub payload_path: String,
    #[serde(with = "time::serde::rfc3339::option")]
    #[schema(value_type = Option<String>, format = DateTime)]
    pub last_event_at: Option<OffsetDateTime>,
    pub created_by: Uuid,
    #[serde(with = "time::serde::rfc3339")]
    #[schema(value_type = String, format = DateTime)]
    pub created_at: OffsetDateTime,
}

impl From<Connection> for SourceConnectionDto {
    fn from(c: Connection) -> Self {
        Self {
            payload_path: format!("/hooks/{}/{}", c.provider, c.id),
            id: c.id,
            provider: c.provider,
            name: c.name,
            project_keys: c.project_keys,
            last_event_at: c.last_event_at,
            created_by: c.created_by,
            created_at: c.created_at,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SourceConnectionList {
    pub items: Vec<SourceConnectionDto>,
}

/// Returned once, when a connection is made or its secret replaced.
#[derive(Debug, Serialize, ToSchema)]
pub struct SourceConnectionWithSecret {
    pub connection: SourceConnectionDto,
    /// Entered as the webhook's secret at the provider. Not shown again.
    pub secret: String,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateSourceConnectionRequest {
    /// `github`.
    pub provider: String,
    pub name: String,
    /// Keys of the projects whose issues commits, branches and pull requests may name.
    pub project_keys: Vec<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct DevLinkDto {
    /// `commit`, `branch` or `pull_request`.
    pub kind: String,
    pub repository: String,
    /// Commit SHA, branch name or pull request number.
    #[serde(rename = "ref")]
    pub reference: String,
    pub title: String,
    pub url: String,
    /// Pull requests: `open`, `closed` or `merged`.
    pub state: Option<String>,
    pub author: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    #[schema(value_type = String, format = DateTime)]
    pub occurred_at: OffsetDateTime,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct Development {
    /// Newest first, at most 100.
    pub items: Vec<DevLinkDto>,
    /// Whether any source-control connection covers the issue's project.
    pub connected: bool,
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct DevelopmentQuery {
    /// Issue key, e.g. `PAY-12`.
    pub issue: String,
}

fn admin(ctx: &OrgContext) -> ApiResult<()> {
    ctx.require(OrgPermission::ManageOrganization)?;
    Ok(())
}

fn conn_name(raw: &str) -> ApiResult<String> {
    let n = raw.trim();
    if !(1..=100).contains(&n.chars().count()) {
        return Err(ApiError::invalid("name", "length", "Give the connection a name of up to 100 characters."));
    }
    Ok(n.to_owned())
}

#[utoipa::path(get, path = "/api/v1/source-connections", security(("bearer" = [])),
    responses((status = 200, body = SourceConnectionList), (status = 403)))]
pub async fn list(State(state): State<AppState>, ctx: OrgContext) -> ApiResult<Json<SourceConnectionList>> {
    admin(&ctx)?;
    let mut tx = state.db.begin(ctx.tenant_id).await?;
    let items = source::connections(tx.conn(), ctx.tenant_id).await?;
    tx.commit().await?;
    Ok(Json(SourceConnectionList { items: items.into_iter().map(Into::into).collect() }))
}

#[utoipa::path(post, path = "/api/v1/source-connections", request_body = CreateSourceConnectionRequest, security(("bearer" = [])),
    responses((status = 201, body = SourceConnectionWithSecret), (status = 403), (status = 409, description = "Too many connections"), (status = 422)))]
pub async fn create(
    State(state): State<AppState>,
    ctx: OrgContext,
    Json(body): Json<CreateSourceConnectionRequest>,
) -> ApiResult<(StatusCode, Json<SourceConnectionWithSecret>)> {
    admin(&ctx)?;
    if body.provider != "github" {
        return Err(ApiError::invalid("provider", "unsupported", "Only GitHub can be connected."));
    }
    let name = conn_name(&body.name)?;
    let mut keys: Vec<String> = body.project_keys.iter().map(|k| k.trim().to_ascii_uppercase()).collect();
    keys.sort();
    keys.dedup();
    if keys.is_empty() || keys.len() > MAX_PROJECTS {
        return Err(ApiError::invalid("project_keys", "count", "Choose between 1 and 50 projects."));
    }
    // The ids come from project-service, never from the request.
    let mut ids = Vec::with_capacity(keys.len());
    for key in &keys {
        let valid = (2..=10).contains(&key.len()) && key.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit());
        let id = if valid { state.projects.id_of(ctx.tenant_id, key).await? } else { None };
        ids.push(
            id.ok_or_else(|| ApiError::invalid("project_keys", "unknown_project", &format!("No project has the key {key}.")))?,
        );
    }
    let secret = source::new_secret();
    let id = platform_common::new_id();
    // The route first: a delivery can only reach a connection that has one.
    sqlx::query("INSERT INTO source_hook_routes (connection_id, tenant_id) VALUES ($1, $2)")
        .bind(id)
        .bind(ctx.tenant_id)
        .execute(state.db.global()?)
        .await?;
    let mut tx = state.db.begin(ctx.tenant_id).await?;
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM source_connections WHERE tenant_id = $1")
        .bind(ctx.tenant_id)
        .fetch_one(tx.conn())
        .await?;
    if count >= MAX_CONNECTIONS {
        drop(tx);
        drop_route(&state, id).await;
        return Err(ApiError::Conflict(format!("A workspace can have at most {MAX_CONNECTIONS} source-control connections.")));
    }
    let connection: Connection = sqlx::query_as(&format!(
        "INSERT INTO source_connections (id, tenant_id, provider, name, project_ids, project_keys, secret_sealed, created_by) \
         VALUES ($1, $2, 'github', $3, $4, $5, $6, $7) RETURNING {}",
        source::CONNECTION
    ))
    .bind(id)
    .bind(ctx.tenant_id)
    .bind(&name)
    .bind(&ids)
    .bind(&keys)
    .bind(state.webhooks.secrets.seal(&secret))
    .bind(ctx.user_id)
    .fetch_one(tx.conn())
    .await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(SourceConnectionWithSecret { connection: connection.into(), secret })))
}

async fn drop_route(state: &AppState, id: Uuid) {
    if let Ok(pool) = state.db.global() {
        if let Err(error) = sqlx::query("DELETE FROM source_hook_routes WHERE connection_id = $1").bind(id).execute(pool).await {
            tracing::warn!(%error, "could not remove a source hook route");
        }
    }
}

/// Removes the connection and every link it made.
#[utoipa::path(delete, path = "/api/v1/source-connections/{id}", security(("bearer" = [])), params(("id" = Uuid, Path)),
    responses((status = 204), (status = 404)))]
pub async fn remove(State(state): State<AppState>, ctx: OrgContext, Path(id): Path<Uuid>) -> ApiResult<StatusCode> {
    admin(&ctx)?;
    let mut tx = state.db.begin(ctx.tenant_id).await?;
    let gone = sqlx::query("DELETE FROM source_connections WHERE tenant_id = $1 AND id = $2")
        .bind(ctx.tenant_id)
        .bind(id)
        .execute(tx.conn())
        .await?
        .rows_affected();
    tx.commit().await?;
    if gone == 0 {
        return Err(NOT_FOUND);
    }
    drop_route(&state, id).await;
    Ok(StatusCode::NO_CONTENT)
}

/// Replace the secret; deliveries signed with the old one are refused at once.
#[utoipa::path(post, path = "/api/v1/source-connections/{id}/secret", security(("bearer" = [])), params(("id" = Uuid, Path)),
    responses((status = 200, body = SourceConnectionWithSecret), (status = 404)))]
pub async fn rotate(
    State(state): State<AppState>,
    ctx: OrgContext,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<SourceConnectionWithSecret>> {
    admin(&ctx)?;
    let secret = source::new_secret();
    let mut tx = state.db.begin(ctx.tenant_id).await?;
    let connection: Option<Connection> = sqlx::query_as(&format!(
        "UPDATE source_connections SET secret_sealed = $3 WHERE tenant_id = $1 AND id = $2 RETURNING {}",
        source::CONNECTION
    ))
    .bind(ctx.tenant_id)
    .bind(id)
    .bind(state.webhooks.secrets.seal(&secret))
    .fetch_optional(tx.conn())
    .await?;
    tx.commit().await?;
    Ok(Json(SourceConnectionWithSecret { connection: connection.ok_or(NOT_FOUND)?.into(), secret }))
}

#[derive(Debug, Serialize, ToSchema)]
pub struct HookReceipt {
    /// What was done: `pong`, `linked`, `ignored`.
    pub result: String,
    /// Links saved or refreshed.
    pub links: usize,
}

/// Public: GitHub's webhook deliveries. The connection id finds the workspace; the delivery
/// counts only if `X-Hub-Signature-256` matches the connection's secret.
#[utoipa::path(post, path = "/hooks/github/{id}", params(("id" = Uuid, Path)),
    request_body(content = String, content_type = "application/json", description = "GitHub's event payload, as GitHub signed it"),
    responses((status = 200, body = HookReceipt), (status = 400), (status = 401, description = "Bad or missing signature"), (status = 404), (status = 413)))]
pub async fn github_hook(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Json<HookReceipt>> {
    let tenant_id: Option<Uuid> = sqlx::query_scalar("SELECT tenant_id FROM source_hook_routes WHERE connection_id = $1")
        .bind(id)
        .fetch_optional(state.db.global()?)
        .await?;
    let tenant_id = tenant_id.ok_or(NOT_FOUND)?;
    let mut tx = state.db.begin(tenant_id).await?;
    let connection = source::connection(tx.conn(), tenant_id, id).await?.ok_or(NOT_FOUND)?;
    let secret = state
        .webhooks
        .secrets
        .open(&connection.secret_sealed)
        .ok_or_else(|| ApiError::internal(anyhow::anyhow!("a connection secret does not open")))?;
    let signature = headers.get("x-hub-signature-256").and_then(|v| v.to_str().ok());
    if !source::verify(&secret, &body, signature) {
        return Err(ApiError::Unauthenticated("The delivery's signature does not match this connection's secret."));
    }
    let event = headers.get("x-github-event").and_then(|v| v.to_str().ok()).unwrap_or_default();
    let bad = |_| ApiError::invalid("body", "malformed", "The delivery could not be read.");
    let projects = connection.projects();
    let (result, links) = match event {
        "ping" => ("pong", 0),
        "push" => {
            let push: source::Push = serde_json::from_slice(&body).map_err(bad)?;
            let (links, deleted) = source::push_links(&push, &projects);
            source::save_links(tx.conn(), tenant_id, id, &links).await?;
            let removed = match deleted {
                Some((repo, branch)) => source::remove_branch(tx.conn(), tenant_id, &repo, &branch).await?,
                None => 0,
            };
            ("linked", links.len() + removed as usize)
        }
        "pull_request" => {
            let event: source::PullRequestEvent = serde_json::from_slice(&body).map_err(bad)?;
            let links = source::pull_request_links(&event, &projects);
            source::save_links(tx.conn(), tenant_id, id, &links).await?;
            ("linked", links.len())
        }
        _ => ("ignored", 0),
    };
    sqlx::query("UPDATE source_connections SET last_event_at = now() WHERE tenant_id = $1 AND id = $2")
        .bind(tenant_id)
        .bind(id)
        .execute(tx.conn())
        .await?;
    tx.commit().await?;
    Ok(Json(HookReceipt { result: result.to_owned(), links }))
}

/// What source control has linked to an issue: commits, branches and pull requests that
/// mention its key. Visible to anyone who can see the issue's project.
#[utoipa::path(get, path = "/api/v1/development", security(("bearer" = [])), params(DevelopmentQuery),
    responses((status = 200, body = Development), (status = 404)))]
pub async fn development(
    State(state): State<AppState>,
    ctx: OrgContext,
    Query(q): Query<DevelopmentQuery>,
) -> ApiResult<Json<Development>> {
    let key = q.issue.trim().to_ascii_uppercase();
    let Some((prefix, number)) = key.split_once('-') else { return Err(ApiError::NotFound("issue")) };
    if number.parse::<u64>().is_err() || prefix.is_empty() {
        return Err(ApiError::NotFound("issue"));
    }
    let mut tx = state.db.begin(ctx.tenant_id).await?;
    let connections = source::connections(tx.conn(), ctx.tenant_id).await?;
    let project_id = connections.iter().flat_map(Connection::projects).find(|(k, _)| k == prefix).map(|(_, id)| id);
    let Some(project_id) = project_id else {
        tx.commit().await?;
        return Ok(Json(Development { items: Vec::new(), connected: false }));
    };
    // Someone who cannot see the project learns nothing, not even that it is connected.
    state
        .policy
        .snapshot(&ctx)
        .await?
        .require(&ctx, project_id, ProjectPermission::Browse)
        .map_err(|_| ApiError::NotFound("issue"))?;
    let items: Vec<DevLinkDto> =
        sqlx::query_as::<_, (String, String, String, String, String, Option<String>, Option<String>, OffsetDateTime)>(
            "SELECT kind, repository, ref, title, url, state, author, occurred_at FROM dev_links \
         WHERE tenant_id = $1 AND issue_key = $2 AND project_id = $3 ORDER BY occurred_at DESC, id DESC LIMIT $4",
        )
        .bind(ctx.tenant_id)
        .bind(&key)
        .bind(project_id)
        .bind(MAX_LINKS_SHOWN)
        .fetch_all(tx.conn())
        .await?
        .into_iter()
        .map(|(kind, repository, reference, title, url, state, author, occurred_at)| DevLinkDto {
            kind,
            repository,
            reference,
            title,
            url,
            state,
            author,
            occurred_at,
        })
        .collect();
    tx.commit().await?;
    Ok(Json(Development { items, connected: true }))
}

/// Routes under `/api/v1`.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/source-connections", get(list).post(create))
        .route("/source-connections/{id}", delete(remove))
        .route("/source-connections/{id}/secret", post(rotate))
        .route("/development", get(development))
}

/// Public routes, outside `/api/v1`.
pub fn hook_routes() -> Router<AppState> {
    Router::new().route("/hooks/github/{id}", post(github_hook).layer(DefaultBodyLimit::max(MAX_BODY)))
}
