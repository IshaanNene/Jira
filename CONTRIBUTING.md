# Contributing

## Before you push

```bash
make lint
make test
```

Both must pass. Integration tests need the PostgreSQL container (`make infra`); each test creates
and drops its own database, connected as an unprivileged role so row-level security is exercised.

## Rules that are enforced in review

- **A feature is a vertical slice**: UI → API → authentication → tenant → authorization →
  database → outbox event → tests. Do not merge one layer and call it done; record partial work in
  `docs/IMPLEMENTATION_ROADMAP.md`.
- **Tenant comes from the verified token only** (`OrgContext.tenant_id`). Never from a path, query,
  header or body. Tenant-owned data is read and written only inside `DbRouter::begin(tenant_id)`.
- **Every tenant-owned table** has `tenant_id`, `FORCE ROW LEVEL SECURITY` and the
  `tenant_isolation` policy. A test in each service fails if one is missing.
- **Every handler names its permission** (`ctx.require(...)` or a domain rule). Hiding a button is
  not authorization.
- **Every write enqueues its event** with `event_contracts::outbox::enqueue` in the same transaction.
- **SQL uses bind parameters.** No string-built predicates from input.
- **No `unwrap`/`expect` in production paths**, no unbounded retries or queues, explicit timeouts on
  every outbound call.
- **Services never import each other** and never read each other's tables. Shared crates hold
  infrastructure and contracts, not business rules.
- **Migrations are forward-only** files in the owning service; schema changes are
  expand → migrate → contract across releases.
- **Web**: server data lives in TanStack Query, client state in Zustand, never both. Modules import
  each other only through `index.ts`. No control without a working operation behind it.
- API changes: update the Rust DTO, run `make gen-api`, commit the regenerated types.

## Adding a service

Copy the shape of `services/tenant-service` (lib + thin `main`, `migrate` subcommand, health,
metrics), add it to `scripts/development/services.sh` and to the gateway routes, and write its
`README.md`. Its database and roles already exist in `infrastructure/postgres/init`.
