-- Audit log: one hash chain per tenant, append-only.
--
-- Each row's hash is SHA-256(prev_hash || canonical form of the row), computed by the
-- service when it appends (see src/audit.rs). Rows are never changed: the runtime role
-- loses UPDATE and DELETE, and a trigger refuses both for every role, so a change needs
-- the table owner to drop the trigger first, and even then the chain no longer verifies.

CREATE TABLE audit_log (
    tenant_id      uuid        NOT NULL,
    seq            bigint      NOT NULL,          -- 1, 2, 3 … per tenant, no gaps
    id             uuid        NOT NULL,
    event_id       uuid        NOT NULL,          -- the event it was recorded from
    occurred_at    timestamptz NOT NULL,
    recorded_at    timestamptz NOT NULL DEFAULT now(),
    actor_type     text        NOT NULL,          -- user | system | automation
    actor_id       text        NOT NULL,
    action         text        NOT NULL,
    resource_type  text        NOT NULL,
    resource_id    text        NOT NULL,
    resource_label text        NOT NULL,
    details        jsonb       NOT NULL,          -- string → string
    request_id     text,
    prev_hash      bytea       NOT NULL,
    hash           bytea       NOT NULL,
    PRIMARY KEY (tenant_id, seq),
    UNIQUE (tenant_id, event_id),
    UNIQUE (id)
);
CREATE INDEX audit_log_listing_idx ON audit_log (tenant_id, occurred_at DESC, seq DESC);
CREATE INDEX audit_log_action_idx ON audit_log (tenant_id, action, seq DESC);
CREATE INDEX audit_log_actor_idx ON audit_log (tenant_id, actor_id, seq DESC);

ALTER TABLE audit_log ENABLE ROW LEVEL SECURITY;
ALTER TABLE audit_log FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON audit_log USING (tenant_id = app_tenant_id()) WITH CHECK (tenant_id = app_tenant_id());

CREATE FUNCTION audit_log_append_only() RETURNS trigger
    LANGUAGE plpgsql
    AS $$ BEGIN RAISE EXCEPTION 'audit_log is append-only'; END $$;
CREATE TRIGGER audit_log_append_only BEFORE UPDATE OR DELETE ON audit_log
    FOR EACH ROW EXECUTE FUNCTION audit_log_append_only();
CREATE TRIGGER audit_log_no_truncate BEFORE TRUNCATE ON audit_log
    FOR EACH STATEMENT EXECUTE FUNCTION audit_log_append_only();

-- Take back the UPDATE and DELETE that default privileges gave other roles.
DO $$
DECLARE r text;
BEGIN
    FOR r IN SELECT DISTINCT grantee FROM information_schema.role_table_grants
             WHERE table_name = 'audit_log' AND grantee <> current_user AND grantee <> 'PUBLIC' LOOP
        EXECUTE format('REVOKE UPDATE, DELETE, TRUNCATE ON audit_log FROM %I', r);
    END LOOP;
END $$;
