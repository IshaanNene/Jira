-- A local copy of the board-relevant fields of every issue, kept current by the
-- `agile-service.cards` consumer from issue events (each refreshes the issue from
-- issue-service) and filled per project the first time one of its boards loads. Boards and
-- backlogs read it instead of asking issue-service for every card on every load.
CREATE TABLE cards (
    tenant_id       uuid NOT NULL,
    issue_id        uuid NOT NULL,
    project_id      uuid NOT NULL,
    key             text NOT NULL DEFAULT '',
    number          bigint NOT NULL DEFAULT 0,
    issue_type      text NOT NULL DEFAULT '',
    status          text NOT NULL DEFAULT '',
    status_category text NOT NULL DEFAULT '',
    priority        text NOT NULL DEFAULT '',
    summary         text NOT NULL DEFAULT '',
    assignee_id     uuid,
    story_points    double precision,
    resolved_at     timestamptz,
    parent_key      text,
    parent_summary  text,
    labels          text[] NOT NULL DEFAULT '{}',
    -- The issue's version; a copy is only ever replaced by the same or a newer one.
    version         bigint NOT NULL DEFAULT 0,
    -- Deleted issues stay as tombstones so a late backfill or refresh cannot bring them back.
    deleted         boolean NOT NULL DEFAULT false,
    updated_at      timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, issue_id)
);
-- A board's cards: unfinished ones and those finished recently.
CREATE INDEX cards_board_idx ON cards (tenant_id, project_id, status_category, resolved_at) WHERE NOT deleted;
-- Children of an issue whose summary changed.
CREATE INDEX cards_parent_idx ON cards (tenant_id, parent_key) WHERE parent_key IS NOT NULL AND NOT deleted;

-- Projects whose cards have been copied in full once; from then on events keep them current.
CREATE TABLE card_projects (
    tenant_id  uuid NOT NULL,
    project_id uuid NOT NULL,
    synced_at  timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, project_id)
);

DO $$
DECLARE t text;
BEGIN
    FOREACH t IN ARRAY ARRAY['cards', 'card_projects'] LOOP
        EXECUTE format('ALTER TABLE %I ENABLE ROW LEVEL SECURITY', t);
        EXECUTE format('ALTER TABLE %I FORCE ROW LEVEL SECURITY', t);
        EXECUTE format(
            'CREATE POLICY tenant_isolation ON %I USING (tenant_id = app_tenant_id()) WITH CHECK (tenant_id = app_tenant_id())', t);
    END LOOP;
END $$;
