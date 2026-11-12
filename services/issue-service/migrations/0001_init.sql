-- Issue service: issues, comments, links, history, watchers. All tenant-owned.

CREATE FUNCTION app_tenant_id() RETURNS uuid
    LANGUAGE sql STABLE PARALLEL SAFE
    AS $$ SELECT NULLIF(current_setting('app.tenant_id', true), '')::uuid $$;

-- One row per project; the row lock taken when it is incremented serialises issue
-- creation per project, which is what makes issue numbers gap-free and unique (ADR-0006).
CREATE TABLE project_issue_counters (
    tenant_id   uuid NOT NULL,
    project_id  uuid NOT NULL,
    next_number bigint NOT NULL,
    PRIMARY KEY (tenant_id, project_id)
);

CREATE TABLE issues (
    id           uuid PRIMARY KEY,
    tenant_id    uuid NOT NULL,
    project_id   uuid NOT NULL,
    -- Denormalised (project keys are immutable) so KEY-123 resolves without a remote call.
    project_key  text NOT NULL,
    number       bigint NOT NULL CHECK (number > 0),
    type_key     text NOT NULL,
    status       text NOT NULL,
    priority     text NOT NULL CHECK (priority IN ('highest', 'high', 'medium', 'low', 'lowest')),
    summary      text NOT NULL CHECK (char_length(summary) BETWEEN 1 AND 255),
    -- Rich text document (validated against an allow-list before it is stored).
    description  jsonb,
    assignee_id  uuid,
    reporter_id  uuid NOT NULL,
    creator_id   uuid NOT NULL,
    parent_id    uuid REFERENCES issues (id),
    labels       text[] NOT NULL DEFAULT '{}',
    story_points numeric(6, 1) CHECK (story_points >= 0),
    start_date   date,
    due_date     date,
    resolution   text,
    resolved_at  timestamptz,
    version      bigint NOT NULL DEFAULT 1,
    created_at   timestamptz NOT NULL DEFAULT now(),
    updated_at   timestamptz NOT NULL DEFAULT now(),
    deleted_at   timestamptz,
    UNIQUE (tenant_id, project_id, number)
);
CREATE INDEX issues_key_idx ON issues (tenant_id, project_key, number);
CREATE INDEX issues_status_idx ON issues (tenant_id, project_id, status) WHERE deleted_at IS NULL;
CREATE INDEX issues_assignee_idx ON issues (tenant_id, assignee_id) WHERE deleted_at IS NULL;
CREATE INDEX issues_parent_idx ON issues (tenant_id, parent_id) WHERE parent_id IS NOT NULL;
CREATE INDEX issues_labels_idx ON issues USING gin (labels);

CREATE TABLE comments (
    id         uuid PRIMARY KEY,
    tenant_id  uuid NOT NULL,
    issue_id   uuid NOT NULL REFERENCES issues (id) ON DELETE CASCADE,
    author_id  uuid NOT NULL,
    body       jsonb NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    edited_at  timestamptz,
    deleted_at timestamptz
);
CREATE INDEX comments_issue_idx ON comments (tenant_id, issue_id, created_at);

CREATE TABLE issue_links (
    id         uuid PRIMARY KEY,
    tenant_id  uuid NOT NULL,
    source_id  uuid NOT NULL REFERENCES issues (id) ON DELETE CASCADE,
    target_id  uuid NOT NULL REFERENCES issues (id) ON DELETE CASCADE,
    link_type  text NOT NULL CHECK (link_type IN ('blocks', 'relates', 'duplicates', 'depends_on')),
    created_by uuid NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    CHECK (source_id <> target_id),
    UNIQUE (tenant_id, source_id, target_id, link_type)
);
CREATE INDEX issue_links_target_idx ON issue_links (tenant_id, target_id);

-- Append-only change log.
CREATE TABLE issue_history (
    id            uuid PRIMARY KEY,
    tenant_id     uuid NOT NULL,
    issue_id      uuid NOT NULL REFERENCES issues (id) ON DELETE CASCADE,
    actor_id      uuid NOT NULL,
    issue_version bigint NOT NULL,
    -- [{ "field": "...", "from": ..., "to": ... }]
    changes       jsonb NOT NULL,
    created_at    timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX issue_history_issue_idx ON issue_history (tenant_id, issue_id, created_at);

CREATE TABLE watchers (
    tenant_id  uuid NOT NULL,
    issue_id   uuid NOT NULL REFERENCES issues (id) ON DELETE CASCADE,
    user_id    uuid NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, issue_id, user_id)
);

DO $$
DECLARE t text;
BEGIN
    FOREACH t IN ARRAY ARRAY['project_issue_counters', 'issues', 'comments', 'issue_links', 'issue_history', 'watchers'] LOOP
        EXECUTE format('ALTER TABLE %I ENABLE ROW LEVEL SECURITY', t);
        EXECUTE format('ALTER TABLE %I FORCE ROW LEVEL SECURITY', t);
        EXECUTE format(
            'CREATE POLICY tenant_isolation ON %I USING (tenant_id = app_tenant_id()) WITH CHECK (tenant_id = app_tenant_id())', t);
    END LOOP;
END $$;

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
