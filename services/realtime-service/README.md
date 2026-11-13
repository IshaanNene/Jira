# realtime-service

Pushes **change hints** to open browsers so boards, backlogs, issue views and the notification bell refresh
without reloading. A hint says *what* changed (`{type, id, version, key?, actor?, at}`), never the new data: the
browser refetches through the normal APIs, which apply every permission check again (ADR-0008).

**Flow.** Consumes the issue, agile, project, workflow and notification topics. Each event is routed to channels:
issue and agile events to `project:<id>`, project events to the project and `org`, workflow events to `org`,
`notification.created` to `user:<recipient>`. Each channel is a Redis Stream `rt:{tenant}:{channel}` (about 1000
entries, 24-hour TTL); a Redis Pub/Sub message on `rt:fanout` wakes every instance, which hands the hint to the
streams it has open. Events older than 5 minutes are not announced.

**Stream.** `GET /api/v1/rt/stream?channel=project:<id>|user:<id>|org` as Server-Sent Events with a bearer token
(the web client uses `fetch`, since `EventSource` cannot send headers). Events: `ready` (with the current cursor),
`change`, `resync` (the cursor is older than what the stream keeps: refetch everything), `expired` (the token ran
out: reconnect with a fresh one) and `denied` (access was withdrawn). Clients resume with `Last-Event-ID` or
`?cursor=`. Project access is checked when the stream opens and again every 60 seconds, together with session
revocation; a project the caller cannot browse, or another person's `user:` channel, answers 404.
Connections are capped per instance by a semaphore.

| Variable | Required | Default |
|---|---|---|
| `REDIS_URL`, `TOKEN_ISSUER`, `JWKS_URL`, `INTERNAL_SERVICE_SECRET` | yes | |
| `KAFKA_BROKERS` | no | unset: streams open but nothing is announced |
| `PERMISSION_SERVICE_GRPC_URL` | no | `http://127.0.0.1:9108` |
| `REALTIME_MAX_CONNECTIONS` | no | 10000 |
| `ALLOWED_ORIGINS`, `HTTP_ADDR` | no | –, `127.0.0.1:8109` |

It has no database (`migrate` does nothing). **Tests:** `cargo test -p realtime-service` (needs Redis).

**Not built yet.** Presence ("who is viewing"); WebSockets; per-tenant connection quotas; load testing of fan-out.
