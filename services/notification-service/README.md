# notification-service

In-app notifications for issue activity, delivered live through realtime-service.

**What creates one.** Consumes the issue topic:

| Event | Who hears about it |
|---|---|
| `issue.assigned` | the new assignee (`assigned`) |
| `comment.created` | people mentioned (`mentioned`); reporter, assignee and watchers (`commented`) |
| `issue.transitioned` | reporter, assignee and watchers (`status_changed`) |

Never the person who acted, and only active workspace members who can still browse the project (their policy
snapshot is evaluated for that project). A mention wins over a plain comment notice. Events older than 24 hours
are ignored, so replaying history does not flood inboxes. The issue's audience comes from
`issue.v1.Issues/GetAudience`, names and roles from `user.v1.Membership/GetMembers`. A unique
`(tenant, recipient, source event)` key makes redelivery harmless. Every notification emits
`notification.created` through the outbox, which realtime-service routes to `user:<recipient>`.

**API.** REST, contract at `/openapi.json`; always the caller's own notifications, limited to projects they can
see now (losing access hides them):

| Path | |
|---|---|
| `GET /api/v1/notifications?limit&cursor&unread=true` | newest first, with `unread_count` |
| `GET /api/v1/notifications/unread-count` | |
| `POST /api/v1/notifications/{id}/read` | 204, or 404 when it is not yours |
| `POST /api/v1/notifications/read-all` | |

**Database.** Owns `notification_db`: `notifications` (FORCE RLS), `outbox`, `inbox`.

| Variable | Required | Default |
|---|---|---|
| `DATABASE_URL`, `DATABASE_MIGRATION_URL` | yes / for `migrate` | |
| `REDIS_URL`, `TOKEN_ISSUER`, `JWKS_URL`, `INTERNAL_SERVICE_SECRET` | yes | |
| `KAFKA_BROKERS` | no | unset: no notifications are created |
| `TENANT_SERVICE_GRPC_URL`, `PERMISSION_SERVICE_GRPC_URL`, `ISSUE_SERVICE_GRPC_URL`, `USER_SERVICE_GRPC_URL` | no | local ports 9102 / 9108 / 9105 / 9103 |
| `ALLOWED_ORIGINS`, `HTTP_ADDR` | no | –, `127.0.0.1:8110` |

**Tests:** `cargo test -p notification-service` (PostgreSQL).

**Not built yet.** Email delivery (and digests); per-person notification preferences; notifications for sprint,
project or workflow events; retention of old notifications.
