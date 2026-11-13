# Implementation roadmap and status

This file is the honest ledger. A feature is **done** only when it works end to end (UI → API → auth → tenant →
authorization → database → outbox) and has tests. "Specified" means there is design but no code.

Legend: ✅ done and verified · 🚧 partly done · ⬜ not started

Last updated: 2026-11-13 (Phase 7: ClickHouse pipeline and dashboards; Phase 6: issue query language, audit log, two-step verification).

## Phase status

| Phase | Scope | Status |
|---|---|---|
| 0 | Architecture, contracts, ADRs | ✅ |
| 1 | Monorepo, shared crates, identity, tenant registry, user/org, web shell, compose, gateway | ✅ (gaps listed below) |
| 2 | Project + issue engine, **permission-service core** (moved up from Phase 6) | ✅ (smaller gaps listed below) |
| 3 | Workflow engine + visual designer | ✅ (gaps listed below) |
| 4 | Boards, backlog, sprints, reports | ✅ (gaps listed below) |
| 5 | Kafka relay, consumers, search indexing, realtime gateway, notifications | ✅ (gaps listed below) |
| 6 | SSO/SCIM/MFA, admin, audit, QL search, automation, integrations | 🚧 (query language, audit log and authenticator-app two-step verification done; the rest not started) |
| 7 | ClickHouse pipeline, dashboards | ✅ (four chart kinds; gaps listed below) |
| 8 | Kubernetes/Helm/Terraform/CI/CD, observability stack, load + failure testing | ⬜ |

Deviations from the brief's phase order, and why, are in [ARCHITECTURE.md](ARCHITECTURE.md) §7.

