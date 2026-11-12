# ADR-0009: Audit store hosted by user-service initially

Status: accepted (revisit at Phase 6)

**Decision.** All services emit `audit.recorded`; a consumer inside user-service appends to a hash-chained, append-only `audit_log`. No sixteenth service is created now.

**Consequences.** If write volume or retention requirements diverge from user-service's, the consumer and table move to their own deployment unit with no change to producers or to the read API path.
