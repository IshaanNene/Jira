-- SCIM groups: the identity provider's groups and who is in them, and the workspace role an admin
-- gives each group. A provisioned member's role is the strongest role among their groups that
-- have one; `scim_users.group_role` remembers what groups last decided, so that leaving every
-- such group returns the person to member rather than leaving the old role behind. Owners, and
-- the admin whose token is calling, are never changed this way.

CREATE TABLE scim_groups (
    id              uuid PRIMARY KEY,
    tenant_id       uuid NOT NULL,
    organization_id uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    display_name    citext NOT NULL CHECK (char_length(display_name) BETWEEN 1 AND 200),
    external_id     text CHECK (char_length(external_id) <= 200),
    -- Set by a workspace admin, never by the provider.
    org_role        text CHECK (org_role IN ('admin', 'member', 'guest')),
    version         bigint NOT NULL DEFAULT 1,
    created_at      timestamptz NOT NULL DEFAULT now(),
    updated_at      timestamptz NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX scim_groups_name_idx ON scim_groups (tenant_id, display_name);
CREATE INDEX scim_groups_external_idx ON scim_groups (tenant_id, external_id);
CREATE INDEX scim_groups_listing_idx ON scim_groups (tenant_id, created_at, id);

CREATE TABLE scim_group_members (
    tenant_id    uuid NOT NULL,
    group_id     uuid NOT NULL REFERENCES scim_groups (id) ON DELETE CASCADE,
    scim_user_id uuid NOT NULL REFERENCES scim_users (id) ON DELETE CASCADE,
    PRIMARY KEY (group_id, scim_user_id)
);
CREATE INDEX scim_group_members_user_idx ON scim_group_members (scim_user_id);

ALTER TABLE scim_users ADD COLUMN group_role text CHECK (group_role IN ('admin', 'member', 'guest'));

DO $$
DECLARE t text;
BEGIN
    FOREACH t IN ARRAY ARRAY['scim_groups', 'scim_group_members'] LOOP
        EXECUTE format('ALTER TABLE %I ENABLE ROW LEVEL SECURITY', t);
        EXECUTE format('ALTER TABLE %I FORCE ROW LEVEL SECURITY', t);
        EXECUTE format(
            'CREATE POLICY tenant_isolation ON %I USING (tenant_id = app_tenant_id()) WITH CHECK (tenant_id = app_tenant_id())', t);
    END LOOP;
END $$;
