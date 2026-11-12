# ADR-0008: Kafka is durable, Redis is fanout plus a short replay buffer

Status: accepted

**Decision.** The realtime consumer writes each client-visible event to a capped Redis Stream per topic and publishes a Pub/Sub notification for cross-instance fanout. Stream ids are the client cursor. Aged-out cursor → `resync` → client refetches over REST.

**Why.** Pub/Sub alone loses events for disconnected clients; replaying from Kafka per client is too expensive and needs per-topic indexing.

**Consequences.** Redis loss costs a resync, never data. Collaborative text editing is out of scope until separately designed.
