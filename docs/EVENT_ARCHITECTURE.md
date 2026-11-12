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

## 2. Topics

One topic per aggregate type, so that all events of one aggregate are totally ordered.

| Topic | Event types | Key | Producers |
|---|---|---|---|
| `platform.identity.v1` | `identity.user_registered`, `identity.login_succeeded`, `identity.login_failed`, `identity.session_revoked` | `account_id` | identity |
| `platform.tenant.v1` | `tenant.provisioned`, `tenant.placement_changed`, `tenant.suspended` | `tenant_id` | tenant |
| `platform.organization.v1` | `organization.created`, `organization.updated`, `user.invited`, `membership.changed`, `team.changed` | `tenant_id` | user |
| `platform.project.v1` | `project.created`, `project.updated`, `project.archived`, `version.released` | `tenant_id:project_id` | project |
| `platform.issue.v1` | `issue.created`, `issue.updated`, `issue.deleted`, `issue.assigned`, `issue.transitioned`, `comment.created`, `comment.updated`, `comment.deleted` | `tenant_id:issue_id` | issue |
| `platform.workflow.v1` | `workflow.updated`, `workflow.published` | `tenant_id:workflow_id` | workflow |
| `platform.agile.v1` | `sprint.created`, `sprint.started`, `sprint.completed`, `board.updated`, `issue.ranked` | `tenant_id:board_id` | agile |
| `platform.permission.v1` | `permission.updated` | `tenant_id` | permission |
| `platform.automation.v1` | `automation.triggered`, `automation.executed`, `automation.failed` | `tenant_id:rule_id` | automation |
| `platform.notification.v1` | `notification.requested`, `notification.delivered` | `tenant_id:user_id` | any / notification |
| `platform.audit.v1` | `audit.recorded` | `tenant_id` | all |
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

**Relay** (a Tokio task in each service, or the same binary with `--role worker`):

1. `SELECT … FROM outbox WHERE published_at IS NULL ORDER BY created_at LIMIT 100 FOR UPDATE SKIP LOCKED`
2. produce to Kafka (`acks=all`, `enable.idempotence=true`), await delivery reports
3. `UPDATE outbox SET published_at = now()` for delivered rows; increment `attempts` + `last_error` for failed
4. commit; sleep with backoff when idle or failing; woken early by `LISTEN outbox_new`

Guarantee: **at-least-once**, ordered per partition key as long as a single relay batch holds a key (rows of one
key are published in `created_at` order and a failed row stops later rows of the same key in that batch).
Published rows are deleted after 7 days. Rows with `attempts > 20` raise an alert; they are never silently dropped.

The outbox table and writer exist from Phase 1. The Kafka relay is enabled in Phase 5 (`OUTBOX_RELAY=kafka`);
before that the relay mode is `hold` — events accumulate, unpublished, and are drained on first enable.

## 4. Consumers

```
poll -> validate envelope -> BEGIN
          INSERT INTO inbox(consumer, event_id) ON CONFLICT DO NOTHING   -- 0 rows => already done, skip
          apply side effect (guarded by aggregate_version where it is a projection)
        COMMIT -> commit offset
```

- Retry in place with backoff (3 attempts) for transient errors, then publish to `<topic>.dlq` with headers
  `x-error`, `x-consumer`, `x-attempts`, and move on. Order-sensitive projections pause the partition instead of
  skipping, and alert.
- Side effects that leave the platform (email, webhooks, Slack) cannot be made exactly-once. They use a
  delivery table keyed by `(event_id, target)` and send an idempotency key to providers that support one.
  We document at-least-once delivery to integrators and sign every webhook with a delivery id.
- **Replay**: reset a consumer group's offsets, or re-drive a DLQ with `scripts/development/replay-dlq`.
  Projections (search, analytics, read models) are rebuildable from offset 0 or from the owning service's
  gRPC stream when the topic has been compacted/expired.
- **Loop prevention** for automation: the envelope's causation chain depth is carried in a header; automation
  refuses to act on events whose chain already contains the same rule or exceeds depth 5.

## 5. Schema evolution

Adding optional fields: allowed, same `schema_version`. Removing/renaming/changing meaning: new payload struct,
new `schema_version`, producers dual-publish until all consumers have upgraded. Contract tests in
`crates/event-contracts/tests` deserialize golden fixtures of every published version.