Against the brief's 45 final deliverables: roughly the foundations of items 1–6, 8, 9 (revocation list only),
15–16 (password auth and org-level RBAC only), 26–27, 34 (instrumentation, no collector stack), 36–37, 41–43, 45
exist, plus items 12 (attachment storage), 17–18 (issue and project management), 20 (workflow designer and
engine), plus the agile item (boards, backlog, sprints and sprint reports), and the event pipeline, realtime updates,
in-app and email notifications, full-text issue search, and dashboards over a ClickHouse pipeline.
Everything else is not started. Three of the fifteen logical services have no code.

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
| One-click unsubscribe and board cleanup | Signed, expiring, off-only unsubscribe links in every email (footer page + RFC 8058 headers), public through the gateway for that one path; agile consumer removes placements of deleted issues (scope removal logged for running sprints) and retires boards of deleted projects | 2 unit + 1 integration test (headers and link read from a real Mailpit message), 1 agile integration test, 2 web tests; live: unsubscribed from a real email's link in the browser |
| search-service | OpenSearch index fed from issue and project events (fresh document per event, external versioning, deletes), tenant-filtered and routed, limited to visible projects; key, prefix and typo-tolerant matching over summary, description, comments and labels; structured highlights (no HTML); cursor paging; `reindex <tenant>` that also drops vanished issues | 3 unit + 2 integration tests against real OpenSearch; live: reindexed the demo workspace, an edit was searchable within 3 s through Kafka |
| New internal RPCs | `issue.v1.Issues/GetAudience`, `GetDocuments` (≤ 500), `ListIssueIds` (≤ 1000); `user.v1.Membership/GetMembers` (≤ 500), `FindMembers` (≤ 100 terms) | `audiences_and_search_documents_are_served_to_other_services`, `grpc_membership_check_reflects_role_and_status` |
| Issue query language (MQL) | Fields, operators, AND / OR / NOT, parentheses, lists, EMPTY, people by `currentUser()` / email / exact name, relative and absolute dates, text match with prefixes, ORDER BY with keys sorted by number and priorities by rank; compiled server-side inside the tenant and visibility filters; errors with a character span; size and depth limits; guests limited to `currentUser()` | 8 unit + 1 integration test against real OpenSearch (tenancy, visibility, every field kind, paging, errors, guests) |
| Web: MQL search | Basic / MQL switch on `/search` (basic filters carried over as a query), query editor with suggestions for fields, operators, values (project keys, statuses, people from the directory, dates) and connectives; Tab completes, Enter runs; invalid queries show the message with the faulty part underlined; syntax help with runnable examples; query kept in the URL | 5 web tests; driven in a real browser against the demo workspace |
| Audit log | user-service consumer turns admin-relevant domain events (members, invitations, projects, project access, releases, workflows, boards, sprints, deleted issues and comments) plus validated `audit.recorded` events into entries; one SHA-256 hash chain per tenant (serialized appends, de-duplicated by event id); append-only by trigger and revoked grants; forced RLS; admin/owner-only list with category, person and date filters and cursor paging, CSV export (formula-safe cells), on-demand chain verification that names the first broken entry; project-service emits `audit.recorded` for access changes | 3 unit + 1 integration test (redelivery, forged payloads rejected, filters, tenant isolation, 403 for members, export, runtime role cannot UPDATE, an owner's edit with the trigger disabled is detected); 1 project-service test asserts the access audit events; live: the dev consumer backfilled history from Kafka |
| Web: audit log | Settings → Audit log for admins and owners: entries as sentences ("gave Maya Chen Viewer access to PAY"), expandable details, activity / person / date filters kept in the URL, older entries on demand, CSV export, "Check integrity" banner | 4 web tests; checked in a real browser (17 entries intact) |
| Two-step verification (TOTP) | Authenticator-app codes (RFC 6238: SHA-1, 6 digits, 30 s, one step of drift) and ten single-use recovery codes; set-up and changes need the password again; turning it on or off ends the account's other sessions; after a correct password, sign-in returns `202` with a single-use 5-minute challenge instead of tokens; 5 wrong codes end the challenge and every wrong code counts towards the account lockout; a used time step is never accepted again; secrets sealed with AES-256-GCM bound to the account id; recovery codes stored as SHA-256; QR code rendered server-side as SVG | 5 unit tests (RFC 6238 and RFC 4648 vectors, replay, drift, recovery code typing) + 1 crypto test + 1 integration test (set-up, sign-in, replay, recovery codes, challenge exhaustion, lockout, new codes, turning off) |
| Web: two-step verification | Settings → Security (was Sessions): set up with a QR code or typed key, recovery codes to copy or download, new codes, turn off; sign-in asks for the app code or a recovery code after the password and goes back to the password when the attempt expires | 4 web tests; set up, signed in with a code and turned off with a recovery code in a real browser |
| analytics-service | Kafka consumer (`analytics-service` group on the issue, agile and project topics) turns events into ClickHouse facts: `issue_events` (ReplacingMergeTree keyed by event id, so redelivery and replay are idempotent) and `issue_state` (current issue state fetched with `GetDocuments`, versioned by issue version; deletions win); `issue.transitioned` now carries status categories. Four chart kinds compiled from a closed enum into fixed SQL with typed parameters: created vs resolved per UTC day, weekly cycle time (median and 85th percentile, first start to resolution), open-issue breakdown by status category, status, type, priority, assignee or project, and sprint velocity. Every query runs as a read-only ClickHouse user under a row policy on `SQL_tenant_id` (sent from the verified token) and is limited to the projects the caller can browse. Dashboards and widgets in PostgreSQL with forced RLS: private until shared, edited by the owner (or an admin when shared), only the owner shares; 200 dashboards per owner, 30 charts per dashboard | 2 ClickHouse + 3 widget unit tests + 2 integration tests against real ClickHouse and PostgreSQL (tenant and project isolation, replay de-duplication, cycle time, issue and project deletion, velocity order, 422 on bad options, the reader refused without the tenant setting, across tenants and for writes; dashboard privacy, sharing and edit rights); live: the dev consumer backfilled the demo workspace from Kafka |
| Web: dashboards | Dashboards in the top navigation (`g d`): list with owner, sharing and chart count; create, rename, share / stop sharing, delete with confirmation; charts in a half/full-width grid with edit, resize, reorder and remove for editors and read-only for viewers; add/edit dialog with a live preview; every chart has a table view, hover tooltips, one value axis, legends for two series and values in ink colours; chart chrome shared with the sprint reports | 6 web tests; driven in a real browser against the demo workspace in both themes (counts checked against ClickHouse) |
| Web: live updates | Fetch-based SSE client with token refresh, jittered reconnect and cursor resume; project screens refetch only what a hint affects, batched and deferred while the person's own change is in flight; "Live updates paused" banner | 7 web tests; live in a real browser |
| Web: notifications and search | Bell with live unread badge, unread filter, mark all read, open-and-mark-read, link to settings; Settings → Notifications with email switches saved instantly; ⌘K palette searches issues as you type; `/search` page with project, status and "assigned to me" filters, highlighted matches, paging | 7 web tests; driven in a real browser in light and dark themes |
| Developer workflow | `make dev/stop/test/lint/migrate/seed/logs/clean/gen-api`, `.env.example`, seed script (idempotent, via public API) | each command run |

Totals: **206 Rust tests, 94 web tests, all passing**; `cargo clippy -D warnings`, `rustfmt --check`, `eslint`, `tsc` clean.

## Incomplete within Phase 5 scope

| Item | State |
|---|---|
| Email | One email per notification (no digests); comment emails do not quote the comment; no bounce or complaint handling |
| Notification preferences | Email only (in-app is always on), per workspace. No notifications for sprint, project or workflow events |
| Boards | Still read cards from issue-service on each load rather than an event-fed copy |
| Search | No custom fields, components, versions, sprints or parents in the index (so not in MQL either), no search for projects or people; `reindex` takes one tenant id; no index alias for mapping changes; the shared `search_db` database is unused |
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
| Deleted issues | Fixed in Phase 5: the agile cleanup consumer removes placements and records scope removal from running sprints |
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
| WebAuthn, OIDC, SAML, SCIM, API tokens | Not built (Phase 6); TOTP two-step verification is built |
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

## Incomplete within Phase 7 scope

| Item | State |
|---|---|
| Chart kinds | Four built. Not built: cumulative flow, control chart, time in status, burnup, recently-created-by-type and other gadgets; filtering charts by MQL; per-dashboard date pickers |
| Accuracy | Created vs resolved and breakdowns use each issue's current state, so a reopened issue loses its earlier resolution date; days and weeks are UTC; cycle time starts at the first move into an in-progress status recorded since analytics was deployed (older transitions without categories are filled from the current state only when the issue is still in that status) |
| Dashboards | No `If-Match` concurrency control (last write wins); no sharing with specific people, groups or projects; no copying a dashboard; no auto-refresh beyond refetch on visit |
| Pipeline | Backfill needs the topics' retention or a consumer offset reset (no rebuild-from-source command like search's `reindex`); no materialized views or rollups yet (queries scan the facts, fine at demo scale, untested at volume); ClickHouse users are created by the dev init script, so production needs them provisioned the same way |

