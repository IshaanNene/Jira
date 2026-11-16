-- Project automation: declarative rules (a trigger, conditions and actions, validated by the
-- service against a closed set of types; nothing in them is ever executed as code).
CREATE TABLE automation_rules (
    id          uuid PRIMARY KEY,
    tenant_id   uuid NOT NULL,
    project_id  uuid NOT NULL,
    name        text NOT NULL CHECK (char_length(name) BETWEEN 1 AND 100),
    enabled     boolean NOT NULL DEFAULT true,
    definition  jsonb NOT NULL,
    version     bigint NOT NULL DEFAULT 1,
    created_by  uuid NOT NULL,
    created_at  timestamptz NOT NULL DEFAULT now(),
    updated_at  timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX automation_rules_project_idx ON automation_rules (tenant_id, project_id, created_at);

-- What each rule did, newest kept (the service trims to the latest 100 per rule).
CREATE TABLE automation_runs (
    id          uuid PRIMARY KEY,
    tenant_id   uuid NOT NULL,
    rule_id     uuid NOT NULL REFERENCES automation_rules (id) ON DELETE CASCADE,
    issue_id    uuid NOT NULL,
    issue_key   text NOT NULL,
    actor_id    uuid NOT NULL,
    outcome     text NOT NULL CHECK (outcome IN ('applied', 'no_change', 'failed')),
    detail      text,
    created_at  timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX automation_runs_rule_idx ON automation_runs (tenant_id, rule_id, created_at DESC);

DO $$
DECLARE t text;
BEGIN
    FOREACH t IN ARRAY ARRAY['automation_rules', 'automation_runs'] LOOP
        EXECUTE format('ALTER TABLE %I ENABLE ROW LEVEL SECURITY', t);
        EXECUTE format('ALTER TABLE %I FORCE ROW LEVEL SECURITY', t);
        EXECUTE format(
            'CREATE POLICY tenant_isolation ON %I USING (tenant_id = app_tenant_id()) WITH CHECK (tenant_id = app_tenant_id())', t);
    END LOOP;
END $$;
