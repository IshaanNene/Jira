# ADR-0006: Issue numbers from a per-project counter row

Status: accepted

**Decision.** `UPDATE project_issue_counters SET next_number = next_number + 1 WHERE tenant_id=$1 AND project_id=$2 RETURNING next_number - 1` in the same transaction as the issue insert, backed by `UNIQUE (tenant_id, project_id, number)`.

**Alternatives.** PostgreSQL sequence per project (DDL per project, not transactional → gaps, awkward under RLS and tenant moves); `MAX(number)+1` (races).

**Consequences.** Concurrent creators in one project serialise on a row lock held for the insert's duration (sub-millisecond). Bulk import reserves a range in one update.
