-- Statuses now come from the workflow service. The category (todo / in_progress / done)
-- of an issue's status is kept on the row so lists and filters need no remote call;
-- it is written whenever the issue is created or changes status.
ALTER TABLE issues ADD COLUMN status_category text NOT NULL DEFAULT 'todo'
    CHECK (status_category IN ('todo', 'in_progress', 'done'));
UPDATE issues SET status_category = CASE status
    WHEN 'done' THEN 'done' WHEN 'in_progress' THEN 'in_progress' WHEN 'in_review' THEN 'in_progress' ELSE 'todo' END;
CREATE INDEX issues_status_category_idx ON issues (tenant_id, project_id, status_category) WHERE deleted_at IS NULL;