## Incomplete within Phase 6 scope

| Item | State |
|---|---|
| Query language | Built (see above). Not built: saved filters, filter subscriptions, sharing; `membersOf()`, `openSprints()`, `issueHistory()` and other functions; `WAS` / `CHANGED`; custom fields; time zones other than UTC; the board and backlog do not accept MQL yet |
| Two-step verification | TOTP built (see above). Not built: WebAuthn / passkeys; a workspace setting that requires it of members; an email when it is turned on or off or a recovery code is used; an `amr` claim in tokens; admin reset for a member who lost their phone and codes (today only a database operator can) |
| SSO (OIDC, SAML), SCIM, API tokens | Not started |
| Audit log | Built (see above). Not built: client IP address on entries; retention and purge (append-only for now, so entries are kept forever); workflow and board entries have no name (their events carry none); entries are listed in the order they were recorded, which can differ slightly from when they happened across topics; project access entries are written just after the permission change rather than in the same transaction, so a crash between the two loses the entry; no scheduled verifier; no audit of sign-ins (identity-service emits no events yet) |
| Admin console | Not started (settings pages exist for members, invitations, notifications and the audit log) |
| Automation, integrations (webhooks, Git, Slack) | Not started |

## Measured performance

No load tests have been run. The targets in the brief (1M users, 50k concurrent, p95 200/300 ms, 99.95 %)
are **design targets only** until k6 results are recorded here.

## Next

1. Phase 8: Kubernetes, Helm, Terraform, CI/CD, the observability stack, load and failure testing.
2. Phase 7, remaining: more chart kinds (cumulative flow first), a rebuild command for the analytics store, `If-Match` on dashboards.
3. Phase 6, remaining: admin console, WebAuthn and requiring two-step verification per workspace, API tokens, SSO (OIDC, SAML) and
   SCIM, saved filters on top of MQL, declarative automation, integrations.
