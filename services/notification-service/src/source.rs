//! Source control: GitHub repositories tell the workspace about commits, branches and pull
//! requests, and those that mention an issue key are linked to the issue.
//!
//! A workspace administrator connects GitHub by creating a connection for chosen projects;
//! GitHub then POSTs to `/hooks/github/{connection id}`, signing each delivery with the
//! connection's secret (`X-Hub-Signature-256`, HMAC-SHA256 of the body). Nothing in a
//! delivery is trusted until that signature checks out, and even then only what is used is
//! kept, bounded and checked: keys of the connection's own projects, `https://github.com/`
//! links, short plain-text titles. Nothing a delivery contains is ever run.

use reqwest::Url;
use ring::hmac;
use serde::Deserialize;
use sqlx::{FromRow, PgConnection};
use time::OffsetDateTime;
use uuid::Uuid;

pub const MAX_CONNECTIONS: i64 = 10;
pub const MAX_PROJECTS: usize = 50;
/// Largest delivery read. GitHub caps payloads at 25 MB, but a push carries at most 20
/// commits; larger bodies are refused.
pub const MAX_BODY: usize = 2 * 1024 * 1024;
const MAX_COMMITS: usize = 50;
const MAX_KEYS: usize = 10;
const MAX_TITLE: usize = 300;
/// Links returned for one issue, newest first.
pub const MAX_LINKS_SHOWN: i64 = 100;

// --- secrets and signatures ------------------------------------------------------------

/// A new connection secret, entered in GitHub's webhook settings.
pub fn new_secret() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    format!("ghsec_{}", hex::encode(bytes))
}

/// Check GitHub's `X-Hub-Signature-256: sha256=<hex>` over the raw body, in constant time.
pub fn verify(secret: &str, body: &[u8], header: Option<&str>) -> bool {
    let Some(hex_sig) = header.and_then(|h| h.strip_prefix("sha256=")) else { return false };
    let Ok(sig) = hex::decode(hex_sig) else { return false };
    let key = hmac::Key::new(hmac::HMAC_SHA256, secret.as_bytes());
    hmac::verify(&key, body, &sig).is_ok()
}

/// What GitHub would send, for tests and for documentation.
pub fn sign(secret: &str, body: &[u8]) -> String {
    let key = hmac::Key::new(hmac::HMAC_SHA256, secret.as_bytes());
    format!("sha256={}", hex::encode(hmac::sign(&key, body).as_ref()))
}

// --- issue keys ------------------------------------------------------------------------

/// Issue keys in `text` that belong to `projects` (their keys), in order of appearance,
/// without repeats, at most ten. Case does not matter (`pay-12-fix` names PAY-12), and a key
/// must stand alone: `XPAY-1` and `PAY-12a` name nothing.
pub fn issue_keys(text: &str, projects: &[String]) -> Vec<String> {
    let upper = text.to_ascii_uppercase();
    let bytes = upper.as_bytes();
    let mut found: Vec<String> = Vec::new();
    let mut i = 0;
    while i < bytes.len() && found.len() < MAX_KEYS {
        let starts_word = i == 0 || !bytes[i - 1].is_ascii_alphanumeric();
        if !(starts_word && bytes[i].is_ascii_uppercase()) {
            i += 1;
            continue;
        }
        let mut j = i;
        while j < bytes.len() && (bytes[j].is_ascii_uppercase() || bytes[j].is_ascii_digit()) {
            j += 1;
        }
        if j < bytes.len() && bytes[j] == b'-' {
            let mut k = j + 1;
            while k < bytes.len() && bytes[k].is_ascii_digit() {
                k += 1;
            }
            let digits = &upper[j + 1..k];
            let ends_word = k == bytes.len() || !bytes[k].is_ascii_alphanumeric();
            let prefix = &upper[i..j];
            if ends_word && (1..=9).contains(&digits.len()) && !digits.starts_with('0') && projects.iter().any(|p| p == prefix) {
                let key = format!("{prefix}-{digits}");
                if !found.contains(&key) {
                    found.push(key);
                }
            }
            i = k.max(i + 1);
        } else {
            i = j.max(i + 1);
        }
    }
    found
}

