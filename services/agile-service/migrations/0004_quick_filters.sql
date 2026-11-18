-- Saved quick filters of a board: [{ "id", "name", "query" }], each an MQL condition the
-- board and backlog can switch on. Queries are stored as text and run by search-service,
-- which parses them; they are never executed here.
ALTER TABLE boards ADD COLUMN quick_filters jsonb NOT NULL DEFAULT '[]';
