# tenant-service

**Responsibility.** The tenant registry: which tenant belongs to which organization, its deployment
tier, region, status, and the *logical* database destination its data lives in.

**API.** Internal gRPC only ([`proto/platform/tenant/v1/tenant.proto`](../../proto/platform/tenant/v1/tenant.proto)):
`ProvisionTenant` (idempotent per organization), `ResolveTenant`, `GetTenantByOrganization`,
`AbandonTenant`. Calls require a service token. The HTTP port serves only health and metrics.

**Database.** Owns `tenant_db`: `tenants`, `database_clusters`, `tenant_migrations`, `outbox`, `inbox`.
Not tenant-scoped (it is the registry). Emits `tenant.provisioned`.

**Not built yet.** Tier migration execution (the state table exists), provisioning of dedicated
databases/clusters for business and enterprise tiers (they are recorded as `pending` and are not
routable), admin REST API.

| Variable | Required | Default | Purpose |
|---|---|---|---|
| `DATABASE_URL` | yes | | runtime role connection |
| `DATABASE_MIGRATION_URL` | for `migrate` | | owner role connection |
| `INTERNAL_SERVICE_SECRET` | yes | | verifies service tokens (≥ 32 chars) |
| `HTTP_ADDR` / `GRPC_ADDR` | no | `127.0.0.1:8102` / `:9102` | listen addresses |
| `DEFAULT_REGION` | no | `local-1` | region for new tenants |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | no | | enables trace export |

**Run.** `make dev`, or `cargo run -p tenant-service -- migrate` then `cargo run -p tenant-service`.
**Test.** `cargo test -p tenant-service`. **Observe.** `/health/ready`, `/metrics`, JSON logs on stdout.
**Image.** `docker build -f infrastructure/docker/service.Dockerfile --build-arg SERVICE=tenant-service .`
