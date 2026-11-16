-- One vote per person per issue: how much people want it done.
CREATE TABLE votes (
    tenant_id  uuid NOT NULL,
    issue_id   uuid NOT NULL REFERENCES issues (id) ON DELETE CASCADE,
    user_id    uuid NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, issue_id, user_id)
);
ALTER TABLE votes ENABLE ROW LEVEL SECURITY;
ALTER TABLE votes FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON votes USING (tenant_id = app_tenant_id()) WITH CHECK (tenant_id = app_tenant_id());
