# ADR-0005: Custom field values in a JSONB column, definitions in tables

Status: accepted

**Context.** Administrators define fields at runtime; the schema must not hardcode them.

**Decision.** `custom_fields` + `custom_field_contexts` hold definitions and applicability. Values are stored in `issues.custom_fields` as `{field_id: value}` and validated in issue-service against the definition (type, required, options, bounds). GIN index for containment; OpenSearch for rich querying.

**Alternatives.** EAV table (joins per field, hard to keep one issue version atomic); column-per-field DDL at runtime (locks, migrations by users).

**Consequences.** One row write per issue update, atomic with the version check. Range queries on custom fields in PostgreSQL need expression indexes, created only for fields flagged searchable.
