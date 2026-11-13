# project-service

**Responsibility.** Projects (key, name, type, lead, archive/restore/delete), components, versions/releases, and
the public API for project access (delegating storage of bindings to permission-service).

**API.** REST under `/api/v1/projects` — contract at `/openapi.json`. Internal gRPC `Projects.GetProject` for
issue-service. Calls permission-service (bindings) and user-service (is this person in the organization?).

**Rules.** Keys are 2–10 uppercase characters, unique per tenant, immutable, and stay reserved after deletion.
Guests cannot create projects. A project the caller has no role on answers `404`, never `403`. Edits use
`If-Match`. Deleting needs an organization admin and an archived project.

**Database.** Owns `project_db`: `projects`, `components`, `versions` (forced RLS), `outbox`, `inbox`. Emits
`project.created`, `project.updated`, `project.deleted`, `version.released`, and `audit.recorded` for access
changes (`project.access_granted`, `project.access_removed`, `project.default_access_changed`).

**Not built yet.** Templates, categories, avatars, milestones/roadmaps, notification and issue-type schemes,
project dashboards, activity feed, retention purge of soft-deleted projects.

| Variable | Required | Default |
|---|---|---|
| `DATABASE_URL`, `DATABASE_MIGRATION_URL` | yes / for `migrate` | |
| `REDIS_URL`, `TOKEN_ISSUER`, `JWKS_URL`, `INTERNAL_SERVICE_SECRET` | yes | |
| `TENANT_SERVICE_GRPC_URL`, `PERMISSION_SERVICE_GRPC_URL`, `USER_SERVICE_GRPC_URL` | no | local ports 9102 / 9108 / 9103 |
| `ALLOWED_ORIGINS`, `HTTP_ADDR`, `GRPC_ADDR` | no | –, `127.0.0.1:8104`, `:9104` |

**Run / test / observe / image.** As for tenant-service, with `project-service`.
