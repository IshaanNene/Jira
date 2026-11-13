-- Notification service: each person's in-app notifications. Tenant-owned, forced RLS.

CREATE FUNCTION app_tenant_id() RETURNS uuid
    LANGUAGE sql STABLE PARALLEL SAFE
    AS $$ SELECT NULLIF(current_setting('app.tenant_id', true), '')::uuid $$;

CREATE TABLE notifications (
    id              uuid PRIMARY KEY,
    tenant_id       uuid NOT NULL,
    recipient_id    uuid NOT NULL,
    kind            text NOT NULL CHECK (kind IN ('assigned', 'commented', 'mentioned', 'status_changed')),
    project_id      uuid NOT NULL,
    issue_id        uuid NOT NULL,
    -- Snapshots for display, as of when the notification was created.
    issue_key       text NOT NULL,
    issue_summary   text NOT NULL,
    actor_id        uuid,
    actor_name      text NOT NULL DEFAULT '',
    -- Kind-specific facts, e.g. {"to_status": "done"} or {"comment_id": "…"}.
    detail          jsonb NOT NULL DEFAULT '{}',
    -- The event that caused it: one notification per person per event.
    source_event_id uuid NOT NULL,
    read_at         timestamptz,
    created_at      timestamptz NOT NULL DEFAULT now(),
    UNIQUE (tenant_id, recipient_id, source_event_id)
);
CREATE INDEX notifications_inbox_idx ON notifications (tenant_id, recipient_id, created_at DESC, id DESC);
CREATE INDEX notifications_unread_idx ON notifications (tenant_id, recipient_id) WHERE read_at IS NULL;

ALTER TABLE notifications ENABLE ROW LEVEL SECURITY;
ALTER TABLE notifications FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON notifications
    USING (tenant_id = app_tenant_id()) WITH CHECK (tenant_id = app_tenant_id());

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
