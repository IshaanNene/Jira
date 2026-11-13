# Implementation roadmap and status

This file is the honest ledger. A feature is **done** only when it works end to end (UI → API → auth → tenant →
authorization → database → outbox) and has tests. "Specified" means there is design but no code.

Legend: ✅ done and verified · 🚧 partly done · ⬜ not started

Last updated: 2026-11-13 (end of Phase 5).

## Phase status

| Phase | Scope | Status |
|---|---|---|
| 0 | Architecture, contracts, ADRs | ✅ |
| 1 | Monorepo, shared crates, identity, tenant registry, user/org, web shell, compose, gateway | ✅ (gaps listed below) |
| 2 | Project + issue engine, **permission-service core** (moved up from Phase 6) | ✅ (smaller gaps listed below) |
| 3 | Workflow engine + visual designer | ✅ (gaps listed below) |
| 4 | Boards, backlog, sprints, reports | ✅ (gaps listed below) |
| 5 | Kafka relay, consumers, search indexing, realtime gateway, notifications | ✅ (gaps listed below) |
| 6 | SSO/SCIM/MFA, admin, audit, QL search, automation, integrations | ⬜ |
| 7 | ClickHouse pipeline, dashboards | ⬜ |
| 8 | Kubernetes/Helm/Terraform/CI/CD, observability stack, load + failure testing | ⬜ |

Deviations from the brief's phase order, and why, are in [ARCHITECTURE.md](ARCHITECTURE.md) §7.

Against the brief's 45 final deliverables: roughly the foundations of items 1–6, 8, 9 (revocation list only),
15–16 (password auth and org-level RBAC only), 26–27, 34 (instrumentation, no collector stack), 36–37, 41–43, 45
exist, plus items 12 (attachment storage), 17–18 (issue and project management), 20 (workflow designer and
engine), plus the agile item (boards, backlog, sprints and sprint reports), and the event pipeline, realtime updates,
in-app and email notifications and full-text issue search.
Everything else is not started. Four of the fifteen logical services have no code.

## Implemented and verified

