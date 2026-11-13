# System design

How the mechanisms in [ARCHITECTURE.md](ARCHITECTURE.md) actually work.

## 1. Request lifecycle

```
Browser ──HTTPS──> Envoy Gateway ──> service (Axum)
                    │ request-id, rate limit,          │ 1. request-id / trace context
                    │ size limit, coarse JWT check     │ 2. verify JWT (JWKS cache)  -> AuthContext{user, tenant, org, roles, policy_version}
                                                       │ 3. validate input (typed DTO + validator)
                                                       │ 4. authorize (local policy snapshot)
                                                       │ 5. tenant-scoped tx: SET LOCAL app.tenant_id
                                                       │ 6. write business rows + outbox row, COMMIT
                                                       │ 7. respond; relay publishes outbox -> Kafka
```

Every step runs under an explicit timeout (`tower::timeout`, default 10 s for HTTP, 2 s for gRPC, 5 s statement
timeout in PostgreSQL).

## 2. Tenancy and database routing

### Tenant identity is established once, by the identity service

1. The user authenticates → receives an **account token** (no tenant).
2. The client asks for an organization → `POST /api/v1/auth/token/exchange {organization_id}`.
3. Identity asks user-service over gRPC whether this user is an active member, and what the org's `tenant_id`
   and role are. Only then does it sign an **organization token** carrying `tid`, `org`, `roles`, `pv`.
4. Every downstream service takes the tenant **only** from the verified `tid` claim. Path parameters such as
   `/organizations/{id}` are compared against the claim and rejected on mismatch; they never select data.

Service-to-service calls carry the same tenant in signed gRPC metadata (service token + propagated user context).
Kafka events carry `tenant_id` in the envelope; consumers validate the envelope before touching a database.

### Three tiers, one code path

```rust
let mut tx = router.begin(tenant_id).await?;   // picks the pool, opens tx, sets app.tenant_id
```

`tenant_context::DbRouter` holds a map `database_destination -> PgPool` and a cached copy of the tenant's
placement record from the tenant registry (TTL 60 s, invalidated by `tenant.placement_changed`).

| Tier | `database_destination` | Isolation |
|---|---|---|
| standard | `shared/<cluster>` | shared tables, `tenant_id` column, **RLS** |
| business | `dedicated-db/<cluster>/<db>` | database per tenant on shared cluster; own pool with connection cap and `statement_timeout`; RLS stays on |
| enterprise | `dedicated-cluster/<cluster>` | cluster per tenant, region-pinned, own backup policy; RLS stays on |

`database_destination` is a **logical name**. It is resolved to a connection string from the service's own
secret store (`DATABASE_URL__<DESTINATION>`), never from the registry row and never from a request. A compromised
registry row can therefore not point a service at an attacker-controlled database.

RLS remains enabled on dedicated tiers. It costs nothing measurable and means a routing bug degrades to
"no rows" rather than "another tenant's rows".

### Row-level security pattern (every tenant-owned table)

```sql
ALTER TABLE t ENABLE ROW LEVEL SECURITY;
ALTER TABLE t FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON t
  USING (tenant_id = app_tenant_id()) WITH CHECK (tenant_id = app_tenant_id());
-- app_tenant_id(): NULLIF(current_setting('app.tenant_id', true), '')::uuid
-- (a pooled connection reports '' after a previous transaction's SET LOCAL ends)
```

Services connect as a role that is **not** the table owner and has no `BYPASSRLS`. Migrations run as a separate
owner role. With no tenant set, `app_tenant_id()` is NULL and every policy evaluates false.
`SET LOCAL` scopes the setting to the transaction, so pooled connections cannot leak a tenant between requests.

Every tenant-owned index leads with `tenant_id`. Every cache key is prefixed `t:{tenant_id}:`.

### Tenant migration between tiers

State machine in the registry: `active → migrating(read_only) → active`. Copy → verify row counts and checksums
→ flip `database_destination` and bump `configuration_version` → routers pick up the change → old data retained
for the rollback window. Writes are rejected with `503 tenant_migrating` during the freeze. Online (dual-write)
migration is deliberately out of scope until there is a measured need.

## 3. Identifiers and concurrency

