# agile-service

**Responsibility.** Boards (Scrum and Kanban), the shared backlog order, sprints and their reports. It does not
own issues: issue data (status, points, assignee) is read from issue-service, and a status change made by dragging
a card goes to issue-service, where the workflow engine decides it.

**Boards.** A board belongs to one project and has ordered columns. A column lists workflow status keys and may
also catch every status of one category (`todo`, `in_progress`, `done`) that no column lists explicitly. An
explicit mapping wins over a category. A status no column shows is reported as `unmapped` so the board can say so.
New boards get To Do / In Progress / Done by category. Columns may carry a soft WIP limit (shown, not enforced).
Scrum boards show the running sprint; Kanban boards show every open issue plus recently finished ones.

**Ranking.** One order per project, shared by all of its boards and the backlog, stored as fractional base-36
strings (`rank.rs`) compared bytewise (`COLLATE "C"`). Moves are serialised per project with a transaction-scoped
advisory lock and are expressed relative to a neighbour (`before_id` / `after_id`). Issues without a stored rank
follow the ranked ones in creation order. Many moves into one gap make ranks longer; a move that produces a
rank over 12 characters queues the project (`rank_rebalance`, a content-free system queue), and a per-database
runner claims it with a lease and, under the same lock, rewrites every rank of the project evenly spaced in the
same order (6 characters or fewer for up to ~800,000 issues).

**Sprints.** `future → active → closed`. One active sprint per board (unique partial index). Starting needs at
least one issue and records the commitment (issues and points). Completing moves open issues to a planned sprint
or the backlog and freezes the report. Only planned sprints can be deleted; their issues return to the backlog.

**Reports.** Burndown: scope over time comes from `sprint_scope_events` (issues added or removed while the sprint
runs); completion comes from each issue's `resolved_at`. Points use each issue's current estimate. Velocity compares
commitment with completed work per closed sprint.

**API.** REST, contract at `/openapi.json`:

| Path | |
|---|---|
| `GET/POST /api/v1/boards?project_key=` | list / create |
| `GET/PATCH/DELETE /api/v1/boards/{id}` | `If-Match` for changes |
| `GET /api/v1/boards/{id}/view`, `/backlog` | board columns; planning view |
| `POST /api/v1/boards/{id}/move` | rank and move issues between sprints and the backlog |
| `GET/POST /api/v1/boards/{id}/sprints`, `GET /boards/{id}/velocity` | |
| `GET/PATCH/DELETE /api/v1/sprints/{id}`, `POST /start`, `POST /complete`, `GET /report` | |

It calls `project.v1.Projects/GetProjectConfig` and `issue.v1.Issues/ListCards` with the service token. It serves
no gRPC itself.

**Permissions.** Browse to view; Manage sprints (project member) to create, start, complete and plan sprints;
Edit issue to rank and move; Manage project to change or delete boards. Without Browse every board and sprint id
answers 404. The board response carries the caller's `permissions` so the UI can hide what they cannot do.

**Database.** Owns `agile_db`: `boards`, `sprints`, `issue_placements`, `sprint_scope_events` (all FORCE RLS), plus
`outbox` and `inbox`. Emits `board.updated`, `sprint.created`, `sprint.started`, `sprint.completed`,
`issue.ranked` on the agile topic.

**Cleanup consumer** (`agile-service.cleanup`, issue and project topics). `issue.deleted` removes the issue's
placement and, when it was in a running sprint, logs it as removed from scope so the burndown drops it (closed
sprints keep their frozen reports). `project.deleted` marks the project's boards deleted and removes its
placements. Inbox-deduplicated; tenant from the event envelope.

**Card copy** (`cards`, `card_projects`; consumer `agile-service.cards` on the issue topic). Boards and
backlogs read cards from a local copy instead of asking issue-service on every load. A project is copied the
first time one of its boards loads (`ListIssueIds` for the project a page of 1000 at a time, then the cards by
id); afterwards `issue.created`, `issue.updated`, `issue.transitioned` and `issue.assigned` make the consumer
fetch that issue's current card again, and its children's (up to 500) when it is someone's parent. Fetching
rather than applying the event means late, repeated or reordered events are harmless; a row is only replaced
by the same or a newer issue version, and `issue.deleted` leaves a tombstone so a racing copy cannot bring the
card back. A board shows at most 5000 cards (then `truncated`). Moves and sprint reports still check
issue-service directly. `project.deleted` drops the project's copy.

**Epics.** Board and backlog responses carry `epics`: the project's unfinished and recently finished epics (up
to 200, from the card copy), which the web pages offer as a filter; issues belong to an epic through `parent_key`.

**Not built yet.** Epics are not cards and subtasks are not shown on boards; swimlanes and quick filters (the web board has
search and "Only my issues"); cumulative flow and control charts.

| Variable | Required | Default |
|---|---|---|
| `DATABASE_URL`, `DATABASE_MIGRATION_URL` | yes / for `migrate` | |
| `REDIS_URL`, `TOKEN_ISSUER`, `JWKS_URL`, `INTERNAL_SERVICE_SECRET` | yes | |
| `TENANT_SERVICE_GRPC_URL`, `PERMISSION_SERVICE_GRPC_URL`, `PROJECT_SERVICE_GRPC_URL`, `ISSUE_SERVICE_GRPC_URL` | no | local ports 9102 / 9108 / 9104 / 9105 |
| `ALLOWED_ORIGINS`, `HTTP_ADDR` | no | –, `127.0.0.1:8107` |

**Run / test / observe / image.** As for tenant-service, with `agile-service`.
