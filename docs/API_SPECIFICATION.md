# API specification

Each service serves its own machine-readable contract at `/openapi.json` (generated from the handlers with
`utoipa`), reachable through the gateway at `/api/docs/<service>/openapi.json`. Load it in any OpenAPI viewer; the web app's types are generated from it (`make gen-api`). Those documents are authoritative; this file
defines the conventions and the resource map. gRPC contracts live in [`proto/`](../proto).

## 1. REST conventions

| Topic | Rule |
|---|---|
| Base | `/api/v1/...`; breaking changes get `/api/v2`, old version supported for a published window |
| Auth | `Authorization: Bearer <access token>`; refresh token only in an `HttpOnly; Secure; SameSite=Strict` cookie scoped to `/api/v1/auth` |
| Tenant | taken from the token. Never from a header, query or body |
| Media | `application/json`; errors are `application/problem+json` (RFC 9457) |
| IDs | UUIDs; issues are also addressable by key (`PAY-101`) |
| Concurrency | responses for versioned resources carry `ETag: "<version>"`; `PATCH/PUT/DELETE` require `If-Match` → `412` if absent, `409 version_conflict` (with current entity) on mismatch |
| Idempotency | `POST` that creates accepts `Idempotency-Key`; replays within 24 h return the original response |
| Pagination | `?limit=` (default 50, max 200) `&cursor=`; response `{ "items": [...], "next_cursor": "..." | null }` |
| Filtering / sort | explicit allow-listed query params per endpoint; free-form querying only through the search API |
| Timestamps | RFC 3339 UTC |
| Correlation | `X-Request-Id` accepted or generated, echoed; W3C `traceparent` propagated |
| Rate limits | `429` with `Retry-After`, `RateLimit-*` headers |
| Long operations | `202 Accepted` + `{ "job_id" }`; progress on `GET /rt/sse/jobs/{job_id}` |

### Error body

```json
{
  "type": "https://errors.platform.dev/validation_failed",
  "title": "Validation failed",
  "status": 422,
  "code": "validation_failed",
  "detail": "One or more fields are invalid.",
  "request_id": "0195...",
  "errors": [{ "field": "email", "code": "invalid_email", "message": "Enter a valid email address." }]
}
```

Stable `code` values: `unauthenticated`, `forbidden`, `not_found`, `validation_failed`, `version_conflict`,
`conflict`, `rate_limited`, `precondition_required`, `tenant_unavailable`, `dependency_unavailable`, `internal`.
`not_found` is returned instead of `forbidden` when revealing existence would leak information.

## 2. Resource map

✅ = implemented and exercised, 🚧 = in progress, ⬜ = specified, not built. Kept in sync with the roadmap.

### Identity — `/api/v1/auth`
| Method | Path | Purpose | |
|---|---|---|---|
| POST | `/register` | create account (email, password, display name) | ✅ |
| POST | `/login` | password login → access token + refresh cookie, or `202` with an `mfa_token` when two-step verification is on | ✅ |
| POST | `/token/refresh` | rotate refresh token | ✅ |
| POST | `/token/exchange` | account/organization token → token for `organization_id` | ✅ |
| POST | `/logout` | revoke current session | ✅ |
| GET | `/me` | current account | ✅ |
| GET / DELETE | `/sessions`, `/sessions/{id}` | list / revoke devices | ✅ |
| POST | `/email/verify`, `/password/forgot`, `/password/reset` | email flows | ⬜ |
| GET | `/.well-known/jwks.json` | public signing keys | ✅ |
| POST | `/login/mfa` | second step after `/login` answered `202` with `mfa_token`: authenticator or recovery code → access token + refresh cookie | ✅ |
| GET | `/mfa` | two-step verification status, recovery codes left | ✅ |
| POST | `/mfa/totp/setup`, `/mfa/totp/confirm` | start (password) / finish (code) set-up; finishing returns recovery codes once | ✅ |
| POST | `/mfa/recovery-codes`, `/mfa/totp/disable` | new recovery codes / turn off (password and a code) | ✅ |
| … | `/passkeys/*`, `/api-tokens`, `/sso/*` (OIDC and SAML; built: `sso/saml/{organization_id}` serves the workspace's SP metadata, `sso/saml/acs` takes the provider's POST, `sso/saml/continue` finishes in the browser that started) | Phase 6 | ✅ |

