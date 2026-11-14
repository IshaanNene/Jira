//! Issue attachments: upload validation, storage and authorized download.
//!
//! Bytes go to an S3-compatible object store under a server-generated key; the database
//! holds metadata. Downloads are proxied through this service so every read is
//! authorized, and are always served as attachments so nothing uploaded can run in the
//! application's origin.

use std::sync::Arc;

use auth_middleware::{OrgContext, ProjectPermission};
use axum::{
    extract::{DefaultBodyLimit, Multipart, Path, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    routing::get,
    Json, Router,
};
use bytes::Bytes;
use error_handling::{ApiError, ApiResult};
use event_contracts::payloads::IssueUpdatedV1;
use object_store::{aws::AmazonS3Builder, memory::InMemory, path::Path as ObjectPath, ObjectStore, ObjectStoreExt, PutPayload};
use serde_json::json;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{
    dto::{AttachmentDto, AttachmentList},
    http::{emit, ensure_writable, open},
    store, AppState,
};

pub const MAX_FILE_BYTES: usize = 10 * 1024 * 1024;
const MAX_PER_ISSUE: usize = 50;

/// File kinds that are refused outright: things that execute, or that a browser would
/// render as active content if they were ever served inline.
const BLOCKED_EXTENSIONS: &[&str] = &[
    "exe", "dll", "msi", "bat", "cmd", "com", "scr", "ps1", "vbs", "js", "mjs", "jar", "sh", "bash", "app", "dmg", "pkg", "deb",
    "rpm", "html", "htm", "xhtml", "svg", "php", "hta", "lnk",
];
const BLOCKED_MIME: &[&str] = &[
    "application/x-msdownload",
    "application/vnd.microsoft.portable-executable",
    "application/x-executable",
    "application/x-mach-binary",
    "application/x-sh",
    "text/html",
    "application/java-archive",
];

/// Build the object store from configuration. `ATTACHMENTS_STORE=memory` keeps files in
/// process memory (tests and throwaway runs only).
pub fn store_from_env() -> anyhow::Result<Arc<dyn ObjectStore>> {
    use platform_common::config;
    if config::or("ATTACHMENTS_STORE", "s3") == "memory" {
        tracing::warn!("attachments are stored in memory and will be lost on restart");
        return Ok(Arc::new(InMemory::new()));
    }
    let endpoint = config::required("S3_ENDPOINT")?;
    let store = AmazonS3Builder::new()
        .with_endpoint(&endpoint)
        .with_region(config::or("S3_REGION", "us-east-1"))
        .with_bucket_name(config::required("S3_BUCKET")?)
        .with_access_key_id(config::required("S3_ACCESS_KEY_ID")?)
        .with_secret_access_key(config::required("S3_SECRET_ACCESS_KEY")?)
        // Path-style addressing and plain http are what local S3-compatible stores expect.
        .with_virtual_hosted_style_request(false)
        .with_allow_http(endpoint.starts_with("http://"))
        .build()?;
    Ok(Arc::new(store))
}

/// A file name safe to store and to echo in a header: no path parts, no control characters.
pub fn sanitize_filename(raw: &str) -> Option<String> {
    let base = raw.rsplit(['/', '\\']).next().unwrap_or("");
    let cleaned: String =
        base.chars().filter(|c| !c.is_control() && !matches!(c, '"' | '<' | '>' | '|' | ':' | '*' | '?')).collect();
    let cleaned = cleaned.trim().trim_matches('.').trim().to_owned();
    if cleaned.is_empty() || cleaned.chars().count() > 200 {
        None
    } else {
        Some(cleaned)
    }
}

/// Decide whether a file may be stored and which content type to record. The type comes
/// from the bytes; whatever the client declared is ignored.
pub fn inspect(filename: &str, bytes: &[u8]) -> ApiResult<String> {
    let reject = || ApiError::invalid("file", "blocked_type", "This type of file can't be attached.");
    let extension = filename.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    if BLOCKED_EXTENSIONS.contains(&extension.as_str()) {
        return Err(reject());
    }
    let sniffed = infer::get(bytes).map(|k| k.mime_type());
    if sniffed.is_some_and(|m| BLOCKED_MIME.contains(&m)) {
        return Err(reject());
    }
    // Markup hiding behind a harmless extension.
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(512)]).to_ascii_lowercase();
    if ["<!doctype html", "<html", "<script", "<svg"].iter().any(|needle| head.contains(needle)) {
        return Err(reject());
    }
    Ok(sniffed.unwrap_or(if std::str::from_utf8(bytes).is_ok() { "text/plain" } else { "application/octet-stream" }).to_owned())
}

fn storage_error(err: object_store::Error) -> ApiError {
    tracing::error!(error = %err, "object storage request failed");
    ApiError::DependencyUnavailable("object storage")
}

#[utoipa::path(get, path = "/api/v1/issues/{key}/attachments", security(("bearer" = [])), params(("key" = String, Path)),
    responses((status = 200, body = AttachmentList)))]