// --- GitHub payloads -------------------------------------------------------------------
// Only the fields used are read; GitHub adds fields freely, so unknown ones are ignored.

#[derive(Debug, Deserialize)]
pub struct Repository {
    pub full_name: String,
    pub html_url: String,
}

#[derive(Debug, Deserialize)]
pub struct Push {
    #[serde(rename = "ref")]
    pub git_ref: String,
    #[serde(default)]
    pub deleted: bool,
    #[serde(default)]
    pub commits: Vec<Commit>,
    pub repository: Repository,
}

#[derive(Debug, Deserialize)]
pub struct Commit {
    pub id: String,
    pub message: String,
    pub timestamp: Option<String>,
    pub url: String,
    pub author: Option<Person>,
}

#[derive(Debug, Deserialize)]
pub struct Person {
    pub name: Option<String>,
    pub login: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct PullRequestEvent {
    pub action: String,
    pub pull_request: PullRequest,
    pub repository: Repository,
}

#[derive(Debug, Deserialize)]
pub struct PullRequest {
    pub number: u64,
    pub title: String,
    pub html_url: String,
    pub state: String,
    #[serde(default)]
    pub merged: Option<bool>,
    pub merged_at: Option<String>,
    pub updated_at: Option<String>,
    pub head: Head,
    pub user: Option<Person>,
}

#[derive(Debug, Deserialize)]
pub struct Head {
    #[serde(rename = "ref")]
    pub git_ref: String,
}

/// A commit, branch or pull request linked to an issue.
#[derive(Debug, Clone, PartialEq)]
pub struct Link {
    pub project_id: Uuid,
    pub issue_key: String,
    pub kind: &'static str,
    pub repository: String,
    pub reference: String,
    pub title: String,
    pub url: String,
    pub state: Option<&'static str>,
    pub author: Option<String>,
    pub occurred_at: OffsetDateTime,
}

/// The connection's projects: `(key, id)`.
pub type Projects = [(String, Uuid)];

/// One line of plain text: the first line, control characters dropped, bounded.
fn plain(text: &str, limit: usize) -> String {
    let line = text.lines().next().unwrap_or_default();
    let clean: String = line.chars().filter(|c| !c.is_control()).collect();
    let clean = clean.trim();
    match clean.char_indices().nth(limit) {
        Some((end, _)) => format!("{}…", &clean[..end]),
        None => clean.to_owned(),
    }
}

/// Only links to github.com pages are kept.
fn github_url(raw: &str) -> Option<String> {
    let url = Url::parse(raw).ok()?;
    (url.scheme() == "https" && url.host_str() == Some("github.com") && url.port().is_none() && raw.len() <= 1000)
        .then(|| url.to_string())
}

fn repository_name(repo: &Repository) -> Option<String> {
    let name = &repo.full_name;
    let ok = name.len() <= 200
        && name.split('/').count() == 2
        && name.split('/').all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c)));
    ok.then(|| name.clone())
}

fn when(raw: Option<&str>) -> OffsetDateTime {
    raw.and_then(|t| OffsetDateTime::parse(t, &time::format_description::well_known::Rfc3339).ok())
        .unwrap_or_else(OffsetDateTime::now_utc)
}

fn branch_url(repo: &Repository, branch: &str) -> Option<String> {
    let mut url = Url::parse(&github_url(&repo.html_url)?).ok()?;
    url.path_segments_mut().ok()?.push("tree").extend(branch.split('/'));
    Some(url.to_string())
}

fn project_id(projects: &Projects, key: &str) -> Option<Uuid> {
    let prefix = key.split_once('-')?.0;
    projects.iter().find(|(k, _)| k == prefix).map(|(_, id)| *id)
}

fn keys(projects: &Projects) -> Vec<String> {
    projects.iter().map(|(k, _)| k.clone()).collect()
}

