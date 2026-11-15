-- Subscriptions to saved filters: the filter runs on a schedule as the subscriber, with their
-- access at that moment, and the result is emailed by notification-service.

CREATE TABLE filter_subscriptions (
    tenant_id   uuid        NOT NULL,
    id          uuid        PRIMARY KEY,
    filter_id   uuid        NOT NULL REFERENCES saved_filters (id) ON DELETE CASCADE,
    user_id     uuid        NOT NULL,
    frequency   text        NOT NULL CHECK (frequency IN ('daily', 'weekly')),
    -- UTC hour of the day it runs.
    hour        smallint    NOT NULL CHECK (hour BETWEEN 0 AND 23),
    -- ISO weekday (1 = Monday) for weekly subscriptions.
    weekday     smallint    CHECK (weekday BETWEEN 1 AND 7),
    last_sent_at timestamptz,
    created_at  timestamptz NOT NULL DEFAULT now(),
    CHECK ((frequency = 'weekly') = (weekday IS NOT NULL))
);
CREATE UNIQUE INDEX filter_subscriptions_one_per_person_idx ON filter_subscriptions (tenant_id, filter_id, user_id);
CREATE INDEX filter_subscriptions_user_idx ON filter_subscriptions (tenant_id, user_id);

ALTER TABLE filter_subscriptions ENABLE ROW LEVEL SECURITY;
ALTER TABLE filter_subscriptions FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON filter_subscriptions
    USING (tenant_id = app_tenant_id()) WITH CHECK (tenant_id = app_tenant_id());

-- When each subscription runs next. Like `outbox`, a system queue drained across tenants by the
-- runner, so it is not row-level secured and holds no content; no API reads it.
CREATE TABLE subscription_schedule (
    subscription_id uuid        PRIMARY KEY REFERENCES filter_subscriptions (id) ON DELETE CASCADE,
    tenant_id       uuid        NOT NULL,
    next_run_at     timestamptz NOT NULL,
    leased_until    timestamptz
);
CREATE INDEX subscription_schedule_due_idx ON subscription_schedule (next_run_at);

-- Digests leave through the transactional outbox (event-contracts' OUTBOX_TABLE_SQL).
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
