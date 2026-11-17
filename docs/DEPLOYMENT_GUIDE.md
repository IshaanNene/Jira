# Deployment guide

Local development is described in the root [README](../README.md). This document covers the production target
and the pieces of it that are in the repository: container images (`infrastructure/docker`), the Helm chart
(`infrastructure/helm/meridian`), Terraform for AWS (`infrastructure/terraform`), CI (`.github/workflows/ci.yml`),
the observability configuration (`infrastructure/observability`), and a local Kubernetes run of all of it
(`make kind-up`). Items marked **(not built)** are design only.

## 1. Environments

| Env | Where | Stateful dependencies |
|---|---|---|
| local | host processes + Docker Compose | containers |
| ci | GitHub Actions | service containers |
| staging / production | Kubernetes, one region, three availability zones | **managed** PostgreSQL, Kafka, Redis, OpenSearch, object storage; ClickHouse Cloud or operator |

Running stateful systems in the cluster does not make them highly available by itself. Where managed services
are unavailable use CloudNativePG, Strimzi, the OpenSearch operator and the Altinity ClickHouse operator, and
budget for their operation (backups, upgrades, quorum loss procedures).

## 2. Kubernetes

### Images

`infrastructure/docker/service.Dockerfile` builds every Rust service from one shared build stage (the workspace
compiles once for all twelve images) onto Debian bookworm slim, upgraded at build time, running as uid 65532. The
binaries use rustls for HTTP, Postgres and Redis; librdkafka (and, in identity-service, xmlsec1 for SAML) link the
system OpenSSL, installed as Debian's `libssl3` so Trivy sees and checks it. identity-service's image adds
`libxmlsec1-openssl` (the Dockerfile's `xmlsec` runtime). Until 2026-11-16 the images were distroless without
OpenSSL; that changed with SAML, and distroless's own OpenSSL lagged a fixed HIGH CVE (libssl3 3.0.20), which
Debian's security updates had fixed (3.0.22).
`infrastructure/docker/web.Dockerfile` builds the web app onto unprivileged nginx 1.30 with Alpine updates applied
(SPA fallback, immutable caching of hashed assets, a strict Content-Security-Policy).
`scripts/images/build.sh <tag> [registry]` builds them all, always pulling fresh base images, and starts each
service image once without configuration so that a missing shared library fails the build. On 2026-11-14 all
thirteen images passed Trivy with no fixable high or critical findings (12 service images ~80–96 MB, web 89 MB).

### The chart (`infrastructure/helm/meridian`)

One chart for the application; data stores are external. Per service (from `values.yaml` → `services`):
Deployment, Service (HTTP 8080, gRPC 9090 where the service has it), ServiceAccount without an API token,
PodDisruptionBudget (`maxUnavailable: 1`), HorizontalPodAutoscaler on CPU, and a pre-install/pre-upgrade migration
Job for services with a database. Every pod: non-root, read-only root filesystem, all capabilities dropped,
`RuntimeDefault` seccomp, startup/readiness/liveness probes on `/health/live` and `/health/ready`, a small `/tmp`
emptyDir, topology spread across zones and nodes, Prometheus scrape annotations, a config checksum so settings
changes roll the pods.

- **Configuration**: one ConfigMap of non-secret settings (public URL, in-cluster gRPC and JWKS addresses, data
  store endpoints).
- **Secrets** are never in values. The chart reads `meridian-shared` (internal service secret, Redis URL, Kafka
  SASL credentials), `meridian-<service>` (that service's runtime database URL and its own credentials) and
  `meridian-<service>-migrate` (the owner-role URL, given only to the migration Job). External Secrets fills them
  from AWS Secrets Manager when `externalSecrets.enabled` (staging): one `ExternalSecret` per Secret, reading the
  store entry `<prefix>/<Secret name>` (a JSON object of keys) from a `ClusterSecretStore`. They are
  pre-install/pre-upgrade hooks weighted before the migration Jobs, so the Jobs' credentials exist on a first
  install, and the Secrets they write are orphaned so recreating a hook never removes them from running pods.
- **Gateway**: Envoy, configured from `gateway.routes`: the same request ids, client address handling, body and
  rate limits, edge JWT check and identity-header stripping as the local gateway, plus HSTS on https origins.
  The admin API listens on loopback only; `/ready` and `/stats/prometheus` are exposed read-only on port 8081.
  Unmatched `/api` paths get a problem+json 404; everything else goes to the web app.
- **NetworkPolicy**: default deny for every pod of the release; the gateway accepts traffic on 8080 (and 8081
  from the monitoring namespace); services accept HTTP only from the gateway and monitoring, and gRPC only from
  the release's services; identity's HTTP port is also open to services (signing keys). Egress is limited to DNS,
  the release's pods and the CIDRs in `networkPolicy.egress.cidrs` (the data stores' subnets).
- **Migrations** are expand/contract, so a rollback of the code never needs one of the schema; a failed Job
  stops the upgrade before any pod changes.

- **Autoscaling**: an HPA on CPU per service. With `keda.enabled` (staging), services that run Kafka consumers
  (user, agile, notification, search, analytics; `services.<name>.consumers` lists their groups) get a KEDA
  `ScaledObject` instead, scaling on CPU and on each group's lag (`keda.lagThreshold` messages per replica),
  signing in to MSK with the platform's SCRAM credentials through a `TriggerAuthentication`; `maxReplicas`
  stays at or below the partition count (12 on MSK).

- **Canaries** (`rollouts.enabled`, staging): services are Argo Rollouts `Rollout`s. A new version takes 25, 50
  and 75 % of the replicas for five minutes each; the `meridian-canary` AnalysisTemplate checks the new pods'
  own 5xx ratio and share of requests over 500 ms in Prometheus (`rollouts.prometheusAddress`, the `prometheus`
  Argo CD add-on in staging) each minute, and
  two misses abort the release and scale the canary back. `kubectl argo rollouts promote <service>` skips the
  remaining pauses; `kubectl argo rollouts abort <service>` stops one by hand.

- **Image signatures** (`imageVerification.enabled`, staging): a Kyverno `Policy` in the release namespace admits
  pods running `image.registry/*` only if the digest carries a keyless cosign signature from CI's workflow on
  `main` (`imageVerification.subjectRegExp`) and, by default, a signed CycloneDX SBOM attestation; pods are
  pinned to the verified digest. An image built elsewhere, or a tag moved to an unsigned image, is refused.

Every rendered manifest, for the default, kind and staging values, passes `kubeconform -strict` against the
Kubernetes 1.31 schemas and the CRD catalog (ExternalSecret, ScaledObject, Argo CD) in CI.

Not built: a separate `realtime` HPA signal (open connections), Argo Rollouts canaries, the Gateway API
(Envoy Gateway) form of the edge, cert-manager and TLS termination (the cloud load balancer terminates TLS).

### Local Kubernetes (`make kind-up`)

`scripts/kind/up.sh` creates a three-node kind cluster (workers labelled with two zones), loads the images,
attaches the Docker Compose data stores to the nodes' network under their service names (the way production
reaches managed services outside the cluster), creates the Secrets from the development placeholders, and
installs the chart with `values-kind.yaml` (one replica each, no autoscaling, gateway on `localhost:8088`).
`make kind-down` deletes the cluster.

