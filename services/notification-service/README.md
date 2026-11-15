# notification-service

In-app notifications for issue activity, delivered live through realtime-service.

**What creates one.** Consumes the issue topic (and the search topic, for filter digests):

| Event | Who hears about it |
|---|---|
| `issue.assigned` | the new assignee (`assigned`) |
| `comment.created` | people mentioned (`mentioned`); reporter, assignee and watchers (`commented`) |
| `issue.mentioned` | people newly mentioned in a description or an edited comment (`mentioned`) only |
| `issue.transitioned` | reporter, assignee and watchers (`status_changed`) |

Never the person who acted, and only active workspace members who can still browse the project (their policy
snapshot is evaluated for that project). A mention wins over a plain comment notice. Events older than 24 hours
are ignored, so replaying history does not flood inboxes. The issue's audience comes from
`issue.v1.Issues/GetAudience`, names and roles from `user.v1.Membership/GetMembers`. A unique
`(tenant, recipient, source event)` key makes redelivery harmless. Every notification emits
`notification.created` through the outbox, which realtime-service routes to `user:<recipient>`.

**Email.** When `SMTP_URL` is set, each new notification is also queued in `email_deliveries` for people whose
preferences allow that kind and whose member record has an address (`GetMembers`). A sender loop per database
leases due rows (`FOR UPDATE SKIP LOCKED`, 5-minute lease so a crashed instance's rows are retried) and sends a
plain-text + HTML message with lettre. Each email waits `EMAIL_DELAY_SECONDS` (default 60) first and is skipped if
the notification was read in the app meanwhile or the person has turned that kind off. Temporary SMTP failures are
retried after 1, 2, 4, 8 and 16 minutes; permanent ones (5xx, invalid address) are marked `failed`. Finished rows
are deleted after 7 days. All names and summaries are HTML-escaped and flattened to one line in the subject; links
are built only from keys of the form `ABC-123` under `APP_URL`. Addresses are never logged. Locally, Mailpit
catches everything (inbox at http://localhost:58025).

**Filter digests.** The consumer also reads `platform.search.v1`: a `filter.digest` from search-service (a
subscribed saved filter ran) becomes one email to the subscriber listing the issues it matched, at most 20, with
the total and a link to the filter. search-service has already limited the issues to what the subscriber can see.
There is no in-app notification; the email is skipped if the person is inactive, has no address, or has turned off
`filter_digest` (the "Saved filter subscriptions" preference, on unless turned off), and is not delayed. The event
id keys the delivery, so redelivery sends nothing twice; digests older than 24 hours are dropped.

**Unsubscribe.** Every email carries a signed link (HMAC-SHA256 over workspace, person, kind and issue time, key
derived from `INTERNAL_SERVICE_SECRET` with a purpose label, valid 90 days) in its footer and in
`List-Unsubscribe` / `List-Unsubscribe-Post: List-Unsubscribe=One-Click` headers (RFC 8058). The links work signed
out and can only turn email off. The footer opens the web page `/unsubscribe`, which asks first; mail clients POST
directly. `GET` never changes anything, so link-scanning mail filters cannot unsubscribe anyone.

**API.** REST, contract at `/openapi.json`; always the caller's own notifications, limited to projects they can
see now (losing access hides them):

| Path | |
|---|---|
| `GET /api/v1/notifications?limit&cursor&unread=true` | newest first, with `unread_count` |
| `GET /api/v1/notifications/unread-count` | |
| `POST /api/v1/notifications/{id}/read` | 204, or 404 when it is not yours |
| `POST /api/v1/notifications/read-all` | |
| `GET/PUT /api/v1/notifications/preferences` | email on/off, and per kind (assigned, mentioned, commented, status changed) |
| `GET/POST /api/v1/notifications/unsubscribe?token=` | public: describe the link / turn that kind off (`{"all": true}` for all email); 404 for invalid or expired links |

**Database.** Owns `notification_db`: `notifications` and `notification_preferences` (FORCE RLS); `email_deliveries`,
`outbox` and `inbox` are system queues without RLS that no API reads.

| Variable | Required | Default |
|---|---|---|
| `DATABASE_URL`, `DATABASE_MIGRATION_URL` | yes / for `migrate` | |
| `REDIS_URL`, `TOKEN_ISSUER`, `JWKS_URL`, `INTERNAL_SERVICE_SECRET` | yes | |
| `KAFKA_BROKERS` | no | unset: no notifications are created |
| `SMTP_URL` | no | unset: no email. `smtp://host:port`, `smtp://host?tls=required`, `smtps://user:pass@host:465` |
| `APP_URL` | with `SMTP_URL` | base URL for links in emails |
| `EMAIL_FROM`, `EMAIL_DELAY_SECONDS` | no | `Meridian <notifications@meridian.localhost>`, 60 |
| `TENANT_SERVICE_GRPC_URL`, `PERMISSION_SERVICE_GRPC_URL`, `ISSUE_SERVICE_GRPC_URL`, `USER_SERVICE_GRPC_URL` | no | local ports 9102 / 9108 / 9105 / 9103 |
| `ALLOWED_ORIGINS`, `HTTP_ADDR` | no | –, `127.0.0.1:8110` |

**Tests:** `cargo test -p notification-service` (PostgreSQL and Mailpit from `make infra`).

**Not built yet.** Digests of several notifications in one email (one email per notification today); the comment text in comment emails; bounce and complaint handling from the
mail provider; notifications for sprint, project or workflow events; retention of old in-app notifications.
