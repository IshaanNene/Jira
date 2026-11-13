-- Analytics facts. Derived from events and issue-service, never the source of truth:
-- dropping the database and replaying the topics rebuilds it. Statements are idempotent and
-- run in order, split on lines that end with a semicolon.

-- One row per event that matters to a chart. Replays insert the same event_id again; the
-- engine keeps one copy, and queries read with FINAL.
CREATE TABLE IF NOT EXISTS issue_events (
    tenant_id     UUID,
    event_id      UUID,
    event_type    LowCardinality(String),
    occurred_at   DateTime64(3, 'UTC'),
    project_id    UUID,
    issue_id      UUID,
    sprint_id     UUID,
    actor_id      String,
    from_status   String,
    to_status     String,
    from_category LowCardinality(String),
    to_category   LowCardinality(String),
    name          String,
    completed     UInt32,
    incomplete    UInt32,
    ingested_at   DateTime64(3, 'UTC') DEFAULT now64(3)
) ENGINE = ReplacingMergeTree(ingested_at)
PARTITION BY toYYYYMM(occurred_at)
ORDER BY (tenant_id, event_type, event_id);

-- The current state of every issue, as issue-service last reported it. The highest version
-- wins; a deleted issue is a row with is_deleted = 1, which FINAL hides.
CREATE TABLE IF NOT EXISTS issue_state (
    tenant_id       UUID,
    issue_id        UUID,
    project_id      UUID,
    key             String,
    type            LowCardinality(String),
    status          String,
    status_category LowCardinality(String),
    priority        LowCardinality(String),
    assignee_id     String,
    created_at      DateTime64(3, 'UTC'),
    resolved_at     Nullable(DateTime64(3, 'UTC')),
    updated_at      DateTime64(3, 'UTC'),
    version         Int64,
    is_deleted      UInt8
) ENGINE = ReplacingMergeTree(version, is_deleted)
ORDER BY (tenant_id, issue_id);

-- Defence in depth for dashboard reads: the reader only ever sees the rows of the tenant
-- the service names in the SQL_tenant_id setting. Every query also filters on tenant_id.
CREATE ROW POLICY IF NOT EXISTS tenant_isolation ON issue_events
    USING tenant_id = toUUID(getSetting('SQL_tenant_id')) TO {reader};
CREATE ROW POLICY IF NOT EXISTS tenant_isolation ON issue_state
    USING tenant_id = toUUID(getSetting('SQL_tenant_id')) TO {reader};
GRANT SELECT ON {database}.* TO {reader};
GRANT INSERT, SELECT ON {database}.* TO {writer};
