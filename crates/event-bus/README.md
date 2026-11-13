# event-bus

Moves events from each service's transactional `outbox` to Kafka, and runs consumers on the other side.

**Relay** (`spawn_relay(service, pools)`). One loop per database the service owns. Each pass takes
`pg_try_advisory_xact_lock` so only one instance relays a database at a time, reads up to 200 unpublished rows
ordered by `created_at, id`, produces them concurrently with an idempotent producer (`acks=all`, lz4), keyed by
aggregate id so one aggregate's events stay in order on one partition, and marks the rows published in the same
transaction. If a row fails, every later row with the same key is held back until it succeeds. A sweep deletes
published outbox rows after 7 days and inbox rows after 14 days.

**Consumers** (`run_consumer(settings, name, topics, handler)`). Offsets are committed by hand after the handler
returns. `HandlerError::Transient` (dependency down, timeout, deadlock) is retried 3 times with backoff;
`Permanent` (or a transient one that keeps failing) parks the message on `<topic>.dlq` with `x-error`,
`x-consumer`, `x-attempts` and `x-original-topic` headers and moves on. `Event::parse` validates the envelope and
refuses any event without a `tenant_id` except on the identity topic, so a handler always gets its tenant from the
envelope, never from payload fields. Handlers that write to a database call `inbox::claim` inside their own
transaction to ignore redeliveries.

| Variable | Default |
|---|---|
| `KAFKA_BROKERS` | unset: no relay, no consumers |
| `OUTBOX_RELAY` | `kafka` when brokers are set, else `hold` (events stay in the outbox) |

Topics are created by `infrastructure/kafka/create-topics.sh` (auto-create is off): `platform.<domain>.v1` and a
`.dlq` for each, 6 partitions, 7-day retention.

**Tests.** `cargo test -p event-bus` needs Kafka and PostgreSQL from `make infra`.

**Not built yet.** A DLQ replay tool; consumer lag metrics beyond the client's own; exactly-once across Kafka and
the database (delivery is at least once, made idempotent by the inbox or by idempotent writes).
