# ADR-0002: Hybrid tenancy; RLS stays on in every tier

Status: accepted

**Context.** Standard tenants share tables; business tenants get a database; enterprise tenants get a cluster.

**Decision.** One code path: `DbRouter::begin(tenant_id)` resolves a pool from the tenant registry's logical `database_destination`, opens a transaction and sets `app.tenant_id`. All tenant-owned tables use FORCE row-level security in all tiers. Connection strings are resolved from the service's own secrets by logical name, never stored in or read from the registry.

**Alternatives.** Schema-per-tenant (migration fan-out, catalog bloat at >10k tenants); application-only filtering (one missed predicate is a breach); disabling RLS on dedicated tiers (two code paths, weaker failure mode).

**Consequences.** A routing bug yields empty results instead of a leak. Runtime roles must not own tables. Every transaction pays one `set_config` round trip (batched with `BEGIN`).
