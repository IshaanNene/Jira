-- Outgoing webhooks: a workspace's subscriptions to its own events, delivered to HTTPS endpoints.
CREATE TABLE webhooks (
    id                   uuid PRIMARY KEY,
    tenant_id            uuid NOT NULL,
    name                 text NOT NULL CHECK (char_length(name) BETWEEN 1 AND 100),
    url                  text NOT NULL CHECK (char_length(url) BETWEEN 8 AND 2000),
    events               text[] NOT NULL CHECK (cardinality(events) > 0),
    -- Empty: every project.
    project_ids          uuid[] NOT NULL DEFAULT '{}',
    -- The signing secret, encrypted with a key derived from the service secret (AES-256-GCM).
    secret_sealed        bytea NOT NULL,
    enabled              boolean NOT NULL DEFAULT true,
    -- Set when the service turns a webhook off after repeated failures.
    disabled_reason      text,
    consecutive_failures integer NOT NULL DEFAULT 0,
    created_by           uuid NOT NULL,
    created_at           timestamptz NOT NULL DEFAULT now(),
    updated_at           timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX webhooks_tenant_idx ON webhooks (tenant_id, created_at);
ALTER TABLE webhooks ENABLE ROW LEVEL SECURITY;
ALTER TABLE webhooks FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON webhooks USING (tenant_id = app_tenant_id()) WITH CHECK (tenant_id = app_tenant_id());

-- The delivery queue. Like `email_deliveries`, a system queue drained across tenants by the
-- sender, so it is not row-level secured; the API reads it only for a webhook it has first
-- loaded under the tenant's row-level security, and always filters by tenant. Rows are
-- deleted 7 days after they finish.
CREATE TABLE webhook_deliveries (
    id              uuid PRIMARY KEY,
    tenant_id       uuid NOT NULL,
    webhook_id      uuid NOT NULL REFERENCES webhooks (id) ON DELETE CASCADE,
    event_id        uuid NOT NULL,
    event_type      text NOT NULL,
    body            jsonb NOT NULL,
    -- pending → sending (leased) → delivered | failed | skipped (webhook turned off meanwhile)
    status          text NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'sending', 'delivered', 'failed', 'skipped')),
    attempts        integer NOT NULL DEFAULT 0,
    send_after      timestamptz NOT NULL DEFAULT now(),
    response_status integer,
    last_error      text,
    created_at      timestamptz NOT NULL DEFAULT now(),
    finished_at     timestamptz,
    UNIQUE (webhook_id, event_id)
);
CREATE INDEX webhook_deliveries_due_idx ON webhook_deliveries (send_after) WHERE status IN ('pending', 'sending');
CREATE INDEX webhook_deliveries_list_idx ON webhook_deliveries (tenant_id, webhook_id, created_at DESC);
CREATE INDEX webhook_deliveries_finished_idx ON webhook_deliveries (finished_at) WHERE finished_at IS NOT NULL;
