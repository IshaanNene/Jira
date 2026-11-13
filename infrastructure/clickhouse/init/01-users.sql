-- Development-only bootstrap, run once by the image as analytics_owner (see docker-compose.yml).
--   analytics_owner   runs migrations (tables, row policies)
--   analytics_writer  the ingester: INSERT and SELECT, nothing else
--   analytics_reader  dashboard queries: read-only, and every row it can see is filtered to
--                     the tenant the service names per query (row policies in the migrations)
-- Passwords equal user names on purpose; production users come from Terraform + secrets.
CREATE USER IF NOT EXISTS analytics_writer IDENTIFIED BY 'analytics_writer';
GRANT INSERT, SELECT ON analytics.* TO analytics_writer;

CREATE USER IF NOT EXISTS analytics_reader IDENTIFIED BY 'analytics_reader'
    SETTINGS readonly = 2, max_execution_time = 10, max_result_rows = 100000, result_overflow_mode = 'throw';
GRANT SELECT ON analytics.* TO analytics_reader;
