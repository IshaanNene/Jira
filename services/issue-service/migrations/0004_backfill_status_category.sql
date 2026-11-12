-- 0003 backfilled status_category with a plain UPDATE, but `issues` has FORCE ROW LEVEL
-- SECURITY and migrations run without a tenant, so that UPDATE matched no rows and every
-- pre-existing issue kept the default 'todo'. Repeat the backfill with the policy lifted
-- for this transaction only. Only the built-in status keys are mapped; issues created
-- since 0003 got their category from the workflow and are left alone.
ALTER TABLE issues NO FORCE ROW LEVEL SECURITY;
ALTER TABLE issues DISABLE ROW LEVEL SECURITY;

UPDATE issues SET status_category = CASE status WHEN 'done' THEN 'done' ELSE 'in_progress' END
WHERE status_category = 'todo' AND status IN ('in_progress', 'in_review', 'done');

ALTER TABLE issues ENABLE ROW LEVEL SECURITY;
ALTER TABLE issues FORCE ROW LEVEL SECURITY;
