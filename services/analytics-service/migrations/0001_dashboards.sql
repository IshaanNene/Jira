-- Dashboards and their widgets. The facts they chart live in ClickHouse (clickhouse/).

CREATE FUNCTION app_tenant_id() RETURNS uuid
    LANGUAGE sql STABLE PARALLEL SAFE
    AS $$ SELECT NULLIF(current_setting('app.tenant_id', true), '')::uuid $$;

CREATE TABLE dashboards (
    tenant_id   uuid        NOT NULL,
    id          uuid        PRIMARY KEY,
    name        text        NOT NULL CHECK (length(name) BETWEEN 1 AND 80),
    description text        NOT NULL DEFAULT '' CHECK (length(description) <= 500),
    owner_id    uuid        NOT NULL,
    -- false: only the owner sees it. true: every member of the workspace can open it.
    shared      boolean     NOT NULL DEFAULT false,
    created_at  timestamptz NOT NULL DEFAULT now(),
    updated_at  timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX dashboards_listing_idx ON dashboards (tenant_id, owner_id, updated_at DESC);

CREATE TABLE dashboard_widgets (
    tenant_id    uuid        NOT NULL,
    id           uuid        PRIMARY KEY,
    dashboard_id uuid        NOT NULL REFERENCES dashboards (id) ON DELETE CASCADE,
    title        text        NOT NULL CHECK (length(title) BETWEEN 1 AND 80),
    -- One of a fixed set of chart kinds and its options (src/widgets.rs), validated on write.
    query        jsonb       NOT NULL,
    size         text        NOT NULL DEFAULT 'half' CHECK (size IN ('half', 'full')),
    position     integer     NOT NULL,
    created_at   timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX dashboard_widgets_dashboard_idx ON dashboard_widgets (dashboard_id, position);

ALTER TABLE dashboards ENABLE ROW LEVEL SECURITY;
ALTER TABLE dashboards FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON dashboards USING (tenant_id = app_tenant_id()) WITH CHECK (tenant_id = app_tenant_id());
ALTER TABLE dashboard_widgets ENABLE ROW LEVEL SECURITY;
ALTER TABLE dashboard_widgets FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON dashboard_widgets USING (tenant_id = app_tenant_id()) WITH CHECK (tenant_id = app_tenant_id());