- All primary keys are UUIDv7 (time-ordered → index locality).
- **Issue keys** (`PAY-101`): `project_issue_counters(tenant_id, project_id, next_number)`; allocation is
  `UPDATE ... SET next_number = next_number + 1 RETURNING` inside the issue-insert transaction. The row lock
  serialises creators per project; a rollback returns the number. Gaps are impossible, duplicates are prevented
  by `UNIQUE (tenant_id, project_id, number)`. A sequence per project was rejected (ADR-0006).
- **Optimistic concurrency**: every mutable aggregate has `version bigint`. Updates are
  `UPDATE ... WHERE id = $1 AND version = $2`, clients send `If-Match: "<version>"`; mismatch → `409` with the
  current representation so the UI can show a field-level merge.
- **Ranking** (backlog/board order): LexoRank-style string ranks; reorder touches one row. A background
  rebalance runs when rank length exceeds a threshold.

## 4. Events

See [EVENT_ARCHITECTURE.md](EVENT_ARCHITECTURE.md). In one paragraph: writes insert into `outbox` in the same
transaction; a relay claims rows with `FOR UPDATE SKIP LOCKED`, publishes with key `tenant_id:aggregate_id`,
marks them published; consumers record `(consumer, event_id)` in an inbox table in the same transaction as their
side effect; poison messages go to `<topic>.dlq` after bounded retries.

## 5. Real-time

Two endpoints on realtime-service, both authenticated with the organization token:

- `GET /rt/ws` — WebSocket. Client sends `subscribe {topic, cursor?}`. Topics are a closed set:
  `issue:{id}`, `project:{id}`, `board:{id}`, `user:{id}:notifications`, `presence:{resource}`.
  The tenant is prepended server-side (`t:{tid}:...`), so a client cannot name another tenant's topic.
  Subscription to a resource is authorised against the permission snapshot.
- `GET /rt/sse/jobs/{job_id}` — SSE for one long-running job; `Last-Event-ID` resumes.

```
Kafka (durable) ──> realtime consumer (any instance, consumer group)
                      └─ XADD t:{tid}:{topic}  (Redis Stream, capped ~1000 entries / 10 min)   <- replay buffer
                      └─ PUBLISH rt:{shard}                                                     <- cross-instance fanout
each gateway instance: SUBSCRIBE rt:* -> local subscription table -> bounded per-connection queue -> socket
```

- **Sequence numbers** are the Redis Stream ids. On reconnect the client sends its last id; the gateway replays
  from the stream. If the cursor has aged out it answers `resync` and the client refetches via REST. Redis is
  therefore a *cache of recent events*, never the record: losing Redis costs a resync, not data.
- **Backpressure**: each connection has a bounded queue (256). When full, the connection is closed with code
  `4008 slow_consumer`; the client reconnects with its cursor. The gateway never buffers unboundedly.
- **Limits**: connections per user (10) and per tenant (tier-based), subscriptions per connection (50),
  heartbeat every 25 s, idle timeout 60 s. On SIGTERM the gateway sends `going_away` and drains.
- No connection state lives only in one instance: presence is a Redis hash with TTL refreshed by heartbeats.

## 6. Search

- Query text → **lexer → parser → AST → validator → planner** (crate `services/search-service/src/ql`, no I/O,
  fuzz-tested). The validator resolves field names against a registry (system fields + the tenant's custom
  fields) and type-checks operators. The planner emits either an OpenSearch bool query (JSON built from typed
  structs) or a parameterised SQL plan (`sqlx::QueryBuilder` with bind parameters only) for the PostgreSQL
  fallback. Query text is never interpolated into either.
- **Authorization is two-layered.** The planner always adds `tenant_id` and the caller's visible project ids
  and security levels as filters. Then the hits (ids only) are **re-checked** against the permission snapshot
  before hydration, so a stale or corrupt index cannot leak a document.
- Index per tenant tier: shared index with routing by `tenant_id` for standard; index-per-tenant for
  business/enterprise. Indexing is asynchronous from `issue.*` events, versioned with
  `version_type=external` using the aggregate version, which makes out-of-order and duplicate events harmless.
- `reindex` (full rebuild from issue-service gRPC stream) and `reconcile` (compare versions, repair drift) are
  operator commands on the search-service binary.

## 7. Analytics

`issue.*`, `sprint.*` and `project.deleted` events → analytics ingester (consumer group `analytics-service`) →
two ClickHouse tables:

- `issue_events` — one row per event (creations, transitions with status categories, sprint start/complete),
  ReplacingMergeTree keyed by `(tenant_id, event_type, event_id)`, so redelivery and replays are idempotent.