Checked on 2026-11-14 (kind 0.32, Kubernetes 1.36): every migration Job succeeded, all 14 pods ran across the two
workers, and sign-in, search, notifications and dashboards worked through the cluster's gateway, including in a
browser. Probes from test pods showed the **ingress** rules enforced: another namespace, an unlabelled pod and a
service pod could not reach a service's HTTP port, while the gateway could and services could fetch identity's
signing keys. The **egress** rules are not enforced by kind's built-in network plugin (kindnetd): a release pod
reached an in-cluster pod and the internet that the policy does not allow. Checking egress needs a cluster whose
CNI enforces it (Calico, Cilium, or the EKS VPC CNI with network policies turned on); that has not been done.

## 3. Gateway

Envoy Gateway (Gateway API): `HTTPRoute` per path prefix → service; `SecurityPolicy` for JWT (remote JWKS from
identity) on everything except `/api/v1/auth/*`, `/api/v1/invitations/*` and inbound webhooks;
`BackendTrafficPolicy` for rate limits and timeouts; `ClientTrafficPolicy` for body size and TLS. WebSocket
upgrade and SSE (no buffering, long idle timeout) on `/rt/*`.

Locally the same routing table is expressed as a static Envoy config
(`infrastructure/envoy/envoy.dev.yaml`), and the Vite dev server proxies to it.

## 4. Delivery

- **CI** (`.github/workflows/ci.yml`, checked with actionlint): Rust format, clippy `-D warnings` and the full
  test suite against the Compose data stores; RustSec advisories (cargo-deny); web lint, typecheck, tests and
  build; Helm lint for both value sets plus Envoy's own validation of the generated gateway config; Terraform
  format and validate; then all images built with one builder, scanned with Trivy (fails on fixable high and
  critical findings), and pushed to GHCR tagged with the commit on `main`. All jobs passed on GitHub on
  2026-11-14 (run 43604095879), including the push of all thirteen images. The first runs found two problems,
  both fixed: librdkafka 2.12 needs curl's headers to compile even with curl off, and images built by a separate
  buildx container builder were rejected by GHCR (`unknown blob`), so the job uses the engine's own builder.
  On `main`, every pushed image then gets a CycloneDX SBOM (Trivy), a keyless cosign signature and the SBOM as a
  signed attestation, all bound to the image digest and verified in the same job against the identity of
  this workflow on `main` (`cosign verify --certificate-identity-regexp …/ci.yml@refs/heads/main
  --certificate-oidc-issuer https://token.actions.githubusercontent.com`). The SBOMs are also kept as a
  workflow artifact for 30 days. Nothing in the cluster enforces signatures yet (no admission policy).
  The chart job also checks the rendered manifests with kubeconform and runs the alert rule tests.
