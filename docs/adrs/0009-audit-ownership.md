# ADR-0009: Audit store hosted by user-service initially

Status: accepted (implemented in Phase 6)

**Decision.** All services emit `audit.recorded`; a consumer inside user-service appends to a hash-chained, append-only `audit_log`. No sixteenth service is created now.

**Consequences.** If write volume or retention requirements diverge from user-service's, the consumer and table move to their own deployment unit with no change to producers or to the read API path.

**Implementation (Phase 6).** Rather than every service emitting `audit.recorded` for every action, the
`user-service.audit` consumer reads the existing domain topics (organization, project, workflow, agile, issue)
and derives entries from the events that matter to an admin, so producers did not change. `audit.recorded` is
used only where no domain event says enough: project access changes, which project-service records because
permission-service only sees a service caller. Its payload is validated (action shape, lengths, at most 20
details, no control characters); invalid ones are dead-lettered, never stored.
