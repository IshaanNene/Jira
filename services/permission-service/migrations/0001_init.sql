-- Permission service: project role bindings and per-tenant policy versions.

CREATE FUNCTION app_tenant_id() RETURNS uuid
    LANGUAGE sql STABLE PARALLEL SAFE
    AS $$ SELECT NULLIF(current_setting('app.tenant_id', true), '')::uuid $$;

CREATE TABLE project_role_bindings (
    tenant_id  uuid NOT NULL,
    project_id uuid NOT NULL,
    user_id    uuid NOT NULL,
    role       text NOT NULL CHECK (role IN ('administrator', 'member', 'viewer')),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, project_id, user_id)
);
CREATE INDEX project_role_bindings_user_idx ON project_role_bindings (tenant_id, user_id);

-- Role every organization member (not guests) holds on a project. No row = private project.
CREATE TABLE project_default_roles (
    tenant_id  uuid NOT NULL,
    project_id uuid NOT NULL,
    role       text NOT NULL CHECK (role IN ('administrator', 'member', 'viewer')),
    PRIMARY KEY (tenant_id, project_id)
);

-- Bumped on every change; consumers discard cached snapshots with another version.
CREATE TABLE policy_versions (
    tenant_id uuid PRIMARY KEY,
    version   bigint NOT NULL
);

DO $$
DECLARE t text;
BEGIN
    FOREACH t IN ARRAY ARRAY['project_role_bindings', 'project_default_roles', 'policy_versions'] LOOP
        EXECUTE format('ALTER TABLE %I ENABLE ROW LEVEL SECURITY', t);
        EXECUTE format('ALTER TABLE %I FORCE ROW LEVEL SECURITY', t);
        EXECUTE format(
            'CREATE POLICY tenant_isolation ON %I USING (tenant_id = app_tenant_id()) WITH CHECK (tenant_id = app_tenant_id())', t);
    END LOOP;
END $$;

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
