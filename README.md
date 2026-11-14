# Meridian (working name)

A multi-tenant project-management platform: Rust microservices, a React + TypeScript web app,
PostgreSQL with row-level security, and an event-driven core. The product name is configuration
(`PRODUCT_NAME`, `VITE_PRODUCT_NAME`), not a brand baked into the code.

> **Status: Phases 1–3 of 8 are built.** Accounts, tenants, organizations, members, invitations,
> projects, project access and issues (comments, links, history, child issues, custom fields,
> attachments) and configurable workflows with a visual designer work end to end. Boards, sprints, search, notifications, automation,
> analytics and Kubernetes delivery are designed but **not built yet**.
> [docs/IMPLEMENTATION_ROADMAP.md](docs/IMPLEMENTATION_ROADMAP.md) is the honest ledger of what
> exists, what doesn't, and what has been measured (no load tests so far).

## Quick start

Prerequisites: Docker (with Compose), Rust stable (1.85+), Node 22+, pnpm 10+, `protoc`.

```bash
make web-install   # once
make dev           # infrastructure, migrations, services, web app
make seed          # optional demo workspace and accounts (prints the logins)
```

Open http://localhost:5173. `make stop` stops the host processes; `make clean` also removes
containers, volumes and build output.

| Command | What it does |
|---|---|
| `make dev` / `make stop` | start / stop everything |
| `make test` | Rust unit + integration tests (real PostgreSQL) and web unit tests |
| `make lint` | rustfmt, clippy with warnings as errors, eslint, tsc |
| `make migrate` | apply every service's migrations |
| `make gen-api` | regenerate TypeScript types from the services' OpenAPI documents |
| `make logs` | tail service logs (`.run/logs/`) |
| `make observability` | Prometheus (:59090), Grafana (:53000) and Jaeger (:56686) |
| `make resilience` / `make load` | failure tests (stop each data store) / k6 load test, against the running stack |
| `make kind-up` / `make kind-down` | build the images and run the Helm chart in a local kind cluster (:8088) |

Keep the checkout outside iCloud-synced folders such as the Desktop: evicted files make local logs and caches hang.

## What is running locally

| Piece | Address | Notes |
|---|---|---|
| Web app (Vite) | http://localhost:5173 | proxies `/api` to the gateway |
| Gateway (Envoy) | http://localhost:8080 | request ids, rate limits, edge JWT check, routing |
| identity-service | 127.0.0.1:8101 | accounts, sessions, tokens, JWKS |
| tenant-service | 127.0.0.1:8102 (gRPC 9102) | tenant registry and placement |
| user-service | 127.0.0.1:8103 (gRPC 9103) | profiles, organizations, members, invitations |
| permission-service | 127.0.0.1:8108 (gRPC 9108) | project role bindings, policy versions |
| project-service | 127.0.0.1:8104 (gRPC 9104) | projects, components, versions, access |
| workflow-service | 127.0.0.1:8106 (gRPC 9106) | workflow definitions, publishing, transition engine |
| issue-service | 127.0.0.1:8105 | issues, comments, links, history |
| PostgreSQL / Redis | 127.0.0.1:54320 / 63790 | non-default ports to avoid local installs |
| Object storage (SeaweedFS, S3 API) | 127.0.0.1:58333 | attachments |

Each service exposes `/health/live`, `/health/ready`, `/metrics` (Prometheus) and `/openapi.json`.
Configuration is environment variables; `.env.example` documents every one and contains only
development placeholders.

## Repository layout

```
.github/          CI workflow
apps/web/         React app (modules per feature, design system in src/components/ui)
services/         one crate per service: src/, migrations/, tests/, README.md
crates/           shared infrastructure only (errors, auth, tenant routing, events, telemetry)
proto/            internal gRPC contracts
infrastructure/   docker images, envoy, postgres bootstrap, helm chart, terraform, observability
scripts/          development, seed, images, kind
tests/            load (k6) and failure tests
docs/             architecture, design documents, ADRs
```

## Documentation

- [Architecture](docs/ARCHITECTURE.md) · [System design](docs/SYSTEM_DESIGN.md) · [Database](docs/DATABASE_DESIGN.md)
- [API](docs/API_SPECIFICATION.md) · [Events](docs/EVENT_ARCHITECTURE.md) · [Security](docs/SECURITY_DESIGN.md)
- [Deployment](docs/DEPLOYMENT_GUIDE.md) · [Roadmap and status](docs/IMPLEMENTATION_ROADMAP.md) · [Decision records](docs/adrs)