/// What a push means: links for its commits and its branch, and the branch, when the push
/// deleted it (its links are then removed).
pub fn push_links(push: &Push, projects: &Projects) -> (Vec<Link>, Option<(String, String)>) {
    let Some(repository) = repository_name(&push.repository) else { return (Vec::new(), None) };
    let project_keys = keys(projects);
    let branch = push.git_ref.strip_prefix("refs/heads/").filter(|b| !b.is_empty() && b.len() <= 255);
    if push.deleted {
        return (Vec::new(), branch.map(|b| (repository, b.to_owned())));
    }
    let mut links = Vec::new();
    for commit in push.commits.iter().take(MAX_COMMITS) {
        let sha_ok = commit.id.len() == 40 && commit.id.chars().all(|c| c.is_ascii_hexdigit());
        let Some(url) = github_url(&commit.url).filter(|_| sha_ok) else { continue };
        let author = commit.author.as_ref().and_then(|a| a.name.clone().or_else(|| a.login.clone())).map(|n| plain(&n, 100));
        for key in issue_keys(&commit.message, &project_keys) {
            let Some(project_id) = project_id(projects, &key) else { continue };
            links.push(Link {
                project_id,
                issue_key: key,
                kind: "commit",
                repository: repository.clone(),
                reference: commit.id.to_ascii_lowercase(),
                title: plain(&commit.message, MAX_TITLE),
                url: url.clone(),
                state: None,
                author: author.clone(),
                occurred_at: when(commit.timestamp.as_deref()),
            });
        }
    }
    if let Some(name) = branch {
        if let Some(url) = branch_url(&push.repository, name) {
            for key in issue_keys(name, &project_keys) {
                let Some(project_id) = project_id(projects, &key) else { continue };
                links.push(Link {
                    project_id,
                    issue_key: key,
                    kind: "branch",
                    repository: repository.clone(),
                    reference: name.to_owned(),
                    title: plain(name, MAX_TITLE),
                    url: url.clone(),
                    state: None,
                    author: None,
                    occurred_at: OffsetDateTime::now_utc(),
                });
            }
        }
    }
    (links, None)
}

/// Links for a pull request whose title or branch names issues.
pub fn pull_request_links(event: &PullRequestEvent, projects: &Projects) -> Vec<Link> {
    const TRACKED: [&str; 6] = ["opened", "reopened", "edited", "closed", "synchronize", "ready_for_review"];
    let pr = &event.pull_request;
    let (Some(repository), Some(url)) = (repository_name(&event.repository), github_url(&pr.html_url)) else {
        return Vec::new();
    };
    if !TRACKED.contains(&event.action.as_str()) {
        return Vec::new();
    }
    let merged = pr.merged == Some(true) || pr.merged_at.is_some();
    let state = match (merged, pr.state.as_str()) {
        (true, _) => "merged",
        (false, "closed") => "closed",
        _ => "open",
    };
    let project_keys = keys(projects);
    let mut found = issue_keys(&pr.title, &project_keys);
    for key in issue_keys(&pr.head.git_ref, &project_keys) {
        if !found.contains(&key) && found.len() < MAX_KEYS {
            found.push(key);
        }
    }
    found
        .into_iter()
        .filter_map(|key| {
            Some(Link {
                project_id: project_id(projects, &key)?,
                issue_key: key,
                kind: "pull_request",
                repository: repository.clone(),
                reference: pr.number.to_string(),
                title: plain(&pr.title, MAX_TITLE),
                url: url.clone(),
                state: Some(state),
                author: pr.user.as_ref().and_then(|u| u.login.clone()).map(|n| plain(&n, 100)),
                occurred_at: when(pr.updated_at.as_deref()),
            })
        })
        .collect()
}

// --- storage ---------------------------------------------------------------------------

#[derive(Debug, Clone, FromRow)]
pub struct Connection {
    pub id: Uuid,
    pub provider: String,
    pub name: String,
    pub project_ids: Vec<Uuid>,
    pub project_keys: Vec<String>,
    pub secret_sealed: Vec<u8>,
    pub last_event_at: Option<OffsetDateTime>,
    pub created_by: Uuid,
    pub created_at: OffsetDateTime,
}

pub const CONNECTION: &str =
    "id, provider, name, project_ids, project_keys, secret_sealed, last_event_at, created_by, created_at";

