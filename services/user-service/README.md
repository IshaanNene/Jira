# user-service

**Responsibility.** User profiles, organizations, memberships and invitations. Starts the
organization-creation saga (provision tenant → write organization + owner membership → compensate
on failure).

**API.** REST under `/api/v1` — contract at `/openapi.json`: `users/me`, `organizations`,
`organizations/current`, `.../members`, `.../invitations`, `.../audit` (+`/export`, `/verify`; admins and owners),
`invitations/{token}` (+`/accept`).
Internal gRPC `Membership`: `CheckMembership` for identity-service, `GetMembers` (people by id: role, status,
name, email) for notification-service, and `FindMembers` (members whose email or display name equals a term,
ignoring case; no partial matches) for search-service's query language.

**Tenancy.** `organizations`, `memberships`, `invitations` are tenant-owned with forced row-level
security; all access goes through `DbRouter::begin(tenant_id)`. Three small global tables exist by
design: `user_profiles`, `user_org_directory` (which organizations a user is in) and
`invitation_tokens` (token hash → where the invitation lives).

**Rules.** Admins manage non-owners; only owners touch ownership; an organization always keeps one
active owner; the actor's role is re-read from the database on every member change; invitations are
single-use, expire after 7 days and can only be accepted by the invited address.

**Database.** Owns `user_db`. Emits `organization.created`, `organization.updated`, `user.invited`,
`membership.changed`.

**Audit log.** The `user-service.audit` consumer (started when `KAFKA_BROKERS` is set) reads the organization,
project, workflow, agile, issue and audit topics and appends admin-relevant events to `audit_log`: one SHA-256
hash chain per tenant, append-only by trigger and revoked grants, de-duplicated by event id. See
[SECURITY_DESIGN.md](../../docs/SECURITY_DESIGN.md) §7.

**Not built yet.** Teams, departments, groups, avatars, org domains, SCIM, audit retention, invitation
email (the link is returned once to the inviter until the notification service exists).

| Variable | Required | Default | Purpose |
|---|---|---|---|
| `DATABASE_URL` (+ `DATABASE_URL__<DEST>`) | yes | | pool per logical database destination |
| `DATABASE_MIGRATION_URL` | for `migrate` | | owner role |
| `REDIS_URL`, `TOKEN_ISSUER`, `JWKS_URL` | yes | | token verification and revocation |
| `INTERNAL_SERVICE_SECRET` | yes | | service tokens |
| `TENANT_SERVICE_GRPC_URL` | no | `http://127.0.0.1:9102` | provisioning and placement |
| `WEB_BASE_URL`, `ALLOWED_ORIGINS` | no | `http://localhost:5173` | invitation links, CORS |
| `HTTP_ADDR` / `GRPC_ADDR` | no | `127.0.0.1:8103` / `:9103` | |

**Run / test / observe / image.** As for tenant-service, with `user-service`.