| Area | What works | Verified by |
|---|---|---|
| Shared crates | error model (problem+json ↔ gRPC), JWT verification with JWKS cache + revocation, org RBAC, service-to-service auth, tenant DB router, outbox writer, retry + circuit breaker, telemetry, test database harness | 40 unit tests |
| Tenancy | Tenant from signed token only; `DbRouter::begin` sets `app.tenant_id`; FORCE RLS on all tenant-owned tables; runtime role cannot bypass | `row_level_security_isolates_tenants_in_the_database`, `api_never_crosses_tenants` |
| tenant-service | Idempotent + concurrency-safe provisioning, least-loaded shared-cluster placement, resolve, abandon (compensation) | 4 unit + 6 integration tests |
| identity-service | Register, login, lockout, refresh rotation with reuse detection, org token exchange, logout, session list/revoke, JWKS, sealed signing keys | 5 unit + 9 integration tests |
| user-service | Profiles, organization create (saga with compensation), rename with `If-Match`, member list (keyset pagination + search), role change / suspend / remove / leave with ownership rules, invitations (create, list, revoke, preview, accept) | 6 unit + 10 integration tests |
| permission-service | Role bindings, project defaults, version bump only on real change, snapshot per user | 2 integration tests + 4 policy unit tests |
| Policy snapshots | Local evaluation in project/issue services; Redis version check makes grants and revocations effective on the next request across services | live: grant → access, revoke → 404, across three processes |
| project-service | Create (with creator grant + compensation), list by visibility, update with `If-Match`, archive/restore/delete, components, versions/release, member and default access | 1 unit + 5 integration tests |
| issue-service | Create with gap-free concurrent key allocation, hierarchy rules, list with filters + keyset pagination, edit with `If-Match` and history, status → resolution, delete, comments (author-only edit), links (visibility-aware), watchers, rich-text allow-list validation | 7 unit + 8 integration tests |
| Custom fields | Admin-defined fields (8 types) scoped by project and issue type; values validated against the definition on create and update; required fields; history | 4 unit tests + `custom_fields_are_defined_by_admins_and_validated_on_issues`; created and set in a real browser |
| Attachments | Upload to S3-compatible storage (10 MB, 50 per issue), type decided from file content, dangerous types refused, server-generated tenant-prefixed keys, authorized proxied download always as attachment | 3 unit tests + `attachments_are_validated_authorized_and_served_as_downloads`; upload/download exercised live against SeaweedFS |
| Components / fix versions on issues | Validated against the project (with one fresh re-check for just-created ones) | `components_and_versions_must_belong_to_the_project` |
| workflow-service | Declarative engine (conditions, validators, post-functions from closed sets), draft / publish with immutable versions and `If-Match`, publish-time validation incl. reachability, assignment per project and issue type with fallback order | 10 engine unit tests + 4 integration tests |
| Issues on workflows | Initial status, available transitions, and every status change decided by the engine; post-functions applied atomically with the move; refusals carry the engine's explanation; status names resolved per workflow with graceful fallback | `status_changes_are_decided_by_the_workflow_engine`; live: custom workflow assigned to tasks, skip refused, required field enforced, auto-assign and custom resolution applied |
| Workflow designer (web) | Draggable status diagram, connect statuses to add transitions, side panel for names, categories, initial status and rules, save draft / publish / discard, problems list, view-only for non-admins; workflow list, workspace default, per-project and per-type assignment | 10 web tests; opened and dragged in a real browser against a published workflow |
| agile-service | Scrum and Kanban boards with columns mapped to workflow statuses (explicit status or category catch-all), WIP limits, shared per-project ranking (fractional ranks, advisory-locked), sprints (plan, start with commitment snapshot, one active per board, complete moving open work, delete planned), burndown from a scope-change log, velocity, per-caller permissions on every board | 10 unit tests (ranks, columns, burndown) + 5 integration tests incl. cross-tenant isolation |
| Issue cards over gRPC | `issue.v1.Issues/ListCards` serves board cards per project (open + recently done, or by id), capped at 2000 with a `truncated` flag, service-token authenticated | `cards_are_served_to_other_services_per_project` |
| Boards and backlog (web) | Board with drag between columns (the move goes through the workflow; refusals put the card back with the engine's reason), search, "Only my issues", sprint header with time left; backlog with drag to rank and between sprints, row menus, create / edit / start / complete / delete sprint dialogs; board settings with a column editor; Reports with burndown and velocity charts (ECharts, light and dark) and issue tables | 13 web tests; driven in a real browser: plan a sprint, start it, drag a card To Do → In Progress (persisted with history), complete it, read the frozen report and velocity in both themes |
| Internal gRPC | identity → user (`CheckMembership`), user → tenant (`ProvisionTenant`, `ResolveTenant`, `AbandonTenant`), authenticated, with deadlines, bounded retry, breaker | exercised live across three processes |
| Outbox | Every write enqueues its event in the same transaction | asserted in integration tests |
| Gateway | Envoy: routing, request ids, rate limits, edge EdDSA JWT validation, identity headers stripped | manual curl checks (401 without/with bad token, 200 with valid) |
| Web: projects & issues | Top navigation with Projects menu and Create; Your work; project list, create, settings (details, access, components, releases); issue list with URL filters; issue view with inline editing, conflict handling, description editor, child issues, links, comments, history, watch; `c` shortcut | driven in a real browser: create project → create issue → change status → comment → history |
| Web app | Design system (light/dark), login, register, onboarding / workspace chooser, app shell (sidebar, workspace switcher, breadcrumbs, command palette, shortcuts), home, people + invitations, invitation acceptance, workspace settings with conflict handling, profile, sessions | 17 unit tests; register → create workspace → invite driven in a real browser against the running stack |
| Event pipeline | Transactional outbox → Kafka relay in every service (advisory-locked, one relay per database, ordered per aggregate, failures hold back later events of the same key), consumers with manual commits, bounded retry and a dead-letter topic, tenant required on every envelope, inbox de-duplication, retention sweep | 2 integration tests against real Kafka + PostgreSQL; live: every service's outbox drained to 0, including events held since Phase 1 |
| realtime-service | SSE change hints per project, per person and per workspace; Redis Streams buffer with cursor resume and `resync` when a gap is detected; access and session revocation re-checked every 60 s; token expiry ends the stream; connection cap | 4 integration tests against real Redis; live: an edit by another user appeared on an open backlog within 2 s |
| notification-service | Notifications for assignment, mention, comment and status change to reporter, assignee and watchers, only for active members who can still browse the project; never to the actor; idempotent on redelivery; list / unread count / mark read / mark all; hidden when access is lost | 3 unit + 2 integration tests incl. cross-tenant and cross-user isolation |
| Email notifications | Queued with each notification per the person's preferences, sent after a short delay unless read in the app first; leased sender safe across instances; retry with backoff, permanent failures recorded; escaped content, no header injection; per-workspace preferences (master switch + per kind) | 3 unit + 2 integration tests sending through a real Mailpit; live: an assignment by Maya reached Olivia's Mailpit inbox and rendered correctly |
| search-service | OpenSearch index fed from issue and project events (fresh document per event, external versioning, deletes), tenant-filtered and routed, limited to visible projects; key, prefix and typo-tolerant matching over summary, description, comments and labels; structured highlights (no HTML); cursor paging; `reindex <tenant>` that also drops vanished issues | 3 unit + 2 integration tests against real OpenSearch; live: reindexed the demo workspace, an edit was searchable within 3 s through Kafka |
| New internal RPCs | `issue.v1.Issues/GetAudience`, `GetDocuments` (≤ 500), `ListIssueIds` (≤ 1000); `user.v1.Membership/GetMembers` (≤ 500) | `audiences_and_search_documents_are_served_to_other_services`, `grpc_membership_check_reflects_role_and_status` |
| Web: live updates | Fetch-based SSE client with token refresh, jittered reconnect and cursor resume; project screens refetch only what a hint affects, batched and deferred while the person's own change is in flight; "Live updates paused" banner | 7 web tests; live in a real browser |
| Web: notifications and search | Bell with live unread badge, unread filter, mark all read, open-and-mark-read, link to settings; Settings → Notifications with email switches saved instantly; ⌘K palette searches issues as you type; `/search` page with project, status and "assigned to me" filters, highlighted matches, paging | 7 web tests; driven in a real browser in light and dark themes |
| Developer workflow | `make dev/stop/test/lint/migrate/seed/logs/clean/gen-api`, `.env.example`, seed script (idempotent, via public API) | each command run |

Totals: **175 Rust tests, 73 web tests, all passing**; `cargo clippy -D warnings`, `rustfmt --check`, `eslint`, `tsc` clean.

## Incomplete within Phase 5 scope

| Item | State |
|---|---|
| Email | One email per notification (no digests); no one-click unsubscribe; comment emails do not quote the comment; no bounce or complaint handling |
| Notification preferences | Email only (in-app is always on), per workspace. No notifications for sprint, project or workflow events |
| Agile consumer | Board placements and scope-log rows of deleted issues are still not cleaned up (they are ignored); boards still read cards from issue-service on each load |
| Search | No query language (Phase 6), no custom fields, components or versions in the index, no search for projects or people; `reindex` takes one tenant id; no index alias for mapping changes; the shared `search_db` database is unused |
| Realtime | No presence, no WebSocket transport, no per-tenant connection quotas; fan-out has not been load tested |
| Event pipeline | No DLQ replay tool or consumer-lag dashboards; delivery is at least once (consumers are idempotent), not exactly once |
| Search highlight of status names | Search results show the status key in title case (e.g. "In review"), not the workflow's display name, which the index does not carry |

## Incomplete within Phase 4 scope

| Item | State |
|---|---|
| Epics and subtasks on boards | Not shown (as in many Jira configurations, but not configurable here); no epic panel in the backlog |
| Swimlanes, quick filters, card layout settings | Not built; the board has text search and "Only my issues" |
| Issue data on boards | Fetched from issue-service on every board load (capped at 2000 cards, shown as truncated) instead of an event-fed local copy; the relay now exists but the consumer does not |
| Ranks | No rebalancing job; ranks grow longer when many moves land in the same gap. Ranking is per project, so two boards of one project share an order |
| Deleted issues | Their placements and scope-log rows are left behind (no agile consumer yet); they are ignored because the cards no longer exist |
| Reports | Burndown uses each issue's current estimate, not the estimate at the time; no cumulative flow, control chart, sprint retrospective or epic/version reports. The guideline starts from scope at start even when part of it was already done |
| Board drag in automated browsers | dnd-kit drops were verified with a scripted pointer sequence; there are no Playwright tests |
| Multi-select drag | The API moves several issues at once; the UI drags one at a time |
| Bundle size | The Reports chunk is ~520 kB (176 kB gzip), almost all ECharts, loaded only on that page; the main chunk (~676 kB) predates this phase and needs splitting |

Fixed during this phase: the Phase 3 migration that filled `status_category` for existing issues ran under FORCE
row-level security with no tenant set and updated nothing, so older issues showed in the wrong board column.
Migration `0004` re-runs it correctly, and the test database harness now runs migrations as a non-superuser owner
(as production does), so a migration that RLS silently skips fails its test. Every service now has a `build.rs`
so that a new migration file is always embedded in the binary.

## Incomplete within Phase 3 scope

| Item | State |
|---|---|
| Transition screens | Not built. A refused transition names the missing field; the person fills it in on the issue and retries (or sends both in one request through the API) |
| Removing a status that issues are in | No mapping step on publish. Such issues show the old key as text and can only move to the workflow's initial status |
| Status category on existing issues | Stored at transition time for filtering and board columns; if a published change alters a status's category, issues already in it keep the old category until they next move |
| Restore an earlier version | Versions are kept and listed by the API; there is no restore action or version browser in the UI |
| Designer | No undo, no zoom/pan beyond scrolling, straight arrows only; creating a transition is click-to-connect rather than dragging a line |
| Custom resolutions | Chosen from four built-in values in the designer (the API accepts any slug) |
| Real-time | A newly published workflow reaches issue screens within about 15 seconds (display cache); decisions always use the latest published version |

## Incomplete within Phase 2 scope

| Item | State |
|---|---|
| Custom issue types | Not built; the seven built-in types are fixed |
| Custom fields | No filtering or sorting by custom field in the issue list (arrives with the search service); renaming an option does not rewrite stored values |
| Attachments | No malware scanning (`scan_status` is recorded as `not_scanned`); no image previews; whole file is buffered in memory (fine at 10 MB); orphaned objects after a failed delete need a storage sweep that does not exist yet |
| Clone, move between projects, bulk edit, import/export, votes, time tracking | Not built |
| Mentions | Accepted and recorded by the API (`comment.created` carries mentioned ids); no picker in the editor, no notification |
| Permission schemes, custom roles, issue security levels | Not built; the role → permission table is fixed in code |
| Issue list virtualization | Not built; the list pages 50 at a time with "Load more" |
| "Assigned to me" | Queries the first 8 projects one by one until cross-project search exists |
| Web tests | Project list, issue list, issue view (inline edit, conflict, rollback, read-only) and attachment upload are covered; project settings, the create dialogs, comments and links are not. No Playwright end-to-end tests |
| Real-time refresh | None; other people's changes appear on refetch (window focus) or on a save conflict |

## Incomplete within Phase 1 scope

| Item | State |
|---|---|
| Outbox relay to Kafka | Not built (ADR-0004). Events accumulate unpublished; nothing consumes them yet, so no cross-service event propagation exists |
| Email verification, password reset, invitation email | Not built — need email delivery (Phase 5). Invitation links are shown once to the inviter instead. **Consequence: an invitation can be accepted by whoever registers the invited address first; email ownership is not proven** |
| Teams, departments, groups, avatars, org logo/branding, domains | Not built |
| Tenant tier migration; business/enterprise provisioning | Schema and placement decision only; dedicated tiers are recorded as `pending` and are not routable |
| MFA, WebAuthn, OIDC, SAML, SCIM, API tokens | Not built (Phase 6) |
| Per-IP rate limiting | Dev gateway has global and credential-route buckets only; per-IP needs the rate-limit service planned with Envoy Gateway |
| OpenTelemetry export | Code path exists (OTLP/HTTP) but has not been run against a collector |
| Playwright end-to-end tests, Storybook | Not set up |
| Contract tests | Event wire-format fixture only; no gRPC/REST compatibility check in CI (there is no CI yet) |
| Kubernetes, Helm, Terraform, GitHub Actions, Argo CD | Not started (Phase 8); `DEPLOYMENT_GUIDE.md` is design only |

## Known issues

- Crate versions are pinned one major behind current for several dependencies (sqlx 0.8, tonic 0.13, jsonwebtoken 9,
  utoipa 5, tower-http 0.6) to stay on APIs that were verified here. Upgrading is a tracked chore.
- SQL is runtime-checked (`sqlx::query_as`), not compile-time-checked macros; schema drift is caught by integration
  tests rather than the compiler.
- Session revocation fails open when Redis is unavailable (bounded by the 10-minute token lifetime) — by design, documented in SECURITY_DESIGN.md.
- Envoy logs one JWKS fetch error at boot if identity-service is not yet up; keys are then fetched on demand.
- Local object storage is SeaweedFS (S3 API): MinIO's images are no longer published on Docker Hub or Quay. Production points the same settings at a managed S3-compatible service.
- The interface follows Jira's layout conventions by request; it uses its own name, mark and Lucide icons, not Atlassian assets.
- Policy snapshots without Redis are trusted for up to 5 seconds, so a revocation can lag by that long when Redis is down.
- The web app's organization-role checks are a hand-written mirror of the server table (display only).

## Measured performance

No load tests have been run. The targets in the brief (1M users, 50k concurrent, p95 200/300 ms, 99.95 %)
are **design targets only** until k6 results are recorded here.

## Next

1. Close the remaining Phase 5 gaps: an agile consumer that drops placements of deleted issues, email digests and
   one-click unsubscribe.
2. Phase 6: SSO / SCIM / MFA, admin console, audit log, the issue query language on top of search-service,
   declarative automation, integrations.
