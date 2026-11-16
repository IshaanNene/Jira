# Event architecture

## 1. Envelope

Every event on Kafka is this JSON envelope (JSON Schema registered in Schema Registry, `BACKWARD` compatibility):

```json
{
  "event_id": "0195c1e2-...",            // UUIDv7, the idempotency key
  "event_type": "issue.created",
  "schema_version": 1,
  "tenant_id": "…",
  "aggregate_type": "issue",
  "aggregate_id": "…",
  "aggregate_version": 7,                 // lets consumers drop stale / out-of-order events
  "occurred_at": "2026-11-11T09:00:00Z",
  "correlation_id": "…",                  // the originating request id
  "causation_id": "…",                    // the event or command that caused this one (null at the edge)
  "actor": { "type": "user|system|automation", "id": "…" },
  "payload": { }
}
```

Rust types live in `crates/event-contracts`; payload structs are versioned (`IssueCreatedV1`). A consumer must
ignore unknown fields and must reject an envelope whose `tenant_id` is missing or whose payload fails validation
(→ DLQ, never a crash loop).

Payloads grow only by optional fields. Example: `issue.transitioned` gained `from_category` and `to_category`
(Phase 7, for cycle time); events written before carry neither, and analytics fills the target category from the
issue's current state when it can.

## 2. Topics

One topic per aggregate type, so that all events of one aggregate are totally ordered.

| Topic | Event types | Key | Producers |
|---|---|---|---|
| `platform.identity.v1` | `identity.user_registered`, `identity.login_succeeded`, `identity.login_failed`, `identity.session_revoked`, `identity.email_changed` | `account_id` | identity (user-service keeps profile addresses from `identity.email_changed`) |
| `platform.tenant.v1` | `tenant.provisioned`, `tenant.placement_changed`, `tenant.suspended` | `tenant_id` | tenant |
| `platform.organization.v1` | `organization.created`, `organization.updated`, `organization.two_step_changed`, `user.invited`, `membership.changed`, `team.changed` | `tenant_id` | user |
| `platform.project.v1` | `project.created`, `project.updated`, `project.archived`, `version.released` | `tenant_id:project_id` | project |
| `platform.issue.v1` | `issue.created`, `issue.updated`, `issue.deleted`, `issue.assigned`, `issue.transitioned`, `issue.mentioned`, `comment.created`, `comment.updated`, `comment.deleted` | `tenant_id:issue_id` | issue |
| `platform.workflow.v1` | `workflow.updated`, `workflow.published` | `tenant_id:workflow_id` | workflow |
| `platform.agile.v1` | `sprint.created`, `sprint.started`, `sprint.completed`, `board.updated`, `issue.ranked` | `tenant_id:board_id` | agile |
| `platform.permission.v1` | `permission.updated` | `tenant_id` | permission |
| `platform.automation.v1` | `automation.triggered`, `automation.executed`, `automation.failed` | `tenant_id:rule_id` | automation |
| `platform.notification.v1` | `notification.requested`, `notification.delivered` | `tenant_id:user_id` | any / notification |
| `platform.audit.v1` | `audit.recorded` (only where no domain event says enough; today project access changes) | `tenant_id` | project |
| `platform.search.v1` | `filter.digest` (a subscribed saved filter ran; carries only issues the recipient could see) | `tenant_id:subscription_id` | search |
| `<topic>.dlq` | failed envelopes + error metadata headers | same | consumers |

Keying by `tenant:aggregate` preserves per-aggregate order and spreads tenants across partitions. Comments ride
on the issue topic, keyed by the issue, so "comment added" can never overtake "issue created".

## 3. Transactional outbox

```
BEGIN
  INSERT/UPDATE business rows
  INSERT INTO outbox (id, tenant_id, topic, partition_key, payload)   -- the envelope
COMMIT
```

`event_contracts::outbox::enqueue(&mut tx, event)` is the only way services emit events, so there is no code
path that publishes without committing or commits without publishing.

**Relay** (as built in `crates/event-bus`; a Tokio task in each service, one loop per owned database):

1. `BEGIN`; `pg_try_advisory_xact_lock(…)` — if another instance holds it, this one stays idle (one active relay
   per database, so ordering does not depend on row locks)
2. `SELECT … FROM outbox WHERE published_at IS NULL ORDER BY created_at, id LIMIT 200`
3. produce the batch concurrently to Kafka (`acks=all`, `enable.idempotence=true`, lz4), keyed by the partition
   key, and await every delivery report
4. mark delivered rows `published_at = now()`; for failed rows increment `attempts` and set `last_error`. A row is
   only marked if no earlier row with the same key in the batch failed, so a key's events are never published
   out of order
5. commit; poll again at once after a full batch, every 250 ms when idle, with backoff up to 10 s on errors

Guarantee: **at-least-once**, ordered per partition key. An hourly sweep deletes published rows after 7 days and
inbox rows after 14 days. Rows that keep failing stay in the outbox (never silently dropped); alerting on
`attempts` is not built yet.

`OUTBOX_RELAY=kafka` is the default when `KAFKA_BROKERS` is set; `hold` leaves events in the outbox. Enabling the
relay in Phase 5 drained every event held since Phase 1.

## 4. Consumers

```
poll -> validate envelope -> BEGIN
          INSERT INTO inbox(consumer, event_id) ON CONFLICT DO NOTHING   -- 0 rows => already done, skip
          apply side effect (guarded by aggregate_version where it is a projection)
        COMMIT -> commit offset
```

- **Transient errors** (a data store or service is down) are retried in place until they succeed, with backoff
  from 200 ms doubling to 30 s: the consumer waits instead of skipping ahead, so search, analytics and
  notifications never silently miss a change made during an outage. Each retry counts in `events_retried_total`;
  the `ConsumerWaitingOnDependency` alert fires after 10 minutes. (Until Phase 8 the consumer gave up after three
  attempts, about 0.6 s, and dead-lettered the event; the failure tests showed that a short OpenSearch outage lost
  every change from search.)
- **Permanent errors** (invalid envelope, payload against its contract, a handler's permanent error) go to
  `<topic>.dlq` with headers `x-error`, `x-consumer`, `x-attempts`, `x-original-topic`, and the consumer moves
  on. Envelopes without a `tenant_id` are rejected on every topic except identity.
- Side effects that leave the platform (email, webhooks, Slack) cannot be made exactly-once. They use a
  delivery table keyed by `(event_id, target)` and send an idempotency key to providers that support one.
  We document at-least-once delivery to integrators and sign every webhook with a delivery id.
- **Replay**: reset a consumer group's offsets. A DLQ re-drive tool is specified but not built.
  Projections (search, analytics, read models) are rebuildable from offset 0 or from the owning service's
  gRPC stream when the topic has been compacted/expired.
- **Loop prevention** for automation: the envelope's causation chain depth is carried in a header; automation
  refuses to act on events whose chain already contains the same rule or exceeds depth 5.

## 5. Schema evolution

Adding optional fields: allowed, same `schema_version`. Removing/renaming/changing meaning: new payload struct,
new `schema_version`, producers dual-publish until all consumers have upgraded. Contract tests in
`crates/event-contracts/tests` deserialize golden fixtures of every published version.