### Organizations & users — `/api/v1`
| Method | Path | Purpose | |
|---|---|---|---|
| GET / POST | `/organizations` | my organizations / create (provisions tenant) | ✅ |
| GET / PATCH | `/organizations/current` | profile, settings | ✅ |
| GET | `/organizations/current/members` | list members | ✅ |
| PATCH / DELETE | `/organizations/current/members/{user_id}` | change role, suspend / remove | ✅ |
| GET / POST | `/organizations/current/invitations` | list / invite | ✅ |
| DELETE | `/organizations/current/invitations/{id}` | revoke | ✅ |
| GET | `/invitations/{token}` | preview an invitation (public, token is the secret) | ✅ |
| POST | `/invitations/{token}/accept` | accept as the signed-in account | ✅ |
| GET / POST / PATCH / DELETE | `/teams`, `/teams/{id}`, `/teams/{id}/members` | teams & departments | ⬜ |
| GET / PATCH | `/users/me` | own profile | ✅ |
| GET | `/users/{id}`, `/users?query=` | directory search | ⬜ |
| GET | `/organizations/current/audit` | audit log, newest first (`category`, `actor`, `from`, `to`, `limit`, `cursor`); admins and owners | ✅ |
| GET | `/organizations/current/audit/export` | same filters as CSV (≤ 50 000 rows) | ✅ |
| GET | `/organizations/current/audit/verify` | recompute the hash chain; first broken entry and why | ✅ |
| GET / POST | `/organizations/current/provisioning/tokens` | SCIM provisioning tokens (admins); the secret only in the create response, with the SCIM base URL | ✅ |
| DELETE | `/organizations/current/provisioning/tokens/{id}` | revoke a provisioning token | ✅ |
| GET | `/organizations/current/provisioning/users` | provisioned people and where each stands (`member`, `suspended`, `invited`, `invitation_expired`, `not_invited`, `deactivated`), and the role their groups give them | ✅ |
| GET | `/scim/v2/ServiceProviderConfig`, `/ResourceTypes`, `/Schemas` | SCIM discovery (no token needed) | ✅ |
| GET / POST | `/scim/v2/Users` | list (`filter=userName eq "…"` and `externalId`, `emails.value`, `id`; `startIndex`, `count` ≤ 200) and provision; provisioning token, `application/scim+json` | ✅ |
| GET / PUT / PATCH / DELETE | `/scim/v2/Users/{id}` | read, replace, patch (`add`/`replace`/`remove`), deprovision | ✅ |
| GET / POST | `/scim/v2/Groups` | list (`filter=displayName eq "…"` and `externalId`, `id`; `excludedAttributes=members`) and create; members must be provisioned users | ✅ |
| GET / PUT / PATCH / DELETE | `/scim/v2/Groups/{id}` | read, replace, patch (members `add`/`remove` with value lists or `members[value eq "…"]`, `replace`), delete | ✅ |
| GET | `/organizations/current/provisioning/groups` | the provider's groups with member counts and roles (admins) | ✅ |
| PUT | `/organizations/current/provisioning/groups/{id}/role` | `{org_role: "admin" \| "member" \| "guest" \| null}`; members' roles change at once; audited | ✅ |
| … | `/groups`, `/domains` | Phase 6 | ⬜ |

