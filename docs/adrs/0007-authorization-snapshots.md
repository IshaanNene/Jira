# ADR-0007: Local policy evaluation from versioned snapshots

Status: accepted

**Decision.** permission-service is the source of truth for roles and schemes. Other services evaluate permissions locally from a per-(tenant,user) snapshot cached in-process and in Redis, keyed by policy version `pv`. Organization-level roles ride in the token.

**Why.** A remote `Check` per request (or per row in a list) makes permission-service a latency and availability bottleneck for everything.

**Consequences.** Bounded staleness (≤30 s, usually less via `permission.updated`). Evaluation logic lives in `auth-middleware` — it is generic policy evaluation, not business logic, so it is an acceptable shared crate. Phase order changed: permission-service core is built in Phase 2.