- **CD (Argo CD)**: `infrastructure/argocd` holds the add-ons (External Secrets Operator 0.10.7 with a
  `ClusterSecretStore` usable only from the `meridian` namespace, KEDA 2.16.0) and an `AppProject` that may
  deploy only this repository's chart into `meridian`, with the `meridian-staging` Application syncing
  `values-staging.yaml` from `main` automatically (prune, self-heal, retries). Helm hooks become PreSync steps.
  A build reaches staging when its tag is committed to `values-staging.yaml`. Checked with kubeconform, never
  applied (no cluster). **Not built:** Argo Rollouts canaries (5 % → 25 % → 100 % gated on the SLO burn rates),
  automatic tag promotion.
- **Migrations**: a pre-sync Job per service runs `sqlx migrate run` as the owner role. Migrations are
  backwards-compatible with the previous release (expand/contract) so a rollback of the code never needs a
  rollback of the schema.
- **Rollback**: `argocd app rollback` / Rollouts abort. Schema is not rolled back; see above.

## 5. Infrastructure as code

`infrastructure/terraform` (AWS, Terraform ≥ 1.10, validated against the AWS provider; never applied):

| Module | What it creates |
|---|---|
| `network` | VPC over three zones, public and private subnets, one NAT gateway per zone |
| `cluster` | EKS with private endpoint, KMS-encrypted Kubernetes secrets, control-plane audit logs, a managed node group |
| `postgres` | RDS PostgreSQL 17, Multi-AZ, KMS-encrypted, TLS forced, 14 days point-in-time recovery, deletion protection, managed admin password |
| `kafka` | MSK, three brokers, TLS in transit and at rest, SASL/SCRAM credentials in Secrets Manager, RF 3 / `min.insync.replicas` 2 |
| `redis` | ElastiCache Redis, primary and replica in two zones, TLS and auth token (`rediss://` URL in Secrets Manager) |
| `opensearch` | Three zone-aware data nodes in the VPC, encryption everywhere, HTTPS only, fine-grained access control |
| `storage` | Attachments bucket: private, versioned, KMS, TLS-only policy, old versions expire after 30 days |

`environments/staging` composes them with one KMS key; state in S3 with native locking (`backend.hcl`).
`environments/staging/secrets.tf` creates one KMS-encrypted Secrets Manager entry per chart Secret
(`meridian-staging/meridian-shared`, `…/meridian-<service>`, `…/meridian-<service>-migrate`; values are
written out of band, so none is in Terraform state) and gives the External Secrets Operator's service account,
through EKS Pod Identity, a role that may read exactly those entries.
Not built: a production environment (same modules, larger sizes), per-service database roles (a bootstrap Job
creating the owner/app roles per service, as `infrastructure/postgres/init` does locally), ClickHouse (ClickHouse
Cloud or the Altinity operator), Pod Identity roles for S3 access (today: access keys in a Secret), CDN for the
web app, DNS and certificates.

## 6. Regions and data residency

Start: one region, multi-AZ. The tenant registry already records `region` and services are deployed per region
with a region-local data plane, so adding a region is "deploy the stack again + global routing by tenant".
No active-active writes: a tenant lives in exactly one region. Identity (global accounts) is the one dataset
that needs a cross-region strategy before a second region opens — an open design item.

## 7. Backup and recovery

Targets (not yet validated in AWS): RPO ≤ 5 min, RTO ≤ 60 min.
PostgreSQL: RDS automated backups with 14 days of point-in-time recovery (Terraform); a monthly restore drill.
`make restore-drill` (`tests/restore/run.sh`) is that drill for logical backups: it backs up every service
database with `pg_dump`, restores each into a fresh database, and checks exact row counts of every table, the
applied migrations, that the same tables are under forced row-level security with the same policies, and that
the runtime role sees nothing without a tenant and exactly one workspace's rows with one. Run on 2026-11-16
against the local data (10 databases, 6 MiB of backups, 136,000 rows in issue_db): all checks passed, backup and
restore of everything in 24 s. Restoring an RDS snapshot or point in time has not been drilled (nothing is
provisioned), so the RPO and RTO above remain targets.
Kafka: replication factor 3, `min.insync.replicas=2`; it is not a backup — projections rebuild from PostgreSQL.
OpenSearch / ClickHouse: rebuildable; snapshots only shorten recovery.
Object storage: versioning + cross-zone replication.
Runbooks live in `docs/operations/` as they are written and rehearsed.
