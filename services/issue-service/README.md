# issue-service

**Responsibility.** Issue lifecycle: creation with safe key allocation, editing with optimistic concurrency,
hierarchy (epic → issue → subtask), comments, links, change history, watchers.

**API.** REST under `/api/v1/issues` — contract at `/openapi.json`. Calls project-service (`GetProject`, cached
15 s) and user-service (assignee is an organization member).

**How the important parts work.**
- *Keys*: `project_issue_counters` row per project, incremented inside the insert transaction — unique and
  gap-free under concurrency (tested with 25 parallel creators).
- *Concurrency*: `PATCH` requires `If-Match`; the row is locked, the version compared, and a mismatch returns
  `409` with the current issue. No-op edits do not bump the version.
- *Rich text*: descriptions and comments are structured documents validated against an allow-list of nodes,
  marks and link schemes; they are never stored or rendered as HTML.
- *Authorization*: every handler checks a project permission from the caller's policy snapshot; invisible
  issues and projects are indistinguishable from missing ones, including as link targets.

- *Custom fields*: definitions in `custom_fields`, values in `issues.custom_fields` (JSONB), validated in
  `custom_fields.rs` against type, options, context and required-ness.
- *Attachments*: bytes in S3-compatible storage under `t/{tenant}/{id}`; content type decided from the bytes;
  executables and markup refused; downloads proxied, authorized and always served as attachments.

**Database.** Owns `issue_db`: `custom_fields`, `attachments`, `issues`, `project_issue_counters`, `comments`, `issue_links`, `issue_history`,
`watchers` (forced RLS), `outbox`, `inbox`. Emits `issue.created|updated|transitioned|assigned|deleted|mentioned`,
`comment.created|deleted`.

**Temporary.** Statuses and transitions are a fixed workflow in `domain.rs` until workflow-service (Phase 3).

**Not built yet.** Custom issue types, attachment malware scanning and previews, filtering by custom field,
time tracking, votes, clone / move / bulk edit, import / export, mentions UI, real-time updates.

| Variable | Required | Default |
|---|---|---|
| `DATABASE_URL`, `DATABASE_MIGRATION_URL` | yes / for `migrate` | |
| `REDIS_URL`, `TOKEN_ISSUER`, `JWKS_URL`, `INTERNAL_SERVICE_SECRET` | yes | |
| `TENANT_SERVICE_GRPC_URL`, `PERMISSION_SERVICE_GRPC_URL`, `PROJECT_SERVICE_GRPC_URL`, `USER_SERVICE_GRPC_URL` | no | local ports |
| `S3_ENDPOINT`, `S3_BUCKET`, `S3_ACCESS_KEY_ID`, `S3_SECRET_ACCESS_KEY` | yes (unless `ATTACHMENTS_STORE=memory`) | |
| `S3_REGION` | no | `us-east-1` |
| `ALLOWED_ORIGINS`, `HTTP_ADDR` | no | –, `127.0.0.1:8105` |

**Run / test / observe / image.** As for tenant-service, with `issue-service`.
