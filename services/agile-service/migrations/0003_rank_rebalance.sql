-- Projects whose ranks have grown long enough to rewrite. Like `outbox`, a system queue
-- drained across tenants by the rebalancer, so it is not row-level secured and holds no
-- content; no API reads it. Rows are only added by the server, inside the tenant's own
-- transaction, so a row lives in the database that holds that tenant's placements.
CREATE TABLE rank_rebalance (
    tenant_id    uuid        NOT NULL,
    project_id   uuid        NOT NULL,
    queued_at    timestamptz NOT NULL DEFAULT now(),
    leased_until timestamptz,
    PRIMARY KEY (tenant_id, project_id)
);
CREATE INDEX rank_rebalance_queued_idx ON rank_rebalance (queued_at);