impl Connection {
    pub fn projects(&self) -> Vec<(String, Uuid)> {
        self.project_keys.iter().cloned().zip(self.project_ids.iter().copied()).collect()
    }
}

pub async fn connections(conn: &mut PgConnection, tenant_id: Uuid) -> sqlx::Result<Vec<Connection>> {
    sqlx::query_as(&format!("SELECT {CONNECTION} FROM source_connections WHERE tenant_id = $1 ORDER BY created_at, id"))
        .bind(tenant_id)
        .fetch_all(conn)
        .await
}

pub async fn connection(conn: &mut PgConnection, tenant_id: Uuid, id: Uuid) -> sqlx::Result<Option<Connection>> {
    sqlx::query_as(&format!("SELECT {CONNECTION} FROM source_connections WHERE tenant_id = $1 AND id = $2"))
        .bind(tenant_id)
        .bind(id)
        .fetch_optional(conn)
        .await
}

/// Save links, refreshing any already known (a pull request's state, a commit seen again).
pub async fn save_links(conn: &mut PgConnection, tenant_id: Uuid, connection_id: Uuid, links: &[Link]) -> sqlx::Result<()> {
    for l in links {
        sqlx::query(
            "INSERT INTO dev_links (id, tenant_id, connection_id, project_id, issue_key, kind, repository, ref, title, url, state, author, occurred_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13) \
             ON CONFLICT (tenant_id, issue_key, kind, repository, ref) DO UPDATE SET connection_id = EXCLUDED.connection_id, \
                 title = EXCLUDED.title, url = EXCLUDED.url, state = EXCLUDED.state, author = COALESCE(EXCLUDED.author, dev_links.author), \
                 occurred_at = GREATEST(dev_links.occurred_at, EXCLUDED.occurred_at), updated_at = now()",
        )
        .bind(platform_common::new_id())
        .bind(tenant_id)
        .bind(connection_id)
        .bind(l.project_id)
        .bind(&l.issue_key)
        .bind(l.kind)
        .bind(&l.repository)
        .bind(&l.reference)
        .bind(&l.title)
        .bind(&l.url)
        .bind(l.state)
        .bind(&l.author)
        .bind(l.occurred_at)
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

pub async fn remove_branch(conn: &mut PgConnection, tenant_id: Uuid, repository: &str, branch: &str) -> sqlx::Result<u64> {
    Ok(sqlx::query("DELETE FROM dev_links WHERE tenant_id = $1 AND kind = 'branch' AND repository = $2 AND ref = $3")
        .bind(tenant_id)
        .bind(repository)
        .bind(branch)
        .execute(conn)
        .await?
        .rows_affected())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn projects() -> Vec<(String, Uuid)> {
        vec![("PAY".into(), Uuid::from_u128(1)), ("OPS2".into(), Uuid::from_u128(2))]
    }

    #[test]
    fn keys_must_stand_alone_and_belong_to_a_connected_project() {
        let p: Vec<String> = projects().into_iter().map(|(k, _)| k).collect();
        assert_eq!(issue_keys("PAY-12: fix, also pay-7 and OPS2-3. PAY-12 again", &p), ["PAY-12", "PAY-7", "OPS2-3"]);
        assert_eq!(issue_keys("feature/pay-41-refunds", &p), ["PAY-41"]);
        for none in ["XPAY-1", "PAY-12a", "PAY-0", "PAY-", "PAY-1234567890", "UTF-8", "OTHER-5", "PAY_1", "PAY 1"] {
            assert!(issue_keys(none, &p).is_empty(), "{none}");
        }
        let many: String = (1..30).map(|n| format!("PAY-{n} ")).collect();
        assert_eq!(issue_keys(&many, &p).len(), 10);
    }

    #[test]
    fn signatures_are_checked_over_the_exact_body() {
        let body = br#"{"zen":"Keep it logically awesome."}"#;
        let header = sign("ghsec_x", body);
        assert!(verify("ghsec_x", body, Some(&header)));
        assert!(!verify("ghsec_y", body, Some(&header)));
        assert!(!verify("ghsec_x", b"{}", Some(&header)));
        assert!(!verify("ghsec_x", body, None));
        assert!(!verify("ghsec_x", body, Some(header.trim_start_matches("sha256="))));
        assert!(!verify("ghsec_x", body, Some("sha256=zz")));
    }

    #[test]
    fn pushes_link_commits_and_branches_and_nothing_untrusted() {
        let sha = "a".repeat(40);
        let push: Push = serde_json::from_value(json!({
            "ref": "refs/heads/feature/PAY-9-retries",
            "repository": { "full_name": "acme/payments", "html_url": "https://github.com/acme/payments" },
            "commits": [
                { "id": sha, "message": "PAY-9: retry refunds\n\nDetails PAY-10", "timestamp": "2026-11-16T10:00:00Z",
                  "url": format!("https://github.com/acme/payments/commit/{sha}"), "author": { "name": "Ada" } },
                { "id": "b".repeat(40), "message": "PAY-11 sneaky", "url": "javascript:alert(1)", "author": { "name": "Eve" } },
                { "id": "not-a-sha", "message": "PAY-12", "url": "https://github.com/acme/payments/commit/x" },
                { "id": "c".repeat(40), "message": "chore: nothing to see", "url": format!("https://github.com/acme/payments/commit/{}", "c".repeat(40)) }
            ]
        }))
        .unwrap();
        let (links, deleted) = push_links(&push, &projects());
        assert!(deleted.is_none());
        let summary: Vec<(&str, &str, &str)> = links.iter().map(|l| (l.kind, l.issue_key.as_str(), l.title.as_str())).collect();
        assert_eq!(
            summary,
            [
                ("commit", "PAY-9", "PAY-9: retry refunds"),
                ("commit", "PAY-10", "PAY-9: retry refunds"),
                ("branch", "PAY-9", "feature/PAY-9-retries")
            ]
        );
        assert_eq!(links[2].url, "https://github.com/acme/payments/tree/feature/PAY-9-retries");
        assert_eq!(links[0].author.as_deref(), Some("Ada"));

        let gone: Push = serde_json::from_value(json!({
            "ref": "refs/heads/feature/PAY-9-retries", "deleted": true,
            "repository": { "full_name": "acme/payments", "html_url": "https://github.com/acme/payments" }
        }))
        .unwrap();
        assert_eq!(push_links(&gone, &projects()).1, Some(("acme/payments".into(), "feature/PAY-9-retries".into())));

        let odd: Push = serde_json::from_value(json!({
            "ref": "refs/heads/PAY-1",
            "repository": { "full_name": "../../etc", "html_url": "https://github.com/x/y" }
        }))
        .unwrap();
        assert!(push_links(&odd, &projects()).0.is_empty(), "odd repository names are refused");
    }

    #[test]
    fn pull_requests_carry_their_state() {
        let pr = |action: &str, state: &str, merged: bool| -> PullRequestEvent {
            serde_json::from_value(json!({
                "action": action,
                "repository": { "full_name": "acme/payments", "html_url": "https://github.com/acme/payments" },
                "pull_request": {
                    "number": 42, "title": "Retry refunds (PAY-9)", "html_url": "https://github.com/acme/payments/pull/42",
                    "state": state, "merged": merged, "merged_at": null, "updated_at": "2026-11-16T11:00:00Z",
                    "head": { "ref": "ops2-3-hotfix" }, "user": { "login": "ada" }
                }
            }))
            .unwrap()
        };
        let opened = pull_request_links(&pr("opened", "open", false), &projects());
        assert_eq!(opened.iter().map(|l| l.issue_key.as_str()).collect::<Vec<_>>(), ["PAY-9", "OPS2-3"]);
        assert_eq!(
            (opened[0].state, opened[0].reference.as_str(), opened[1].project_id),
            (Some("open"), "42", Uuid::from_u128(2))
        );
        assert_eq!(pull_request_links(&pr("closed", "closed", true), &projects())[0].state, Some("merged"));
        assert_eq!(pull_request_links(&pr("closed", "closed", false), &projects())[0].state, Some("closed"));
        assert!(pull_request_links(&pr("labeled", "open", false), &projects()).is_empty());
    }
}
