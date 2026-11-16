-- SCIM 2.0 provisioning: a workspace's identity provider creates, updates and deactivates the
-- people in it.
--
-- A provisioning token belongs to one workspace and acts with the role of the admin who made it,
-- re-read on every request (it stops working when they stop being an admin). Only a SHA-256 hash of
-- the token is kept. The global index finds the workspace from the hash, like invitation_tokens.

CREATE TABLE scim_tokens (
    id              uuid PRIMARY KEY,
    tenant_id       uuid NOT NULL,
    organization_id uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    name            text NOT NULL CHECK (char_length(name) BETWEEN 1 AND 80),
    -- The first characters of the token, to tell tokens apart in the list.
    prefix          text NOT NULL,
    created_by      uuid NOT NULL,
    created_at      timestamptz NOT NULL DEFAULT now(),
    last_used_at    timestamptz,
    revoked_at      timestamptz
);
CREATE INDEX scim_tokens_listing_idx ON scim_tokens (tenant_id, created_at DESC);

CREATE TABLE scim_token_index (
    token_hash bytea PRIMARY KEY,
    tenant_id  uuid NOT NULL,
    token_id   uuid NOT NULL UNIQUE
);

-- A person as the identity provider describes them. `user_id` is set once they are a member
-- (straight away when the address already belongs to a member, otherwise when they accept the
-- invitation provisioning sent them). Names here are the provider's; the person's own profile,
-- shared with their other workspaces, is not changed.
CREATE TABLE scim_users (
    id              uuid PRIMARY KEY,
    tenant_id       uuid NOT NULL,
    organization_id uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    user_name       citext NOT NULL CHECK (char_length(user_name) BETWEEN 1 AND 320),
    email           citext NOT NULL CHECK (char_length(email) BETWEEN 3 AND 320),
    external_id     text CHECK (char_length(external_id) <= 200),
    display_name    text CHECK (char_length(display_name) <= 200),
    given_name      text CHECK (char_length(given_name) <= 100),
    family_name     text CHECK (char_length(family_name) <= 100),
    active          boolean NOT NULL DEFAULT true,
    user_id         uuid,
    invitation_id   uuid,
    version         bigint NOT NULL DEFAULT 1,
    created_at      timestamptz NOT NULL DEFAULT now(),
    updated_at      timestamptz NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX scim_users_user_name_idx ON scim_users (tenant_id, user_name);
CREATE UNIQUE INDEX scim_users_user_idx ON scim_users (tenant_id, user_id) WHERE user_id IS NOT NULL;
CREATE INDEX scim_users_invitation_idx ON scim_users (tenant_id, invitation_id) WHERE invitation_id IS NOT NULL;
CREATE INDEX scim_users_external_idx ON scim_users (tenant_id, external_id);
CREATE INDEX scim_users_listing_idx ON scim_users (tenant_id, created_at, id);

DO $$
DECLARE t text;
BEGIN
    FOREACH t IN ARRAY ARRAY['scim_tokens', 'scim_users'] LOOP
        EXECUTE format('ALTER TABLE %I ENABLE ROW LEVEL SECURITY', t);
        EXECUTE format('ALTER TABLE %I FORCE ROW LEVEL SECURITY', t);
        EXECUTE format(
            'CREATE POLICY tenant_isolation ON %I USING (tenant_id = app_tenant_id()) WITH CHECK (tenant_id = app_tenant_id())', t);
    END LOOP;
END $$;
