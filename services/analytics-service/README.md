# analytics-service

Dashboards and the charts on them. Dashboards are records in PostgreSQL (`analytics_db`); the numbers come from
facts in ClickHouse that the ingester derives from events. ClickHouse holds nothing of record: drop the database,
reset the consumer group's offsets, and it is rebuilt from the topics.

**Ingesting.** Consumer group `analytics-service` on the issue, agile and project topics:

| Event | Effect |
|---|---|
| `issue.created`, `issue.updated`, `issue.assigned` | current state fetched with `issue.v1.Issues/GetDocuments` and written to `issue_state`; `issue.created` is also recorded in `issue_events` |
| `issue.transitioned` | state refreshed; the transition recorded with from/to status and status category |
| `issue.deleted`, or an issue the source reports missing | a deleted row at the highest version, so nothing can bring it back |
| `sprint.started`, `sprint.completed` | recorded with the sprint name and issue counts |
| `project.deleted` | every issue of the project marked deleted |

`issue_events` is a ReplacingMergeTree keyed by event id and `issue_state` one versioned by the issue's version,
so redelivery, replays and out-of-order events are harmless. Queries read with `FINAL`.

**Charts.** `POST /api/v1/analytics/query` takes a widget query and answers its numbers:

| Kind | Options | Answer |
|---|---|---|
| `created_vs_resolved` | `projects`, `days` (7–365) | issues created and resolved per UTC day |
| `cycle_time` | `projects`, `weeks` (4–52) | per week of resolution: median and 85th-percentile days from the first move into an in-progress status, and the issue count |
| `breakdown` | `projects`, `group_by` (`status_category`, `status`, `type`, `priority`, `assignee`, `project`), `include_done` | counts per group (top 50) and the total |
| `cumulative_flow` | `projects`, `days` (7–365) | issues to do, in progress and done at the end of each UTC day; today matches the current state and earlier days are worked back through moves recorded with both categories |
| `time_in_status` | `projects`, `weeks` (4–52) | per status (top 50): median and mean days and the number of stays, for stays that ended in the period; a stay runs from the previous move (or creation) to the move out |
| `velocity` | `project`, `sprints` (3–20) | completed and not-completed issues of the last completed sprints, oldest first |

An empty `projects` means every project the caller can browse; listed projects are intersected with those, so a
hidden project contributes nothing. Each kind compiles to fixed SQL with typed parameters (`{name:Type}`), so no
client text becomes SQL.

**Isolation.** Three ClickHouse users: `analytics_owner` runs migrations, `analytics_writer` is the ingester's,
and `analytics_reader` (`readonly = 2`, 10 s, 100 000 rows) serves the API. The reader sees both tables through a
row policy, `tenant_id = toUUID(getSetting('SQL_tenant_id'))`, and the service sends `SQL_tenant_id` from the
verified token with every query, alongside the SQL's own tenant filter. Without the setting ClickHouse refuses the
query. Dashboards and widgets have forced row-level security in PostgreSQL.

**Dashboards.** `GET/POST /api/v1/dashboards`, `GET/PATCH/DELETE /api/v1/dashboards/{id}` (PATCH: name,
description, `shared`, `widget_order`), `POST /api/v1/dashboards/{id}/widgets`,
`PATCH/DELETE /api/v1/dashboards/{id}/widgets/{widget_id}`. A dashboard is private until its owner shares it with
the workspace; the owner edits it, and so do admins once it is shared; only the owner changes sharing. Someone who
cannot see a dashboard gets 404. Limits: 200 dashboards per owner, 30 charts per dashboard. Contract at
`/openapi.json`.

| Variable | Required | Default |
|---|---|---|
| `DATABASE_URL` (+ pool settings), `REDIS_URL`, `TOKEN_ISSUER`, `JWKS_URL`, `INTERNAL_SERVICE_SECRET` | yes | |
| `CLICKHOUSE_URL`, `CLICKHOUSE_{OWNER,WRITER,READER}_{USER,PASSWORD}` | yes | |
| `CLICKHOUSE_DATABASE` | no | `analytics` |
| `DATABASE_MIGRATION_URL` | for `migrate` | |
| `KAFKA_BROKERS` | no | unset: charts do not follow changes |
| `PERMISSION_SERVICE_GRPC_URL`, `ISSUE_SERVICE_GRPC_URL`, `TENANT_SERVICE_GRPC_URL` | no | local ports 9108 / 9105 / 9102 |
| `ALLOWED_ORIGINS`, `HTTP_ADDR` | no | –, `127.0.0.1:8113` |

`analytics-service migrate` applies the PostgreSQL migrations, then the ClickHouse tables, row policies and grants
(as the owner). The ClickHouse users themselves are created by `infrastructure/clickhouse/init/01-users.sql` in
development; elsewhere they are provisioned the same way.

`analytics-service rebuild <tenant-id>` restores one tenant's current issue state (`issue_state`) straight from
issue-service, as the writer, without replaying topics: it pages through every issue id, writes each issue's current
state (versioned, so a newer event still wins) and marks deleted any issue the store holds that issue-service no
longer lists and that was not touched while the rebuild ran. Running it again changes nothing. It needs the same
settings as the consumer.

**Health.** `/health/live`, `/health/ready` (PostgreSQL and ClickHouse), `/metrics`.

**Tests:** `cargo test -p analytics-service` (needs PostgreSQL and ClickHouse from `make infra`; each test uses its
own ClickHouse database).

**Not built yet.** Control chart, time in a status an issue is still in and other chart kinds; charts filtered by MQL;
history-aware created/resolved (a reopened issue loses its earlier resolution); time zones other than UTC;
materialized views or rollups; a rebuild of the event history (`rebuild` restores current state only);
sharing with specific people or groups.
