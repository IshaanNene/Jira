# ADR-0004: Transactional outbox from Phase 1; Kafka relay enabled in Phase 5

Status: accepted

**Decision.** Every service database has `outbox` and `inbox` tables from its first migration and all writes enqueue events through `event_contracts::outbox::enqueue` in the business transaction. Until Kafka is introduced the relay runs in `hold` mode.

**Why.** Retro-fitting event emission into existing write paths is where dual-write bugs come from. Holding events costs a table.

**Consequences.** Outbox tables grow until Phase 5; first enable drains history, which consumers must tolerate (they are idempotent by design).
