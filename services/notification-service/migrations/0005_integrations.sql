-- Integrations: Slack channels as a kind of outgoing webhook, and GitHub repositories that
-- link commits, branches and pull requests to issues.

-- A Slack incoming-webhook URL is itself the credential: for `slack` webhooks it is stored
-- sealed in `secret_sealed`, and `url` keeps only a redacted form for display.
ALTER TABLE webhooks ADD COLUMN kind text NOT NULL DEFAULT 'webhook' CHECK (kind IN ('webhook', 'slack'));

-- A workspace's connection to a source-control provider. GitHub signs each delivery with the
-- connection's secret (sealed, shown once); only keys of the chosen projects are linked.
CREATE TABLE source_connections (
    id            uuid PRIMARY KEY,
    tenant_id     uuid NOT NULL,
    provider      text NOT NULL CHECK (provider IN ('github')),
    name          text NOT NULL CHECK (char_length(name) BETWEEN 1 AND 100),
    -- Resolved from project-service when saved; keys never change.
    project_ids   uuid[] NOT NULL CHECK (cardinality(project_ids) BETWEEN 1 AND 50),
    project_keys  text[] NOT NULL CHECK (cardinality(project_keys) = cardinality(project_ids)),
    secret_sealed bytea NOT NULL,
    last_event_at timestamptz,
    created_by    uuid NOT NULL,
    created_at    timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX source_connections_tenant_idx ON source_connections (tenant_id, created_at);

-- Commits, branches and pull requests that mention an issue key.
CREATE TABLE dev_links (
    id            uuid PRIMARY KEY,
    tenant_id     uuid NOT NULL,
    connection_id uuid NOT NULL REFERENCES source_connections (id) ON DELETE CASCADE,
    project_id    uuid NOT NULL,
    issue_key     text NOT NULL CHECK (char_length(issue_key) <= 30),
    kind          text NOT NULL CHECK (kind IN ('commit', 'branch', 'pull_request')),
    repository    text NOT NULL CHECK (char_length(repository) <= 200),
    -- Commit SHA, branch name or pull request number.
    ref           text NOT NULL CHECK (char_length(ref) <= 255),
    title         text NOT NULL CHECK (char_length(title) <= 300),
    url           text NOT NULL CHECK (char_length(url) <= 1000),
    -- Pull requests: open | closed | merged.
    state         text CHECK (state IN ('open', 'closed', 'merged')),
    author        text CHECK (char_length(author) <= 100),
    occurred_at   timestamptz NOT NULL,
    updated_at    timestamptz NOT NULL DEFAULT now(),
    UNIQUE (tenant_id, issue_key, kind, repository, ref)
);
CREATE INDEX dev_links_issue_idx ON dev_links (tenant_id, issue_key, occurred_at DESC);

DO $$
DECLARE t text;
BEGIN
    FOREACH t IN ARRAY ARRAY['source_connections', 'dev_links'] LOOP
        EXECUTE format('ALTER TABLE %I ENABLE ROW LEVEL SECURITY', t);
        EXECUTE format('ALTER TABLE %I FORCE ROW LEVEL SECURITY', t);
        EXECUTE format(
            'CREATE POLICY tenant_isolation ON %I USING (tenant_id = app_tenant_id()) WITH CHECK (tenant_id = app_tenant_id())', t);
    END LOOP;
END $$;

-- Which workspace an inbound delivery's connection id belongs to, read before any tenant is
-- known. Holds ids only; the delivery is trusted only once its signature checks out against
-- the connection's secret, loaded under that workspace's row-level security.
CREATE TABLE source_hook_routes (
    connection_id uuid PRIMARY KEY,
    tenant_id     uuid NOT NULL
);
