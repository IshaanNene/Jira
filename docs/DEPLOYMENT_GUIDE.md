# Deployment guide

Local development is described in the root [README](../README.md). This document covers the production target.
Sections marked **(planned — Phase 8)** describe design that has no manifests in the repository yet.

## 1. Environments

| Env | Where | Stateful dependencies |
|---|---|---|
| local | host processes + Docker Compose | containers |
| ci | GitHub Actions | service containers |
| staging / production | Kubernetes, one region, three availability zones | **managed** PostgreSQL, Kafka, Redis, OpenSearch, object storage; ClickHouse Cloud or operator |

Running stateful systems in the cluster does not make them highly available by itself. Where managed services
are unavailable use CloudNativePG, Strimzi, the OpenSearch operator and the Altinity ClickHouse operator, and
budget for their operation (backups, upgrades, quorum loss procedures).

## 2. Kubernetes layout **(planned — Phase 8)**

| Namespace | Workloads | Scaling signal |
|---|---|---|
| `core` | API Deployments of all request/response services | HPA on CPU + RPS |
| `realtime` | realtime-gateway | HPA on open connections; long `terminationGracePeriodSeconds`, drain on SIGTERM |
| `workers` | outbox relays, notification / automation / indexer / analytics consumers | KEDA on Kafka consumer lag |
| `observability` | OTel Collector, Prometheus, Grafana, Loki, Jaeger | — |
| `infrastructure` | Envoy Gateway, External Secrets, cert-manager, KEDA, Argo Rollouts | — |

Every Deployment gets: startup/readiness/liveness probes (`/health/startup`, `/health/ready`, `/health/live`),
requests + limits, PodDisruptionBudget (`maxUnavailable: 1`), topology spread across zones, a dedicated
ServiceAccount with workload identity, read-only root filesystem, non-root user, default-deny NetworkPolicy with
explicit allows (gateway → service HTTP, service → service gRPC per the dependency matrix, service → its data stores).

One generic Helm sub-chart (`infrastructure/helm/service`) is instantiated per service by the umbrella chart
(`infrastructure/helm/platform`); per-environment values in `values-<env>.yaml`.

## 3. Gateway

Envoy Gateway (Gateway API): `HTTPRoute` per path prefix → service; `SecurityPolicy` for JWT (remote JWKS from
identity) on everything except `/api/v1/auth/*`, `/api/v1/invitations/*` and inbound webhooks;
`BackendTrafficPolicy` for rate limits and timeouts; `ClientTrafficPolicy` for body size and TLS. WebSocket
upgrade and SSE (no buffering, long idle timeout) on `/rt/*`.

Locally the same routing table is expressed as a static Envoy config
(`infrastructure/envoy/envoy.dev.yaml`), and the Vite dev server proxies to it.

## 4. Delivery **(planned — Phase 8)**

- **CI (GitHub Actions)**: fmt, clippy `-D warnings`, tests (with PostgreSQL/Redis service containers),
  `cargo audit`/`deny`, web lint/typecheck/test/build, image build, Trivy scan, push with digest.
- **CD (Argo CD)**: app-of-apps per environment pointing at the Helm chart; image tags bumped by PR.
  Argo Rollouts canary (5 % → 25 % → 100 %) gated on error-rate and latency analysis from Prometheus.
- **Migrations**: a pre-sync Job per service runs `sqlx migrate run` as the owner role. Migrations are
  backwards-compatible with the previous release (expand/contract) so a rollback of the code never needs a
  rollback of the schema.
- **Rollback**: `argocd app rollback` / Rollouts abort. Schema is not rolled back; see above.

## 5. Infrastructure as code **(planned — Phase 8)**

Terraform modules: `network`, `kubernetes`, `postgres` (shared cluster + per-tenant dedicated factory), `kafka`,
`redis`, `opensearch`, `object-storage`, `dns-cdn`. Remote state with locking; one workspace per environment.
The SPA is a static build served from object storage behind a CDN.

## 6. Regions and data residency

Start: one region, multi-AZ. The tenant registry already records `region` and services are deployed per region
with a region-local data plane, so adding a region is "deploy the stack again + global routing by tenant".
No active-active writes: a tenant lives in exactly one region. Identity (global accounts) is the one dataset
that needs a cross-region strategy before a second region opens — an open design item.

## 7. Backup and recovery

Targets (not yet validated): RPO ≤ 5 min, RTO ≤ 60 min.
PostgreSQL: continuous WAL archiving + daily base backup, PITR, monthly restore drill.
Kafka: replication factor 3, `min.insync.replicas=2`; it is not a backup — projections rebuild from PostgreSQL.
OpenSearch / ClickHouse: rebuildable; snapshots only shorten recovery.
Object storage: versioning + cross-zone replication.
Runbooks live in `docs/operations/` as they are written and rehearsed.
