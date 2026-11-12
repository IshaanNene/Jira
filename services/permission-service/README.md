# permission-service

**Responsibility.** Source of truth for project role bindings (who is administrator / member / viewer of
which project), per-project default roles for organization members, and the per-tenant policy version.

**API.** Internal gRPC only ([`permission.proto`](../../proto/platform/permission/v1/permission.proto)):
`GetPolicySnapshot`, `SetProjectRole`, `RemoveProjectRole`, `SetProjectDefaultRole`, `ListProjectMembers`,
`DeleteProject`. Other services evaluate permissions **locally** from the snapshot
(`auth_middleware::PolicyClient`); this service is not called per request.

**Invalidation.** Every real change bumps `policy_versions.version` in the same transaction and publishes it to
Redis (`t:{tenant}:pv`). Consumers compare that value with their cached snapshot on each request; without Redis
they trust a snapshot for 5 seconds.

**Database.** Owns `permission_db`: `project_role_bindings`, `project_default_roles`, `policy_versions` (all
forced RLS), `outbox`, `inbox`. Emits `permission.updated`.

**Not built yet.** Custom roles, configurable permission schemes (the role → permission table is fixed in
`auth-middleware`), groups, issue security levels, field-level permissions, access reviews.

| Variable | Required | Default |
|---|---|---|
| `DATABASE_URL`, `DATABASE_MIGRATION_URL` | yes / for `migrate` | |
| `REDIS_URL`, `INTERNAL_SERVICE_SECRET` | yes | |
| `TENANT_SERVICE_GRPC_URL` | no | `http://127.0.0.1:9102` |
| `HTTP_ADDR` / `GRPC_ADDR` | no | `127.0.0.1:8108` / `:9108` |

**Run / test / observe / image.** As for tenant-service, with `permission-service`.
