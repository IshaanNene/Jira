# Implementation roadmap and status

This file is the honest ledger. A feature is **done** only when it works end to end (UI → API → auth → tenant →
authorization → database → outbox) and has tests. "Specified" means there is design but no code.

Legend: ✅ done and verified · 🚧 partly done · ⬜ not started

Last updated: 2026-11-14 (Phase 8: container images, Helm chart, Terraform, CI, observability stack, load and failure tests).

## Phase status

| Phase | Scope | Status |
|---|---|---|
| 0 | Architecture, contracts, ADRs | ✅ |
| 1 | Monorepo, shared crates, identity, tenant registry, user/org, web shell, compose, gateway | ✅ (gaps listed below) |
| 2 | Project + issue engine, **permission-service core** (moved up from Phase 6) | ✅ (smaller gaps listed below) |
| 3 | Workflow engine + visual designer | ✅ (gaps listed below) |
| 4 | Boards, backlog, sprints, reports | ✅ (gaps listed below) |
| 5 | Kafka relay, consumers, search indexing, realtime gateway, notifications | ✅ (gaps listed below) |
| 6 | SSO/SCIM/MFA, admin, audit, QL search, automation, integrations | 🚧 (query language with saved filters, audit log, authenticator-app two-step verification (optionally required per workspace) and API tokens done; the rest not started) |
| 7 | ClickHouse pipeline, dashboards | ✅ (seven chart kinds; gaps listed below) |
| 8 | Kubernetes/Helm/Terraform/CI/CD, observability stack, load + failure testing | 🚧 (chart, images, CI, observability and tests built and run locally; Terraform validated but never applied; no Argo CD; gaps listed below) |

Deviations from the brief's phase order, and why, are in [ARCHITECTURE.md](ARCHITECTURE.md) §7.

