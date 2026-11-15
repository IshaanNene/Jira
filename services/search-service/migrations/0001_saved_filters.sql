-- Saved MQL filters. The index itself is not data of record (it is rebuilt from issue-service);
-- these rows are, so they live in PostgreSQL.

CREATE FUNCTION app_tenant_id() RETURNS uuid
    LANGUAGE sql STABLE PARALLEL SAFE
    AS $$ SELECT NULLIF(current_setting('app.tenant_id', true), '')::uuid $$;

CREATE TABLE saved_filters (
    tenant_id   uuid        NOT NULL,
    id          uuid        PRIMARY KEY,
    owner_id    uuid        NOT NULL,
    name        text        NOT NULL CHECK (char_length(name) BETWEEN 1 AND 80),
    -- MQL, checked by the parser on every write; run with the viewer's own access.
    query       text        NOT NULL CHECK (char_length(query) BETWEEN 1 AND 2000),
    -- false: only the owner sees it. true: every member of the workspace can run it.
    shared      boolean     NOT NULL DEFAULT false,
    created_at  timestamptz NOT NULL DEFAULT now(),
    updated_at  timestamptz NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX saved_filters_owner_name_idx ON saved_filters (tenant_id, owner_id, lower(name));
CREATE INDEX saved_filters_shared_idx ON saved_filters (tenant_id, updated_at DESC) WHERE shared;

ALTER TABLE saved_filters ENABLE ROW LEVEL SECURITY;
ALTER TABLE saved_filters FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON saved_filters USING (tenant_id = app_tenant_id()) WITH CHECK (tenant_id = app_tenant_id());
