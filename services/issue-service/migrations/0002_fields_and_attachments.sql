-- Components / fix versions on issues, custom fields, attachments.

ALTER TABLE issues
    -- Ids of the owning project's components and versions (validated against project-service).
    ADD COLUMN component_ids   uuid[] NOT NULL DEFAULT '{}',
    ADD COLUMN fix_version_ids uuid[] NOT NULL DEFAULT '{}',
    -- { "<custom field id>": value } — validated against the field definition (ADR-0005).
    ADD COLUMN custom_fields   jsonb  NOT NULL DEFAULT '{}'::jsonb;
CREATE INDEX issues_custom_fields_idx ON issues USING gin (custom_fields jsonb_path_ops);
CREATE INDEX issues_components_idx ON issues USING gin (component_ids);
CREATE INDEX issues_fix_versions_idx ON issues USING gin (fix_version_ids);

CREATE TABLE custom_fields (
    id          uuid PRIMARY KEY,
    tenant_id   uuid NOT NULL,
    name        text NOT NULL CHECK (char_length(name) BETWEEN 1 AND 60),
    field_type  text NOT NULL CHECK (field_type IN ('text', 'number', 'date', 'select', 'multi_select', 'checkbox', 'url', 'user')),
    -- Choices for select / multi_select.
    options     text[] NOT NULL DEFAULT '{}',
    required    boolean NOT NULL DEFAULT false,
    -- Context: empty array = applies everywhere.
    project_keys text[] NOT NULL DEFAULT '{}',
    issue_types  text[] NOT NULL DEFAULT '{}',
    created_by  uuid NOT NULL,
    created_at  timestamptz NOT NULL DEFAULT now(),
    updated_at  timestamptz NOT NULL DEFAULT now(),
    -- Archived fields are hidden; their stored values are kept.
    archived_at timestamptz
);
CREATE UNIQUE INDEX custom_fields_name_idx ON custom_fields (tenant_id, lower(name)) WHERE archived_at IS NULL;

CREATE TABLE attachments (
    id           uuid PRIMARY KEY,
    tenant_id    uuid NOT NULL,
    issue_id     uuid NOT NULL REFERENCES issues (id) ON DELETE CASCADE,
    uploader_id  uuid NOT NULL,
    filename     text NOT NULL,
    -- Decided by the server from the file's content, never taken from the client.
    content_type text NOT NULL,
    size_bytes   bigint NOT NULL CHECK (size_bytes >= 0),
    sha256       text NOT NULL,
    -- Server-generated; clients never choose or see storage paths.
    object_key   text NOT NULL UNIQUE,
    scan_status  text NOT NULL DEFAULT 'not_scanned' CHECK (scan_status IN ('not_scanned', 'clean', 'infected')),
    created_at   timestamptz NOT NULL DEFAULT now(),
    deleted_at   timestamptz
);
CREATE INDEX attachments_issue_idx ON attachments (tenant_id, issue_id, created_at) WHERE deleted_at IS NULL;

DO $$
DECLARE t text;
BEGIN
    FOREACH t IN ARRAY ARRAY['custom_fields', 'attachments'] LOOP
        EXECUTE format('ALTER TABLE %I ENABLE ROW LEVEL SECURITY', t);
        EXECUTE format('ALTER TABLE %I FORCE ROW LEVEL SECURITY', t);
        EXECUTE format(
            'CREATE POLICY tenant_isolation ON %I USING (tenant_id = app_tenant_id()) WITH CHECK (tenant_id = app_tenant_id())', t);
    END LOOP;
END $$;