- `issue_state` — each issue's current state, fetched from issue-service `GetDocuments` on every issue event and
  versioned by the issue's version (`ReplacingMergeTree(version, is_deleted)`); a deletion is written with the
  highest possible version so a late event cannot bring the issue back.

Charts are a closed set of kinds (created vs resolved, cycle time, breakdown, velocity), each compiled to fixed SQL
with typed query parameters; nothing from the client becomes SQL text. Queries run as a read-only ClickHouse user
whose row policy is `tenant_id = getSetting('SQL_tenant_id')`; the service sets that from the verified token on
every query, so a missing or wrong filter in SQL still cannot read another tenant. Requested projects are
intersected with the caller's browsable projects. Dashboards and their widget definitions live in PostgreSQL
(`analytics_db`, forced RLS). Small operational reports that must be exact and current (active sprint burndown)
are served by agile-service from its own tables. Materialized views and rollups are deferred until volume needs them.

## 8. Reliability

| Failure | Behaviour |
|---|---|
| PostgreSQL down / failing over | pool acquire timeout 3 s → `503` + `Retry-After`; readiness fails; no in-process retry of writes |
| Kafka down | writes still commit (outbox); relay backs off; lag alert. Nothing user-facing fails |
| Redis down | permission cache falls back to in-process snapshot then to gRPC; rate limiter fails open at service level (gateway still limits); realtime degrades to polling |
| OpenSearch down | search falls back to the PostgreSQL plan for structured queries; text search returns `503 search_degraded` |
| Downstream gRPC slow | deadline → circuit breaker opens after 5 consecutive failures, half-open after 10 s |
| Duplicate event | inbox dedupe |
| Out-of-order event | aggregate version check; older version ignored |
| Worker crash mid-event | offset not committed → redelivered → inbox makes it safe |
| Partial multi-service operation | sagas with explicit states (e.g. org provisioning), reaper for stuck states |

Retries are always bounded (max 3, exponential backoff with full jitter) and only for idempotent calls.
RPO/RTO are **targets, not claims**: RPO ≤ 5 min (WAL archiving / PITR), RTO ≤ 60 min. They become claims only
after a restore drill is recorded in `docs/operations/`.

## 9. Risks, bottlenecks, trade-offs

| # | Risk / bottleneck | Mitigation | Residual |
|---|---|---|---|
| 1 | Issue-key counter row is a per-project write hotspot | Single-row update is ~µs; bulk import allocates ranges | Very large imports serialise per project |
| 2 | Custom fields as JSONB limit indexing | GIN on `custom_fields`; per-field expression indexes created on demand for fields marked "searchable"; heavy querying goes to OpenSearch | JSONB updates rewrite the row |
| 3 | Permission checks on every request | Signed claims for coarse roles + versioned snapshot per (tenant, user) cached in-process and Redis; invalidation by `permission.updated` | Up to TTL (30 s) staleness on revoke unless the event arrives; session revocation is separate and immediate |
| 4 | Access token cannot be revoked instantly | 10-minute lifetime + revocation list in Redis keyed by session id, checked by middleware | If Redis is down, revocation waits for expiry |
| 5 | Eventual consistency confuses users (search/board lag) | Read-your-writes: mutations return the new entity and the UI patches the cache; search screen shows index lag | Cross-user lag up to seconds |
| 6 | 15 services is a lot of operational surface for a young product | Shared bootstrap crate, identical Helm sub-chart, uniform health/metrics | Real cost; accepted because it was a locked decision |
| 7 | Shared Kafka topics across tenants → noisy neighbour | Partition key includes tenant; per-tenant quotas in automation; enterprise tier can get dedicated topics | Not built until measured |
| 8 | Redis Pub/Sub is at-most-once | Stream replay buffer + REST resync | Covered by design |
| 9 | RLS misconfiguration silently returns nothing | Integration test per service asserts cross-tenant read/write fails and same-tenant succeeds; CI check that every table with `tenant_id` has FORCE RLS | — |
| 10 | Scale targets (1M users, 50k concurrent) are unproven | k6 scenarios in Phase 8; results recorded in the roadmap | Until then they are design targets only |

Explicit non-goals for now: active-active multi-region writes, online tenant migration, real-time collaborative
rich-text editing (CRDT). Each needs its own design before code.
