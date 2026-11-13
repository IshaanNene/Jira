-- Email delivery of notifications, and each person's choices about it.

-- What a person wants by email, per workspace. No row = defaults (everything on).
CREATE TABLE notification_preferences (
    tenant_id     uuid NOT NULL,
    user_id       uuid NOT NULL,
    email_enabled boolean NOT NULL DEFAULT true,
    -- Kinds not to email about, e.g. {commented, status_changed}.
    email_muted   text[] NOT NULL DEFAULT '{}',
    updated_at    timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, user_id)
);
ALTER TABLE notification_preferences ENABLE ROW LEVEL SECURITY;
ALTER TABLE notification_preferences FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON notification_preferences
    USING (tenant_id = app_tenant_id()) WITH CHECK (tenant_id = app_tenant_id());

-- The send queue. Like `outbox`, a system queue drained across tenants by the sender, so it
-- is not row-level secured; no API reads it. Rendered content is stored so that sending
-- needs nothing else; rows are deleted 7 days after they finish.
CREATE TABLE email_deliveries (
    id              uuid PRIMARY KEY,
    tenant_id       uuid NOT NULL,
    notification_id uuid NOT NULL UNIQUE,
    recipient_id    uuid NOT NULL,
    kind            text NOT NULL,
    to_address      text NOT NULL,
    subject         text NOT NULL,
    text_body       text NOT NULL,
    html_body       text NOT NULL,
    -- pending → sending (leased) → sent | skipped (read in the app first, or muted) | failed
    status          text NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'sending', 'sent', 'skipped', 'failed')),
    attempts        integer NOT NULL DEFAULT 0,
    send_after      timestamptz NOT NULL,
    last_error      text,
    created_at      timestamptz NOT NULL DEFAULT now(),
    finished_at     timestamptz
);
CREATE INDEX email_deliveries_due_idx ON email_deliveries (send_after) WHERE status IN ('pending', 'sending');
CREATE INDEX email_deliveries_finished_idx ON email_deliveries (finished_at) WHERE finished_at IS NOT NULL;
