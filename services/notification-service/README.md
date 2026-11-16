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
retried after 1, 2, 4, 8 and 16 minutes; permanent ones (5xx) are marked `failed`. Finished rows
are deleted after 7 days. All names and summaries are HTML-escaped and flattened to one line in the subject; links
are built only from keys of the form `ABC-123` under `APP_URL`. Addresses are never logged. Locally, Mailpit
catches everything (inbox at http://localhost:58025).

**Digests.** A person can choose hourly or daily email instead of one per notification
(`email_digest`: `off`, `hourly`, `daily`). Their activity rows are then queued with `digest = true`, all sharing the
`send_after` of the window the first one opened. When it closes, the sender takes that person's due digest rows
together (at most 200), drops the ones read in the app or muted meanwhile, and sends one email listing the rest
(at most 50 lines, the rest counted). Its unsubscribe link (kind `activity_digest`) turns off the four activity
kinds and leaves saved-filter email alone. Saved-filter emails are never gathered.

**Bounces and complaints.** `email_suppressions` (global database, SHA-256 of the trimmed lower-cased address, no
address in the clear) lists addresses not to email; a row for one is finished as `suppressed` without sending. An
address goes on the list when the receiving server rejects the recipient (SMTP 550, 551, 553, or an unparseable
address), or when the mail provider reports a hard bounce or a spam complaint to `POST /hooks/email-events`:
`{"events":[{"type":"bounce"|"complaint","email":"…","permanent":true}]}`, at most 100 events and 256 KB, signed
with `EMAIL_EVENTS_SECRET` as `X-Meridian-Signature: sha256=<hex HMAC-SHA256 of the body>` (401 otherwise; the
route answers 404 when the secret is not set). Soft bounces are ignored. The person sees "Email is paused" in their
settings (`email_paused` on the preferences) and can resume it, which takes their own address off the list.

**Webhooks.** Workspace admins subscribe endpoints (`/api/v1/webhooks`) to issue and comment events, optionally for
some projects only. A second consumer group (`notification-service.webhooks`, issue topic) queues one row per
matching enabled webhook in `webhook_deliveries` (unique per webhook and event, so redelivery is harmless; events
older than a day or than the webhook are skipped). A sender loop per database leases due rows like email and POSTs
`{id, type, occurred_at, data}` with `X-Meridian-Event`, `X-Meridian-Delivery` and
`X-Meridian-Signature: t=<unix>,v1=<hex HMAC-SHA256 of "t.body">`. Endpoints must be HTTPS; the host is resolved at
send time, every address must be public (no loopback, private, link-local, CGNAT, metadata or IPv4-mapped
addresses), the connection is pinned to the checked address, and redirects are not followed; 10 s timeout. Non-2xx
answers are retried after 1, 2, 4, 8 and 16 minutes, then marked `failed`; 15 failed deliveries in a row, or one
`410 Gone`, turn the webhook off with the reason. Signing secrets (`whsec_…`) are shown once and stored encrypted
(AES-256-GCM, key derived from `INTERNAL_SERVICE_SECRET`). URLs and secrets are never logged.

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
| `GET/PUT /api/v1/notifications/preferences` | email on/off, per kind (assigned, mentioned, commented, status changed, saved filters), `email_digest`, and the read-only `email_paused` (`bounce`, `complaint` or null) for the caller's own address |
| `DELETE /api/v1/notifications/preferences/email-pause` | resume email to the caller's own address |
| `POST /hooks/email-events` | bounce and complaint reports from the mail provider, signed with `EMAIL_EVENTS_SECRET` |
| `GET/POST /api/v1/notifications/unsubscribe?token=` | public: describe the link / turn that kind off (`{"all": true}` for all email); 404 for invalid or expired links |

**Database.** Owns `notification_db`: `notifications` and `notification_preferences` (FORCE RLS); `email_deliveries`,
`outbox` and `inbox` are system queues without RLS that no API reads; `email_suppressions` (global database only)
holds address hashes, also without RLS.

| Variable | Required | Default |
|---|---|---|
| `DATABASE_URL`, `DATABASE_MIGRATION_URL` | yes / for `migrate` | |
| `REDIS_URL`, `TOKEN_ISSUER`, `JWKS_URL`, `INTERNAL_SERVICE_SECRET` | yes | |
| `KAFKA_BROKERS` | no | unset: no notifications are created |
| `SMTP_URL` | no | unset: no email. `smtp://host:port`, `smtp://host?tls=required`, `smtps://user:pass@host:465` |
| `APP_URL` | with `SMTP_URL` | base URL for links in emails |
| `EMAIL_FROM`, `EMAIL_DELAY_SECONDS` | no | `Meridian <notifications@meridian.localhost>`, 60 |
| `EMAIL_EVENTS_SECRET` | no | unset: bounce and complaint reports are not accepted |
| `TENANT_SERVICE_GRPC_URL`, `PERMISSION_SERVICE_GRPC_URL`, `ISSUE_SERVICE_GRPC_URL`, `USER_SERVICE_GRPC_URL` | no | local ports 9102 / 9108 / 9105 / 9103 |
| `WEBHOOKS_ALLOW_HTTP`, `WEBHOOKS_ALLOW_PRIVATE_NETWORKS` | no | unset (HTTPS to public addresses only); `true` for local development only |
| `ALLOWED_ORIGINS`, `HTTP_ADDR` | no | –, `127.0.0.1:8110` |

**Tests:** `cargo test -p notification-service` (PostgreSQL and Mailpit from `make infra`).

**Not built yet.** Adapters for a specific provider's bounce format (SES, Postmark, SendGrid; a small relay maps
them to the report above); asynchronous bounces sent back by mail to the From address; notifications for sprint, project or workflow events; retention of old in-app notifications; webhooks for
sprint, project or workflow events, delivery ordering guarantees, manual redelivery, a payload format other than the event contract.