pub async fn list(State(state): State<AppState>, ctx: OrgContext, Path(key): Path<String>) -> ApiResult<Json<AttachmentList>> {
    let (mut tx, issue, _) = open(&state, &ctx, &key, ProjectPermission::Browse, false).await?;
    let rows = store::attachments(tx.conn(), ctx.tenant_id, issue.id).await?;
    tx.commit().await?;
    Ok(Json(AttachmentList { items: rows.into_iter().map(Into::into).collect() }))
}

#[utoipa::path(post, path = "/api/v1/issues/{key}/attachments", security(("bearer" = [])), params(("key" = String, Path)),
    request_body(content_type = "multipart/form-data", description = "One part named `file`, at most 10 MB"),
    responses((status = 201, body = AttachmentDto), (status = 413, description = "File too large"), (status = 422, description = "Blocked file type")))]
pub async fn upload(
    State(state): State<AppState>,
    ctx: OrgContext,
    Path(key): Path<String>,
    mut multipart: Multipart,
) -> ApiResult<(StatusCode, Json<AttachmentDto>)> {
    // Authorize before reading the body.
    let (tx, issue, _) = open(&state, &ctx, &key, ProjectPermission::EditIssue, false).await?;
    tx.commit().await?;
    ensure_writable(&state, &ctx, &issue.project_key).await?;

    let bad = |message: &str| ApiError::invalid("file", "invalid_upload", message);
    let mut file: Option<(String, Bytes)> = None;
    while let Some(field) = multipart.next_field().await.map_err(|_| bad("The upload could not be read."))? {
        if field.name() != Some("file") {
            continue;
        }
        let filename = field.file_name().and_then(sanitize_filename).ok_or_else(|| bad("The file needs a valid name."))?;
        let bytes = field.bytes().await.map_err(|_| bad("The file is larger than 10 MB or the upload was interrupted."))?;
        file = Some((filename, bytes));
        break;
    }
    let (filename, bytes) = file.ok_or_else(|| bad("Choose a file to attach."))?;
    if bytes.is_empty() {
        return Err(bad("The file is empty."));
    }
    if bytes.len() > MAX_FILE_BYTES {
        return Err(bad("The file is larger than 10 MB."));
    }
    let content_type = inspect(&filename, &bytes)?;

    let id = platform_common::new_id();
    // The key is built from server-side ids only; nothing user-controlled reaches a storage path.
    let object_key = format!("t/{}/{}", ctx.tenant_id, id);
    let sha256 = hex::encode(Sha256::digest(&bytes));
    let size = i64::try_from(bytes.len()).map_err(ApiError::internal)?;

    let mut tx = state.db.begin(ctx.tenant_id).await?;
    // Lock the issue so the per-issue limit cannot be exceeded by parallel uploads.
    sqlx::query("SELECT 1 FROM issues WHERE tenant_id = $1 AND id = $2 FOR UPDATE")
        .bind(ctx.tenant_id)
        .bind(issue.id)
        .execute(tx.conn())
        .await?;
    if store::attachments(tx.conn(), ctx.tenant_id, issue.id).await?.len() >= MAX_PER_ISSUE {
        return Err(ApiError::Conflict(format!("An issue can have at most {MAX_PER_ISSUE} attachments.")));
    }
    state.blobs.put(&ObjectPath::from(object_key.as_str()), PutPayload::from(bytes)).await.map_err(storage_error)?;
    let saved = async {
        let row = store::insert_attachment(
            tx.conn(),
            ctx.tenant_id,
            id,
            issue.id,
            ctx.user_id,
            &filename,
            &content_type,
            size,
            &sha256,
            &object_key,
        )
        .await?;
        store::add_history(
            tx.conn(),
            ctx.tenant_id,
            issue.id,
            ctx.user_id,
            issue.version,
            &json!([{ "field": "attachment", "from": null, "to": filename }]),
        )
        .await?;
        emit(
            &mut tx,
            &ctx,
            issue.id,
            issue.version,
            IssueUpdatedV1 {
                issue_id: issue.id,
                key: issue.key(),
                project_id: issue.project_id,
                changed_fields: vec!["attachments".into()],
            },
        )
        .await?;
        tx.commit().await?;
        Ok::<_, ApiError>(row)
    }
    .await;
    match saved {
        Ok(row) => Ok((StatusCode::CREATED, Json(row.into()))),
        Err(err) => {
            // The bytes were written but the record was not: remove the orphan.
            if let Err(cleanup) = state.blobs.delete(&ObjectPath::from(object_key.as_str())).await {
                tracing::warn!(error = %cleanup, object_key, "could not remove orphaned attachment object");
            }
            Err(err)
        }
    }
}

#[utoipa::path(get, path = "/api/v1/issues/{key}/attachments/{id}/content", security(("bearer" = [])),
    params(("key" = String, Path), ("id" = Uuid, Path)), responses((status = 200, description = "The file"), (status = 404)))]