Against the brief's 45 final deliverables: roughly the foundations of items 1–6, 8, 9 (revocation list only),
15–16 (password auth and org-level RBAC only), 26–27, 34 (Prometheus, Grafana, Jaeger, alert rules), 36–37, 41–43, 45
exist, plus items 12 (attachment storage), 17–18 (issue and project management), 20 (workflow designer and
engine), plus the agile item (boards, backlog, sprints and sprint reports), and the event pipeline, realtime updates,
in-app and email notifications, full-text issue search, dashboards over a ClickHouse pipeline, and the deployment
items (container images, a Helm chart, Terraform for AWS, CI) with load and failure tests.
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
| Boards and backlog (web) | Board with drag between columns (the move goes through the workflow; refusals put the card back with the engine's reason), search, "Only my issues", an MQL filter (the editor from `/search`, kept in the URL as `?mql=`; ranking by drag pauses in the backlog while a filter hides issues), sprint header with time left; backlog with drag to rank and between sprints, row menus, create / edit / start / complete / delete sprint dialogs; board settings with a column editor; Reports with burndown and velocity charts (ECharts, light and dark) and issue tables | 15 web tests; driven in a real browser: plan a sprint, start it, drag a card To Do → In Progress (persisted with history), complete it, read the frozen report and velocity in both themes |
| Internal gRPC | identity → user (`CheckMembership`), user → tenant (`ProvisionTenant`, `ResolveTenant`, `AbandonTenant`), authenticated, with deadlines, bounded retry, breaker | exercised live across three processes |
| Outbox | Every write enqueues its event in the same transaction | asserted in integration tests |
| Gateway | Envoy: routing, request ids, rate limits, edge EdDSA JWT validation, identity headers stripped | manual curl checks (401 without/with bad token, 200 with valid) |
| Web: projects & issues | Top navigation with Projects menu and Create; Your work; project list, create, settings (details, access, components, releases); issue list with URL filters; issue view with inline editing, conflict handling, description editor, child issues, links, comments, history, watch; `c` shortcut | driven in a real browser: create project → create issue → change status → comment → history |
| Web app | Design system (light/dark), login, register, onboarding / workspace chooser, app shell (sidebar, workspace switcher, breadcrumbs, command palette, shortcuts), home, people + invitations, invitation acceptance, workspace settings with conflict handling, profile, sessions | 17 unit tests; register → create workspace → invite driven in a real browser against the running stack |
| Event pipeline | Transactional outbox → Kafka relay in every service (advisory-locked, one relay per database, ordered per aggregate, failures hold back later events of the same key), consumers with manual commits, retry with backoff until a transient failure clears (permanent failures go to a dead-letter topic), tenant required on every envelope, inbox de-duplication, retention sweep | 2 integration tests against real Kafka + PostgreSQL; live: every service's outbox drained to 0, including events held since Phase 1 |
| realtime-service | SSE change hints per project, per person and per workspace; Redis Streams buffer with cursor resume and `resync` when a gap is detected; access and session revocation re-checked every 60 s; token expiry ends the stream; connection cap | 4 integration tests against real Redis; live: an edit by another user appeared on an open backlog within 2 s |
| notification-service | Notifications for assignment, mention, comment and status change to reporter, assignee and watchers, only for active members who can still browse the project; never to the actor; idempotent on redelivery; list / unread count / mark read / mark all; hidden when access is lost | 3 unit + 2 integration tests incl. cross-tenant and cross-user isolation |
| Email notifications | Queued with each notification per the person's preferences, sent after a short delay unless read in the app first; leased sender safe across instances; retry with backoff, permanent failures recorded; escaped content, no header injection; per-workspace preferences (master switch + per kind) | 3 unit + 2 integration tests sending through a real Mailpit; live: an assignment by Maya reached Olivia's Mailpit inbox and rendered correctly |
| One-click unsubscribe and board cleanup | Signed, expiring, off-only unsubscribe links in every email (footer page + RFC 8058 headers), public through the gateway for that one path; agile consumer removes placements of deleted issues (scope removal logged for running sprints) and retires boards of deleted projects | 2 unit + 1 integration test (headers and link read from a real Mailpit message), 1 agile integration test, 2 web tests; live: unsubscribed from a real email's link in the browser |
| search-service | OpenSearch index fed from issue and project events (fresh document per event, external versioning, deletes), tenant-filtered and routed, limited to visible projects; key, prefix and typo-tolerant matching over summary, description, comments and labels; structured highlights (no HTML); cursor paging; `reindex <tenant>` that also drops vanished issues | 3 unit + 2 integration tests against real OpenSearch; live: reindexed the demo workspace, an edit was searchable within 3 s through Kafka |
| New internal RPCs | `issue.v1.Issues/GetAudience`, `GetDocuments` (≤ 500), `ListIssueIds` (≤ 1000); `user.v1.Membership/GetMembers` (≤ 500), `FindMembers` (≤ 100 terms) | `audiences_and_search_documents_are_served_to_other_services`, `grpc_membership_check_reflects_role_and_status` |
| Issue query language (MQL) | Fields, operators, AND / OR / NOT, parentheses, lists, EMPTY, people by `currentUser()` / email / exact name, relative and absolute dates, text match with prefixes, ORDER BY with keys sorted by number and priorities by rank; compiled server-side inside the tenant and visibility filters; errors with a character span; size and depth limits; guests limited to `currentUser()`; an ids-only form for one project (at most 5000) lets the board and backlog filter their cards | 8 unit + 1 integration test against real OpenSearch (tenancy, visibility, every field kind, paging, errors, guests; the ids-only answer for one project) |
| Web: MQL search | Basic / MQL switch on `/search` (basic filters carried over as a query), query editor with suggestions for fields, operators, values (project keys, statuses, people from the directory, dates) and connectives; Tab completes, Enter runs; invalid queries show the message with the faulty part underlined; syntax help with runnable examples; query kept in the URL | 5 web tests; driven in a real browser against the demo workspace |
| Saved filters | search-service now has a PostgreSQL database (RLS by tenant) for saved MQL filters: name, query (parsed on every write, the faulty span returned), private or shared with the workspace; the owner edits, an admin can edit or delete a shared one but not unshare it; names unique per person ignoring case; at most 100 per person; a filter is run with the viewer's own access | 2 integration tests (visibility, edit rights, validation, uniqueness, tenant isolation, row-level security, limit) |
| Filter subscriptions | Subscribe to a saved filter you can see (own or shared) by email, daily or weekly at a UTC hour; at most 20 per person. search-service schedules runs in a non-RLS `subscription_schedule` queue leased with `FOR UPDATE SKIP LOCKED`, runs the query as the subscriber (their current role and project access; a subscriber who left or a filter that was unshared ends the subscription) and emits `filter.digest` on the new `platform.search.v1` topic through an outbox. notification-service emails the matching issues (up to 20 listed, the total, escaped) with a signed unsubscribe link and a new "Saved filter subscriptions" email preference. Web: "Email subscription…" on a filter's menu, a toggle in notification settings | 2 unit + 1 integration test in search (API, runner, access loss, rescheduling, cascade); 1 unit + 1 integration test in notification (Mailpit message, preference, inactive and stale skipped, unsubscribe); 2 web tests |
| Email verification and password reset | identity-service emails single-use links (SHA-256 stored, 24 h / 1 h, bound to the address, spent together, 1 per minute and 5 per hour per account) over SMTP through a new shared `smtp-mailer` crate, sent in the background so nothing about an account leaks through answers or timing; a reset sets the password, clears the lockout, verifies the address and ends every session. Access tokens carry `email_verified`; user-service refuses invitations (`403 email_unverified`) without it, closing the "register the invited address first" hole. Web: verify, forgot and reset pages, a reminder banner with resend, the invitation page explains and retries after verifying. The seed script verifies invitees through Mailpit | 1 unit + 3 integration tests in identity (links, rate limit, spent and foreign links, address change, reset ending sessions, no mail server), user-service invitation test extended; 7 web tests; live against Mailpit: registered, verified with the seed's helper, reset email read (logs hold no address or token) |
| Invitation email | user-service emails each new invitation (inviter, workspace, role, link, expiry; escaped) over the shared `smtp-mailer`; `POST .../invitations/{id}/resend` issues a new token and 7 more days for an open or expired invitation and emails it (old link dead; once a minute, `last_sent_at`); the token is only in the email and the inviter's response. Web: "Invitation sent" when emailed, Resend / Renew on pending invitations with "emailed …" times, and the new link shown when no mail server is set up | 1 unit + 1 integration test (email content, rate limit, member refused, renewal of an expired invitation, old link dead, accepted cannot be resent); 2 web tests |
| Web: saved filters | `/search` in MQL mode: Filters menu (mine, then shared), open, Save filter, Save changes when edited, save as new, rename, share / stop sharing, delete with confirmation; `?filter=<id>` links run the filter | 1 web test |
| Audit log | user-service consumer turns admin-relevant domain events (members, invitations, projects, project access, releases, workflows, boards, sprints, deleted issues and comments) plus validated `audit.recorded` events into entries; one SHA-256 hash chain per tenant (serialized appends, de-duplicated by event id); append-only by trigger and revoked grants; forced RLS; admin/owner-only list with category, person and date filters and cursor paging, CSV export (formula-safe cells), on-demand chain verification that names the first broken entry; project-service emits `audit.recorded` for access changes | 3 unit + 1 integration test (redelivery, forged payloads rejected, filters, tenant isolation, 403 for members, export, runtime role cannot UPDATE, an owner's edit with the trigger disabled is detected); 1 project-service test asserts the access audit events; live: the dev consumer backfilled history from Kafka |
| Web: audit log | Settings → Audit log for admins and owners: entries as sentences ("gave Maya Chen Viewer access to PAY"), expandable details, activity / person / date filters kept in the URL, older entries on demand, CSV export, "Check integrity" banner | 4 web tests; checked in a real browser (17 entries intact) |
| Two-step verification (TOTP) | Authenticator-app codes (RFC 6238: SHA-1, 6 digits, 30 s, one step of drift) and ten single-use recovery codes; set-up and changes need the password again; turning it on or off ends the account's other sessions; after a correct password, sign-in returns `202` with a single-use 5-minute challenge instead of tokens; 5 wrong codes end the challenge and every wrong code counts towards the account lockout; a used time step is never accepted again; secrets sealed with AES-256-GCM bound to the account id; recovery codes stored as SHA-256; QR code rendered server-side as SVG | 5 unit tests (RFC 6238 and RFC 4648 vectors, replay, drift, recovery code typing) + 1 crypto test + 1 integration test (set-up, sign-in, replay, recovery codes, challenge exhaustion, lockout, new codes, turning off) |
| Web: two-step verification | Settings → Security (was Sessions): set up with a QR code or typed key, recovery codes to copy or download, new codes, turn off; sign-in asks for the app code or a recovery code after the password and goes back to the password when the attempt expires | 4 web tests; set up, signed in with a code and turned off with a recovery code in a real browser |
| Workspace two-step requirement | Workspace settings → Two-step verification (admins and owners, `If-Match`, audited as `organization.two_step_required` / `_optional`); the membership check tells identity-service, which then refuses workspace tokens (exchange, API-token exchange, creating API tokens) with `403 two_step_required` to members without it, and refresh falls back to an account token; the web sends a refused member to a set-up page and then into the workspace | 1 identity + 1 user-service integration test; 3 web tests |
| API tokens | Personal tokens for scripts and integrations, bound to one person in one organization: `mrd_` + 256 random bits, only the SHA-256 kept, a hint shown, 1–365 days, at most 25 active; exchanged at `POST /api/v1/auth/token/api` (credential-route rate limit) for an ordinary 10-minute org token marked `api`, with membership and account status read afresh at every exchange; such tokens cannot list or make API tokens, see sessions, change two-step verification or switch organization; revoking one revokes its access tokens through the revocation list | 1 integration test (create, list, exchange, refusals, expiry, removed member, other person's revoke, limit) |
| Web: API tokens | Settings → Security: create with a name and lifetime, the secret shown once with copy, list with hint, last use and expiry, revoke with confirmation; scoped to the open organization | 1 web test |
| analytics-service | Kafka consumer (`analytics-service` group on the issue, agile and project topics) turns events into ClickHouse facts: `issue_events` (ReplacingMergeTree keyed by event id, so redelivery and replay are idempotent) and `issue_state` (current issue state fetched with `GetDocuments`, versioned by issue version; deletions win); `issue.transitioned` now carries status categories. Seven chart kinds compiled from a closed enum into fixed SQL with typed parameters: created vs resolved per UTC day, weekly cycle time (median and 85th percentile, first start to resolution), open-issue breakdown by status category, status, type, priority, assignee or project, sprint velocity, and cumulative flow (issues per status category at the end of each UTC day, worked back from the current state through recorded moves, so today always matches the board), and time in status (median and mean days per status for stays that ended in the period, each stay timed from the previous move or creation), and a control chart (every issue resolved in the period with its cycle time, a rolling average of ten issues and the mean; the latest 2000). Every query runs as a read-only ClickHouse user under a row policy on `SQL_tenant_id` (sent from the verified token) and is limited to the projects the caller can browse. Dashboards and widgets in PostgreSQL with forced RLS: private until shared, edited by the owner (or an admin when shared), only the owner shares; 200 dashboards per owner, 30 charts per dashboard. `analytics-service rebuild <tenant-id>` restores one tenant's `issue_state` from issue-service (pages through `ListIssueIds`, fetches documents, marks missing issues deleted) without waiting for topic retention. Chart answers are cached in Redis for up to 5 minutes, keyed by tenant, a digest of the query and the viewer's visible projects, the UTC day and a per-tenant generation the ingester bumps after every write (so new events show at once); without Redis, charts are computed | 2 ClickHouse + 5 widget + 1 cache-key unit tests + 4 integration tests against real ClickHouse and PostgreSQL (tenant and project isolation, replay de-duplication, cycle time, cumulative flow day by day and for a member who browses one project, time in status per stay, the control chart's points and mean, issue and project deletion, velocity order, 422 on bad options, the reader refused without the tenant setting, across tenants and for writes; dashboard privacy, sharing and edit rights; a rebuild repairs changed, deleted and unseen issues for one tenant only and is idempotent; a cached answer is reused, kept apart for a viewer with other projects, and dropped once an event is ingested); live: the dev consumer backfilled the demo workspace from Kafka |
| Web: dashboards | Dashboards in the top navigation (`g d`): list with owner, sharing and chart count; create, rename, share / stop sharing, delete with confirmation; charts in a half/full-width grid with edit, resize, reorder and remove for editors and read-only for viewers; add/edit dialog with a live preview; every chart has a table view, hover tooltips, one value axis, legends for two series and values in ink colours; cumulative flow as stacked bands in one hue, stronger towards done, with done at the bottom; time in status as sideways bars of median days; the control chart as dots on a time axis with the rolling average and a dashed mean, issue keys escaped in tooltips; chart chrome shared with the sprint reports | 9 web tests; driven in a real browser against the demo workspace in both themes (counts checked against ClickHouse) |
| Dashboard concurrency | Dashboards carry a version that every change to them or their charts moves on; renames, sharing, reordering and adding, editing or removing a chart need `If-Match` (428 without, 409 with the current dashboard when stale); responses carry the new version as `ETag`. The web sends the version on screen and, on a conflict, shows the latest dashboard instead of overwriting it. Deleting a whole dashboard does not need `If-Match` | analytics integration test extended (missing, stale and current versions; chart changes move it); 2 web tests |
| Web: live updates | Fetch-based SSE client with token refresh, jittered reconnect and cursor resume; project screens refetch only what a hint affects, batched and deferred while the person's own change is in flight; "Live updates paused" banner | 7 web tests; live in a real browser |
| Web: notifications and search | Bell with live unread badge, unread filter, mark all read, open-and-mark-read, link to settings; Settings → Notifications with email switches saved instantly; ⌘K palette searches issues as you type; `/search` page with project, status and "assigned to me" filters, highlighted matches, paging | 7 web tests; driven in a real browser in light and dark themes |
| Container images | One shared release build for all twelve services on distroless without OpenSSL (non-root, no shell; zlib and Kafka's OpenSSL linked statically); web on unprivileged nginx 1.30; base images always pulled fresh; every service image is started once during the build | `scripts/images/build.sh`; all 13 images passed Trivy (no fixable high/critical) on 2026-11-14. The scan and the start check found and fixed: images that could not start (`libz.so.1` missing), stale bases with fixable CVEs, and the web build copying the host's `node_modules` |
| Helm chart | Deployment, Service, ServiceAccount, PDB and HPA per service; migration Jobs as pre-install/upgrade hooks with owner-role secrets only; Envoy gateway generated from a route list (admin API on loopback, read-only ops port, HSTS on https); default-deny NetworkPolicy with gateway/service/JWKS/egress rules; restricted pod security on every pod; topology spread | `helm lint` for both value sets; generated Envoy config passes `envoy --mode validate`; installed on a three-node kind cluster: migrations, all 14 pods, sign-in and API through the gateway (also in a browser), ingress rules probed from test pods |
| Terraform (AWS) | Modules for VPC, EKS, RDS PostgreSQL, MSK (SCRAM), ElastiCache (TLS), OpenSearch and S3, a staging environment with S3 state locking | `terraform fmt` and `validate` (1.10.5); never applied |
| CI | GitHub Actions: Rust fmt, clippy, tests against the Compose data stores; RustSec advisories; web lint, typecheck, tests, build; chart lint + Envoy validation; Terraform validate; image build, Trivy scan, push to GHCR on `main` | actionlint; all jobs green on GitHub (2026-11-14), 13 images pushed to GHCR |
| Kafka over TLS/SASL | `KAFKA_SECURITY_PROTOCOL` (plaintext, ssl, sasl_ssl), SCRAM or PLAIN, CA file; incomplete settings refused at start-up; password never in `Debug` output | 1 unit test |
| Batch consumer | `run_batch_consumer`: up to N events or a time limit per batch, the whole batch retried on a transient failure, only the rejected events dead-lettered, one offset commit per batch; analytics-service writes 500 events / 250 ms as one `INSERT` per table, fetching issue documents in one RPC per tenant | 1 unit test; 1 analytics integration test (204 events, one active part per table, one rejected event, redelivery idempotent) |
| Consumer retries | A transient failure is retried with backoff (200 ms → 30 s) until it succeeds; only permanent failures are dead-lettered; `events_retried_total` | 1 unit test; `tests/resilience/run.sh opensearch` fails on the old code (an issue changed during the outage never reached search) and passes on the new |
| Observability stack | Prometheus, Grafana (provisioned overview dashboard), Jaeger in the `observability` compose profile; latency exported as histograms; 7 alert rules (service down, error rate, slow requests, consumer waiting, dead letters, outbox, email) | promtool; all 13 targets scraped; p95 by service queried; `ConsumerWaitingOnDependency` went pending with OpenSearch stopped; search-service traces reached Jaeger |
| Failure tests | `make resilience` stops Kafka, OpenSearch, ClickHouse, Redis and PostgreSQL in turn under the running stack and checks the documented behaviour and the recovery | 21 checks, all passing (results in SYSTEM_DESIGN.md §8) |
| Load test | `make load`: k6 open-model scenarios (reads across six endpoints, writes creating issues and comments) with p95 and error thresholds; `tests/load/stress.mjs`: autocannon stages at rising rates to find saturation | see Measured performance |
| Developer workflow | `make dev/stop/test/lint/migrate/seed/logs/clean/gen-api/observability/images/kind-up/kind-down/load/resilience`, `.env.example`, seed script (idempotent, via public API) | each command run |

Totals: **231 Rust tests, 115 web tests, all passing**; `cargo clippy -D warnings`, `rustfmt --check`, `eslint`, `tsc` clean.

## Incomplete within Phase 5 scope

| Item | State |
|---|---|
| Email | One email per notification (no digests); comment emails do not quote the comment; no bounce or complaint handling |
| Notification preferences | Email only (in-app is always on), per workspace. No notifications for sprint, project or workflow events |
| Boards | Still read cards from issue-service on each load rather than an event-fed copy |
| Search | No custom fields, components, versions, sprints or parents in the index (so not in MQL either), no search for projects or people; `reindex` takes one tenant id; no index alias for mapping changes; `search_db` holds only saved filters |
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
| Reports | Burndown uses each issue's current estimate, not the estimate at the time; cumulative flow is a dashboard chart, not a board report, as are time in status and the control chart; no sprint retrospective or epic/version reports. The guideline starts from scope at start even when part of it was already done |
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
| Email verification, password reset, invitation email | Built (see above); accepting an invitation needs a verified address. Not built: changing an account's address; bounce handling |
| Teams, departments, groups, avatars, org logo/branding, domains | Not built |
| Tenant tier migration; business/enterprise provisioning | Schema and placement decision only; dedicated tiers are recorded as `pending` and are not routable |
| WebAuthn, OIDC, SAML, SCIM | Not built (Phase 6); TOTP two-step verification and API tokens are built |
| Per-IP rate limiting | Dev gateway has global and credential-route buckets only; per-IP needs the rate-limit service planned with Envoy Gateway |
| OpenTelemetry export | Verified in Phase 8: search-service traces reached Jaeger over OTLP/HTTP. Spans cover HTTP requests only; no trace propagation through Kafka |
| Playwright end-to-end tests, Storybook | Not set up |
| Contract tests | Event wire-format fixture only; no gRPC/REST compatibility check in CI |

## Incomplete within Phase 8 scope

| Item | State |
|---|---|
| Cloud deployment | Terraform has never been applied; nothing runs in AWS. No ExternalSecret manifests, no cert-manager, no DNS |
| CD | No Argo CD, no Argo Rollouts canaries; the chart is installed with `helm upgrade` |
| CI | No image signing or SBOM; no Playwright or contract tests in it |
| NetworkPolicy egress | Rendered and correct on inspection, but **not verified**: kind's network plugin does not enforce egress, so a cluster with Calico, Cilium or the EKS VPC CNI policy agent is needed |
| Autoscaling | CPU-based HPA only; no KEDA scaling on Kafka lag, no connection-based scaling for realtime; HPAs were not exercised (kind runs one replica) |
| Observability | No log aggregation (Loki or similar), no Alertmanager routing, no SLO burn-rate alerts, no consumer-lag exporter; traces cover HTTP requests only, not Kafka hops; Grafana has one dashboard |
| Load testing | Saturation found on one laptop only (reads ~2,000/s, writes ~1,800/s, with one replica per service sharing the CPU with every data store); no soak, spike, realtime fan-out or multi-replica tests; the brief's scale targets remain untested |
| Failure testing | Data stores only, each stopped cleanly; no network partitions, slow disks, pod kills under load, or database fail-over |
| Backups | No restore drill; RPO/RTO remain targets |

## Known issues

- Crate versions are pinned one major behind current for several dependencies (sqlx 0.8, tonic 0.13, jsonwebtoken 9,
  utoipa 5, tower-http 0.6) to stay on APIs that were verified here. Upgrading is a tracked chore. `redis` (0.27 → 1)
  and `object_store` (0.11 → 0.14) were upgraded in Phase 8 to clear RustSec advisories (quick-xml denial of
  service, unmaintained rustls-pemfile).
- A checkout inside an iCloud-synced folder (such as the Desktop) breaks local development: iCloud evicts files
  such as service logs and test caches, and anything that opens them blocks. Keep the repository outside iCloud.
- Chart queries are the slowest read path (666/s on one replica, p99 658 ms at saturation): every request queries
  ClickHouse directly, with no cache.
- When a disk passes 95 %, OpenSearch makes its indices read-only. Search then answers queries but the search
  consumer stops on `429` and waits (by design, without dropping events) until space is freed. There is no alert for
  OpenSearch disk use yet.
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
| Chart kinds | Seven built; time in status counts only stays that ended (an issue still in a status is not in it); cumulative flow counts only moves recorded with both categories (older moves are left out, so such an issue sits in its current category from creation). Not built: burnup, recently-created-by-type and other gadgets; filtering charts by MQL; per-dashboard date pickers |
| Accuracy | Created vs resolved and breakdowns use each issue's current state, so a reopened issue loses its earlier resolution date; days and weeks are UTC; cycle time starts at the first move into an in-progress status recorded since analytics was deployed (older transitions without categories are filled from the current state only when the issue is still in that status) |
| Dashboards | `If-Match` built (see above). No sharing with specific people, groups or projects; no copying a dashboard; no auto-refresh beyond refetch on visit |
| Pipeline | `rebuild` restores current state only: the event history in `issue_events` (created vs resolved, cycle time) still needs the topics' retention or an offset reset; no materialized views or rollups yet (queries scan the facts, fine at demo scale, untested at volume; the answer cache helps repeated views, not the first one, and has not been re-measured under load); ClickHouse users are created by the dev init script, so production needs them provisioned the same way |

## Incomplete within Phase 6 scope

| Item | State |
|---|---|
| Query language | Built, with saved filters and email subscriptions (see above). Not built: subscriptions in a person's own time zone or for other people and groups, sharing with specific people or groups, favourites, a filter column picker; `membersOf()`, `openSprints()`, `issueHistory()` and other functions; `WAS` / `CHANGED`; custom fields; time zones other than UTC; the board and backlog filter by MQL but quick filters cannot be saved on a board |
| Two-step verification | TOTP built (see above). Requiring it per workspace built (see above): access tokens issued before the requirement stay valid for up to 10 minutes, and a member can still turn their own off (they are then refused at the next refresh). Not built: WebAuthn / passkeys; an email when it is turned on or off or a recovery code is used; an `amr` claim in tokens; admin reset for a member who lost their phone and codes (today only a database operator can) |
| API tokens | Built (see above). Not built: scopes narrower than the person's own access; tokens owned by a service account rather than a person; an admin view of a workspace's tokens; expiry reminders; secret scanning for the `mrd_` prefix |
| SSO (OIDC, SAML), SCIM | Not started |
| Audit log | Built (see above). Not built: client IP address on entries; retention and purge (append-only for now, so entries are kept forever); workflow and board entries have no name (their events carry none); entries are listed in the order they were recorded, which can differ slightly from when they happened across topics; project access entries are written just after the permission change rather than in the same transaction, so a crash between the two loses the entry; no scheduled verifier; no audit of sign-ins (identity-service emits no events yet) |
| Admin console | Not started (settings pages exist for members, invitations, notifications and the audit log) |
| Automation, integrations (webhooks, Git, Slack) | Not started |

## Measured performance

### Stress test: release builds on Kubernetes (autocannon)

`tests/load/stress.mjs` on 2026-11-14, against the Helm chart on a three-node kind cluster on one laptop
(Apple M4 Pro, 14 cores, Docker Desktop), through the Envoy gateway. **Release builds**, one replica per service,
no CPU limits, `DATABASE_MAX_CONNECTIONS=5` per service, data stores from Compose. The gateway's rate limit
(200 requests/s per pod) was raised for the run and restored afterwards. Each stage held its target rate for
30 s (writes 20 s); latency is measured by autocannon per request.

**With the production rate limit**, 400 reads/s offered: 211/s served, the rest refused with `429` and
`retry-after: 1` (p99 49 ms). The limit holds.

| Reads (same mix as k6) | Served | p50 | p90 | p99 | Max | Failures |
|---|---|---|---|---|---|---|
| 250/s | 250/s | 5 ms | 22 ms | 85 ms | 114 ms | 0 |
| 500/s | 500/s | 4 ms | 22 ms | 37 ms | 81 ms | 0 |
| 1,000/s | 1,000/s | 7 ms | 35 ms | 70 ms | 150 ms | 0 |
| 2,000/s | 1,948/s | 16 ms | 109 ms | 249 ms | 433 ms | 8 timeouts (> 10 s) |
| Uncapped, 200 connections | **2,959/s** | 58 ms | 101 ms | 178 ms | 524 ms | 83 timeouts, 13 × `503` (of 88,746) |

| Writes (create issue, add comment) | Served | p50 | p90 | p99 | Max | Failures |
|---|---|---|---|---|---|---|
| 50/s | 50/s | 7 ms | 19 ms | 34 ms | 48 ms | 0 |
| 100/s | 100/s | 7 ms | 21 ms | 47 ms | 80 ms | 0 |
| 200/s | 200/s | 6 ms | 13 ms | 24 ms | 48 ms | 0 |
| Uncapped, 200 connections | **1,799/s** | 102 ms | 143 ms | 310 ms | 469 ms | 0 (35,967 × `201`) |

Each read endpoint alone, uncapped (15 s, 200 connections), shows where the mix saturates:

| Endpoint | Backed by | Peak | p99 | Timeouts |
|---|---|---|---|---|
| Unread count | PostgreSQL | 6,961/s | 45 ms | 0 |
| View issue | PostgreSQL | 5,707/s | 66 ms | 0 |
| List issues | PostgreSQL | 4,398/s | 85 ms | 0 |
| Board view | agile → issue (gRPC) | 2,460/s | 142 ms | 0 |
| Search | OpenSearch | 1,905/s | 71 ms | 176 |
| Chart query | ClickHouse | **666/s** | 658 ms | 150, plus 6 × `503` |

**Event pipeline after the uncapped write burst** (about 40,000 events): every outbox drained to Kafka
straight away; analytics caught up in about 40 s; search caught up at about 290 events/s once its index was
writable again (see Known issues: the Docker disk filled during the run, OpenSearch made its index read-only,
and the search consumer waited, as designed, without losing an event).

What this does and does not show: one laptop runs the load generator, all fifteen pods, Kafka, OpenSearch,
ClickHouse and PostgreSQL, so every number is shared-CPU. The PostgreSQL-backed paths scale well; the first limits
are the ClickHouse and OpenSearch query paths, not the Rust services. The brief's targets (1M users, 50k concurrent,
99.95 %) remain **design targets only**: there is no soak test, no realtime fan-out test, and no multi-replica run.

### Baseline: debug builds on the host (k6)

One run of `make load` on 2026-11-14 against **debug builds** running as local processes, through the gateway:
reads at 100/s for 2 min (p50 5.7 ms, p95 12.6 ms, p99 46.6 ms) and writes at 10/s for 2 min (p50 7.1 ms,
p95 16.3 ms, p99 76.9 ms); 14,408 requests, 1 failed. All thresholds passed.

## Next

1. Phase 8, remaining: verify egress policies on a CNI that enforces them, a staging
   environment from the Terraform with External Secrets and Argo CD, re-running the stress test
   against chart queries now that they are cached, and a restore drill.
2. Phase 6, remaining: admin console, WebAuthn, SSO (OIDC, SAML) and SCIM, declarative
   automation, integrations.
