-- Project service: projects, components and versions. All tenant-owned.

CREATE FUNCTION app_tenant_id() RETURNS uuid
    LANGUAGE sql STABLE PARALLEL SAFE
    AS $$ SELECT NULLIF(current_setting('app.tenant_id', true), '')::uuid $$;

CREATE TABLE projects (
    id            uuid PRIMARY KEY,
    tenant_id     uuid NOT NULL,
    -- Immutable once created: issue keys (KEY-123) are built from it.
    key           text NOT NULL CHECK (key ~ '^[A-Z][A-Z0-9]{1,9}$'),
    name          text NOT NULL,
    description   text NOT NULL DEFAULT '',
    project_type  text NOT NULL CHECK (project_type IN ('scrum', 'kanban', 'general')),
    lead_user_id  uuid,
    status        text NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'archived')),
    -- Display copy of the default role held by the permission service (NULL = private).
    default_role  text CHECK (default_role IN ('administrator', 'member', 'viewer')),
    version       bigint NOT NULL DEFAULT 1,
    created_by    uuid NOT NULL,
    created_at    timestamptz NOT NULL DEFAULT now(),
    updated_at    timestamptz NOT NULL DEFAULT now(),
    -- Soft delete: kept for the retention window, hidden everywhere.
    deleted_at    timestamptz,
    UNIQUE (tenant_id, key)
);
CREATE INDEX projects_listing_idx ON projects (tenant_id, status, name) WHERE deleted_at IS NULL;

CREATE TABLE components (
    id           uuid PRIMARY KEY,
    tenant_id    uuid NOT NULL,
    project_id   uuid NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    name         text NOT NULL,
    description  text NOT NULL DEFAULT '',
    lead_user_id uuid,
    created_at   timestamptz NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX components_name_idx ON components (tenant_id, project_id, lower(name));

CREATE TABLE versions (
    id           uuid PRIMARY KEY,
    tenant_id    uuid NOT NULL,
    project_id   uuid NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    name         text NOT NULL,
    description  text NOT NULL DEFAULT '',
    start_date   date,
    release_date date,
    status       text NOT NULL DEFAULT 'unreleased' CHECK (status IN ('unreleased', 'released', 'archived')),
    released_at  timestamptz,
    created_at   timestamptz NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX versions_name_idx ON versions (tenant_id, project_id, lower(name));

DO $$
DECLARE t text;
BEGIN
    FOREACH t IN ARRAY ARRAY['projects', 'components', 'versions'] LOOP
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
