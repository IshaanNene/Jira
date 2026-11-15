-- Optimistic concurrency: every change to a dashboard or its charts bumps the version, and each
-- edit names the version it was based on (If-Match), so two editors cannot overwrite each other.
ALTER TABLE dashboards ADD COLUMN version bigint NOT NULL DEFAULT 1;
