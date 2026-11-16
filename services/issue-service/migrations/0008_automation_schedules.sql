-- More automation: scheduled rules, and rules that comment on or move issues.
--
-- A rule now remembers who it runs as when nothing triggers it: the administrator who last
-- saved it (`actor_id`), in their workspace (`organization_id`, from their signed token).
-- A scheduled run checks again, at run time, that this person is still an active member who
-- administers the project.

ALTER TABLE automation_rules ADD COLUMN actor_id uuid;
ALTER TABLE automation_rules ADD COLUMN organization_id uuid;

-- Migrations run without a tenant, so lift the policy for this backfill only.
ALTER TABLE automation_rules NO FORCE ROW LEVEL SECURITY;
ALTER TABLE automation_rules DISABLE ROW LEVEL SECURITY;
UPDATE automation_rules SET actor_id = created_by WHERE actor_id IS NULL;
ALTER TABLE automation_rules ENABLE ROW LEVEL SECURITY;
ALTER TABLE automation_rules FORCE ROW LEVEL SECURITY;

ALTER TABLE automation_rules ALTER COLUMN actor_id SET NOT NULL;

-- A scheduled run that cannot start (its owner left, say) is recorded without an issue.
ALTER TABLE automation_runs ALTER COLUMN issue_id DROP NOT NULL;
ALTER TABLE automation_runs ALTER COLUMN issue_key DROP NOT NULL;

-- A comment an automation rule posted, on its author's behalf, names the rule.
ALTER TABLE comments ADD COLUMN rule_name text CHECK (char_length(rule_name) BETWEEN 1 AND 100);

-- When each scheduled rule runs next. Like `outbox`, a system queue drained across tenants by
-- the runner, so it is not row-level secured and holds no content; no API reads it.
CREATE TABLE automation_schedule (
    rule_id      uuid        PRIMARY KEY REFERENCES automation_rules (id) ON DELETE CASCADE,
    tenant_id    uuid        NOT NULL,
    next_run_at  timestamptz NOT NULL,
    leased_until timestamptz
);
CREATE INDEX automation_schedule_due_idx ON automation_schedule (next_run_at);
