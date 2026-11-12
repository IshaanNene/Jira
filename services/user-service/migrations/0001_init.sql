-- User & organization service.
--
-- Tenant-owned tables carry tenant_id and FORCE row-level security. The runtime role
-- neither owns these tables nor has BYPASSRLS, so a statement executed without
-- app.tenant_id set (or with another tenant's id) sees and changes nothing.

-- '' (a reset setting) and NULL (never set) both mean "no tenant".
CREATE FUNCTION app_tenant_id() RETURNS uuid
    LANGUAGE sql STABLE PARALLEL SAFE
    AS $$ SELECT NULLIF(current_setting('app.tenant_id', true), '')::uuid $$;

-- ---------------------------------------------------------------------------
-- Global tables (a person exists independently of any organization)
-- ---------------------------------------------------------------------------

CREATE TABLE user_profiles (
    user_id      uuid PRIMARY KEY,           -- = identity account id
    email        citext NOT NULL,
    display_name text NOT NULL,
    avatar_key   text,
    timezone     text NOT NULL DEFAULT 'UTC',
    locale       text NOT NULL DEFAULT 'en',
    preferences  jsonb NOT NULL DEFAULT '{}'::jsonb,
    version      bigint NOT NULL DEFAULT 1,
    created_at   timestamptz NOT NULL DEFAULT now(),
    updated_at   timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX user_profiles_email_idx ON user_profiles (email);

-- Answers "which organizations is this user in?" without crossing tenant boundaries.
-- Every query against it is keyed by the caller's own user_id from their token.
CREATE TABLE user_org_directory (
    user_id         uuid NOT NULL,
    organization_id uuid NOT NULL,
    tenant_id       uuid NOT NULL,
    created_at      timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, organization_id)
);
CREATE INDEX user_org_directory_tenant_idx ON user_org_directory (tenant_id);

-- Lets an invitee (who is not yet in the tenant) find the invitation from the secret
-- token. Holds only the token hash and where to look.
CREATE TABLE invitation_tokens (
    token_hash    bytea PRIMARY KEY,
    tenant_id     uuid NOT NULL,
    invitation_id uuid NOT NULL UNIQUE
);

-- ---------------------------------------------------------------------------
-- Tenant-owned tables
-- ---------------------------------------------------------------------------

CREATE TABLE organizations (
    id             uuid PRIMARY KEY,
    tenant_id      uuid NOT NULL UNIQUE,
    name           text NOT NULL,
    slug           citext NOT NULL UNIQUE,
    settings       jsonb NOT NULL DEFAULT '{}'::jsonb,
    version        bigint NOT NULL DEFAULT 1,
    -- Version counter of the membership aggregate (members + invitations) for events.
    membership_seq bigint NOT NULL DEFAULT 0,
    created_at     timestamptz NOT NULL DEFAULT now(),
    updated_at     timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE memberships (
    tenant_id       uuid NOT NULL,
    user_id         uuid NOT NULL,
    organization_id uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    org_role        text NOT NULL CHECK (org_role IN ('owner', 'admin', 'member', 'guest')),
    status          text NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'suspended')),
    joined_at       timestamptz NOT NULL DEFAULT now(),
    updated_at      timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, user_id)
);
CREATE INDEX memberships_listing_idx ON memberships (tenant_id, joined_at, user_id);

CREATE TABLE invitations (
    id              uuid PRIMARY KEY,
    tenant_id       uuid NOT NULL,
    organization_id uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    email           citext NOT NULL,
    org_role        text NOT NULL CHECK (org_role IN ('admin', 'member', 'guest')),
    invited_by      uuid NOT NULL,
    created_at      timestamptz NOT NULL DEFAULT now(),
    expires_at      timestamptz NOT NULL,
    accepted_at     timestamptz,
    accepted_by     uuid,
    revoked_at      timestamptz
);
-- One open invitation per address per organization.
CREATE UNIQUE INDEX invitations_pending_email_idx ON invitations (tenant_id, email)
    WHERE accepted_at IS NULL AND revoked_at IS NULL;
CREATE INDEX invitations_listing_idx ON invitations (tenant_id, created_at DESC);

DO $$
DECLARE t text;
BEGIN
    FOREACH t IN ARRAY ARRAY['organizations', 'memberships', 'invitations'] LOOP
        EXECUTE format('ALTER TABLE %I ENABLE ROW LEVEL SECURITY', t);
        EXECUTE format('ALTER TABLE %I FORCE ROW LEVEL SECURITY', t);
        EXECUTE format(
            'CREATE POLICY tenant_isolation ON %I USING (tenant_id = app_tenant_id()) WITH CHECK (tenant_id = app_tenant_id())', t);
    END LOOP;
END $$;

-- ---------------------------------------------------------------------------
-- Messaging infrastructure (not tenant-scoped: the relay reads across tenants)
-- ---------------------------------------------------------------------------

CREATE TABLE outbox (
    id            uuid PRIMARY KEY,
    tenant_id     uuid,
    topic         text NOT NULL,
    partition_key text NOT NULL,
    event_type    text NOT NULL,
    payload       jsonb NOT NULL,
    created_at    timestamptz NOT NULL DEFAULT now(),
    published_at  timestamptz,
    attempts      integer NOT NULL DEFAULT 0,
    last_error    text
);
CREATE INDEX outbox_unpublished_idx ON outbox (created_at) WHERE published_at IS NULL;

CREATE TABLE inbox (
    consumer     text NOT NULL,
    event_id     uuid NOT NULL,
    processed_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (consumer, event_id)
);
