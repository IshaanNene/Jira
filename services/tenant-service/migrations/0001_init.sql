-- Tenant registry. Global by nature: this database is the map from tenant to data placement.
-- It is reachable only over authenticated internal gRPC.

CREATE TABLE database_clusters (
    id                   text PRIMARY KEY,
    region               text NOT NULL,
    kind                 text NOT NULL CHECK (kind IN ('shared', 'dedicated')),
    -- Logical name. Services map it to a connection string from their own secrets.
    database_destination text NOT NULL UNIQUE,
    status               text NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'draining', 'retired')),
    tenant_capacity      integer NOT NULL DEFAULT 10000 CHECK (tenant_capacity > 0),
    created_at           timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE tenants (
    tenant_id             uuid PRIMARY KEY,
    organization_id       uuid NOT NULL UNIQUE,
    deployment_tier       text NOT NULL CHECK (deployment_tier IN ('standard', 'business', 'enterprise')),
    database_cluster_id   text REFERENCES database_clusters (id),
    database_destination  text NOT NULL,
    region                text NOT NULL,
    tenant_status         text NOT NULL CHECK (tenant_status IN ('provisioning', 'active', 'suspended', 'migrating', 'deleted')),
    provisioning_status   text NOT NULL CHECK (provisioning_status IN ('pending', 'provisioned', 'failed', 'abandoned')),
    configuration_version bigint NOT NULL DEFAULT 1,
    plan                  jsonb NOT NULL DEFAULT '{}'::jsonb,
    created_at            timestamptz NOT NULL DEFAULT now(),
    updated_at            timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX tenants_cluster_idx ON tenants (database_cluster_id) WHERE tenant_status <> 'deleted';

CREATE TABLE tenant_migrations (
    id               uuid PRIMARY KEY,
    tenant_id        uuid NOT NULL REFERENCES tenants (tenant_id),
    from_tier        text NOT NULL,
    to_tier          text NOT NULL,
    from_destination text NOT NULL,
    to_destination   text NOT NULL,
    state            text NOT NULL CHECK (state IN ('requested', 'copying', 'verifying', 'cutover', 'completed', 'failed', 'rolled_back')),
    requested_by     uuid,
    error            text,
    created_at       timestamptz NOT NULL DEFAULT now(),
    updated_at       timestamptz NOT NULL DEFAULT now()
);
-- At most one migration in flight per tenant.
CREATE UNIQUE INDEX tenant_migrations_active_idx ON tenant_migrations (tenant_id)
    WHERE state IN ('requested', 'copying', 'verifying', 'cutover');

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
