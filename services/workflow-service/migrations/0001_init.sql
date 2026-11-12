-- Workflow service: workflow definitions, their published versions, and which
-- workflow governs which project / issue type. All tenant-owned.

CREATE FUNCTION app_tenant_id() RETURNS uuid
    LANGUAGE sql STABLE PARALLEL SAFE
    AS $$ SELECT NULLIF(current_setting('app.tenant_id', true), '')::uuid $$;

CREATE TABLE workflows (
    id                uuid PRIMARY KEY,
    tenant_id         uuid NOT NULL,
    name              text NOT NULL CHECK (char_length(name) BETWEEN 1 AND 80),
    description       text NOT NULL DEFAULT '',
    -- Work in progress. Never used to evaluate transitions.
    draft             jsonb,
    -- Latest published version number (NULL until first publish).
    published_version integer,
    -- Row version for optimistic concurrency on edits.
    version           bigint NOT NULL DEFAULT 1,
    created_by        uuid NOT NULL,
    created_at        timestamptz NOT NULL DEFAULT now(),
    updated_at        timestamptz NOT NULL DEFAULT now(),
    deleted_at        timestamptz
);
CREATE UNIQUE INDEX workflows_name_idx ON workflows (tenant_id, lower(name)) WHERE deleted_at IS NULL;

-- Immutable once written: publishing always adds a new version.
CREATE TABLE workflow_versions (
    tenant_id    uuid NOT NULL,
    workflow_id  uuid NOT NULL REFERENCES workflows (id) ON DELETE CASCADE,
    version      integer NOT NULL CHECK (version > 0),
    definition   jsonb NOT NULL,
    published_by uuid NOT NULL,
    published_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (workflow_id, version)
);

-- The workflow scheme. Resolution order for (project, issue type):
--   project + type, project (any type), tenant default + type, tenant default, built-in.
CREATE TABLE workflow_assignments (
    id          uuid PRIMARY KEY,
    tenant_id   uuid NOT NULL,
    project_id  uuid,          -- NULL = tenant-wide default
    issue_type  text,          -- NULL = every issue type
    workflow_id uuid NOT NULL REFERENCES workflows (id),
    created_by  uuid NOT NULL,
    created_at  timestamptz NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX workflow_assignments_scope_idx ON workflow_assignments
    (tenant_id, COALESCE(project_id, '00000000-0000-0000-0000-000000000000'::uuid), COALESCE(issue_type, ''));
CREATE INDEX workflow_assignments_workflow_idx ON workflow_assignments (tenant_id, workflow_id);

DO $$
DECLARE t text;
BEGIN
    FOREACH t IN ARRAY ARRAY['workflows', 'workflow_versions', 'workflow_assignments'] LOOP
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