pub async fn download(
    State(state): State<AppState>,
    ctx: OrgContext,
    Path((key, id)): Path<(String, Uuid)>,
) -> ApiResult<(HeaderMap, Bytes)> {
    let (mut tx, issue, _) = open(&state, &ctx, &key, ProjectPermission::Browse, false).await?;
    let attachment = store::attachment(tx.conn(), ctx.tenant_id, issue.id, id).await?.ok_or(ApiError::NotFound("attachment"))?;
    tx.commit().await?;
    let bytes = state
        .blobs
        .get(&ObjectPath::from(attachment.object_key.as_str()))
        .await
        .map_err(storage_error)?
        .bytes()
        .await
        .map_err(storage_error)?;

    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(&attachment.content_type).unwrap_or(HeaderValue::from_static("application/octet-stream")),
    );
    // Always a download, never rendered in our origin.
    let ascii: String = attachment.filename.chars().map(|c| if c.is_ascii() && c != '\\' { c } else { '_' }).collect();
    if let Ok(value) = HeaderValue::from_str(&format!("attachment; filename=\"{ascii}\"")) {
        headers.insert(header::CONTENT_DISPOSITION, value);
    }
    headers.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    headers.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static("default-src 'none'; sandbox"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("private, no-store"));
    Ok((headers, bytes))
}

#[utoipa::path(delete, path = "/api/v1/issues/{key}/attachments/{id}", security(("bearer" = [])),
    params(("key" = String, Path), ("id" = Uuid, Path)), responses((status = 204), (status = 403), (status = 404)))]
pub async fn remove(
    State(state): State<AppState>,
    ctx: OrgContext,
    Path((key, id)): Path<(String, Uuid)>,
) -> ApiResult<StatusCode> {
    let (mut tx, issue, role) = open(&state, &ctx, &key, ProjectPermission::EditIssue, false).await?;
    let attachment = store::attachment(tx.conn(), ctx.tenant_id, issue.id, id).await?.ok_or(ApiError::NotFound("attachment"))?;
    // Members remove what they uploaded; administrators can remove anything.
    if attachment.uploader_id != ctx.user_id && !role.allows(ProjectPermission::DeleteIssue) {
        return Err(ApiError::Forbidden);
    }
    store::delete_attachment(tx.conn(), ctx.tenant_id, id).await?;
    store::add_history(
        tx.conn(),
        ctx.tenant_id,
        issue.id,
        ctx.user_id,
        issue.version,
        &json!([{ "field": "attachment", "from": attachment.filename, "to": null }]),
    )
    .await?;
    tx.commit().await?;
    if let Err(err) = state.blobs.delete(&ObjectPath::from(attachment.object_key.as_str())).await {
        // Metadata is gone, so the file is unreachable; the object is left for a storage sweep.
        tracing::warn!(error = %err, object_key = attachment.object_key, "could not delete attachment object");
    }
    Ok(StatusCode::NO_CONTENT)
}

pub fn routes() -> Router<AppState> {
    Router::new()
        // The global 1 MB body limit is raised for this route only (plus multipart overhead).
        .route("/issues/{key}/attachments", get(list).post(upload).layer(DefaultBodyLimit::max(MAX_FILE_BYTES + 64 * 1024)))
        .route("/issues/{key}/attachments/{id}", axum::routing::delete(remove))
        .route("/issues/{key}/attachments/{id}/content", get(download))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filenames_are_reduced_to_a_safe_base_name() {
        assert_eq!(sanitize_filename("report.pdf").as_deref(), Some("report.pdf"));
        assert_eq!(sanitize_filename("../../etc/passwd").as_deref(), Some("passwd"));
        assert_eq!(sanitize_filename("C:\\Users\\a\\notes.txt").as_deref(), Some("notes.txt"));
        assert_eq!(sanitize_filename("a\"b<c>.txt\r\n").as_deref(), Some("abc.txt"));
        assert_eq!(sanitize_filename("..."), None);
        assert_eq!(sanitize_filename(""), None);
        assert_eq!(sanitize_filename(&"x".repeat(300)), None);
    }

    #[test]
    fn content_type_comes_from_the_bytes() {
        let png = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0];
        assert_eq!(inspect("photo.png", &png).unwrap(), "image/png");
        assert_eq!(inspect("lying-name.txt", &png).unwrap(), "image/png", "declared type is irrelevant");
        assert_eq!(inspect("notes.txt", b"hello").unwrap(), "text/plain");
        assert_eq!(inspect("blob.bin", &[0xFF, 0x00, 0xFE, 0x01]).unwrap(), "application/octet-stream");
    }

    #[test]
    fn dangerous_files_are_refused() {
        for name in ["run.exe", "page.HTML", "logo.svg", "script.js", "setup.sh", "macro.vbs"] {
            assert!(inspect(name, b"anything").is_err(), "{name}");
        }
        // Windows executable disguised as an image.
        assert!(inspect("cat.png", b"MZ\x90\x00\x03\x00\x00\x00\x04\x00\x00\x00\xff\xff\x00\x00").is_err());
        // Markup disguised as text.
        assert!(inspect("readme.txt", b"  <!DOCTYPE html><script>alert(1)</script>").is_err());
        assert!(inspect("image.png", b"<svg xmlns='http://www.w3.org/2000/svg' onload='alert(1)'/>").is_err());
    }
}