### Later phases (paths fixed now so the frontend router and gateway routes are stable)
| Area | Paths |
|---|---|
| Projects ✅ | `/projects`, `/projects/{key}` (+`/archive`, `/restore`), `/projects/{key}/components`, `/versions` (+`/release`), `/members/{user_id}`, `/access` |
| Issues ✅ (partly) | built: `/issues`, `/issues/meta`, `/issues/{key}`, `/issues/{key}/comments`, `/links`, `/history`, `/watch`, `/attachments` (+`/{id}/content`), `/custom-fields`. Not built: `/transitions`, `/issues/bulk`, `/issue-types` |
| Workflows ✅ | `/workflows`, `/workflows/{id}` (+`/draft`, `/publish`, `/versions`), `/workflow-assignments`, `/workflow-statuses`; issues: `/issues/{key}/transitions` |
| Agile ✅ | `/boards` (`?project_key=`), `/boards/{id}` (columns are part of the board), `/view`, `/backlog`, `/move` (rank and sprint membership), `/sprints`, `/velocity`; `/sprints/{id}` (+`/start`, `/complete`, `/report`) |
| Permissions | `/roles`, `/permission-schemes`, `/permissions/check`, `/permissions/mine` |
| Search 🚧 | built: `GET /search/issues?q&project_id&status_category&assignee&limit&cursor` (text search, structured highlights); `GET /search/issues/query?mql&limit&cursor` (MQL; 422 with `errors[0].span` for an invalid query). Not built: `/filters`, `/filters/{id}/subscriptions` |
| Notifications ✅ | `/notifications` (`?unread=true`), `/notifications/unread-count`, `/notifications/{id}/read`, `/notifications/read-all`, `/notifications/preferences` (GET/PUT email choices), `/notifications/unsubscribe?token=` (public, GET describes / POST turns off; RFC 8058) |
| Realtime ✅ | `GET /rt/stream?channel=project:{id}\|user:{id}\|org` (Server-Sent Events: `ready`, `change`, `resync`, `expired`, `denied`; resume with `Last-Event-ID`) |
| Automation | `/automation/rules`, `/automation/rules/{id}/executions` |
| Integrations | `/integrations`, `/webhooks`, `/webhooks/{id}/deliveries`, `/hooks/{provider}` (inbound) |
| Analytics ✅ | `/dashboards` (GET, POST), `/dashboards/{id}` (GET, PATCH name / description / shared / `widget_order`, DELETE), `/dashboards/{id}/widgets` (POST), `/dashboards/{id}/widgets/{widget_id}` (PATCH, DELETE), `POST /analytics/query` (a widget query → its numbers; 503 when ClickHouse is down) |
| Jobs | `/imports`, `/exports`, `/jobs/{id}` |

## 3. Internal gRPC

Package per service under `proto/platform/<service>/v1`. Rules: every RPC has a deadline; unary only unless
streaming is the point (reindex); fields are only added, never renumbered; `buf breaking`-style compatibility is
checked in CI.

| Service | RPC | Used by |
|---|---|---|
| `tenant.v1.TenantRegistry` | `ProvisionTenant`, `ResolveTenant`, `GetTenantByOrganization` | user, all (router) |
| `user.v1.Membership` | `CheckMembership(user_id, organization_id) → {tenant_id, org_role, status}`, `GetMembers(tenant_id, user_ids ≤ 500) → {role, status, name, email}`, `FindMembers(tenant_id, terms ≤ 100) → members whose email or display name equals a term, ignoring case` (built) | identity, issue, notification, search |
| `permission.v1.Authorizer` | `GetPolicySnapshot(tenant, user) → {version, grants}`, `Check` | all |
| `workflow.v1.WorkflowEngine` | `GetInitialStatus`, `ValidateTransition`, `ListTransitions` | issue |
| `project.v1.Projects` | `GetProjectConfig` | issue, agile |
| `issue.v1.Issues` | built: `ListCards`, `GetAudience`, `GetDocuments` (≤ 500), `ListIssueIds` (paged, for reindex); planned: `GetIssue`, `ApplyAction` (automation) | agile, notification, search; automation |

**Service authentication**: mTLS at the mesh level in Kubernetes, plus an application-level service token
(short-lived EdDSA JWT, `aud` = callee) in metadata so that authorization does not depend on network position.
The end-user context (`user_id`, `tenant_id`, `request_id`) is forwarded in metadata and trusted **only** when the
service token verifies.
