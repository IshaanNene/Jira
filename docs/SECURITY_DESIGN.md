# Security design

## 1. Trust boundaries

```
Internet ─┬─ [ Envoy Gateway ]  TLS, WAF-ish limits, coarse JWT, rate limit
          │        │
          │   [ services ]      each one: verify JWT, authorize, validate, tenant-scoped tx
          │        │
          │   [ data stores ]   RLS, per-service roles, encryption at rest, network policy
```

Never trusted: client-supplied tenant ids, roles, permissions, database destinations, object keys; event payloads
before validation; anything in `X-Forwarded-*` except from the gateway.
Internal ports (gRPC, metrics) are ClusterIP-only and covered by default-deny NetworkPolicies.

## 2. Authentication

| Item | Design |
|---|---|
| Passwords | Argon2id (`argon2` crate), m=19 MiB, t=2, p=1 (OWASP baseline), per-password salt, PHC string. Hashing runs on `spawn_blocking`. Min length 10, max 128, checked against a breached-password list in Phase 6 |
| Access token | JWT, **EdDSA (Ed25519)**, 10 min. Claims: `iss`, `aud`, `sub` (user), `sid` (session), `typ` (`account` \| `org`), and for org tokens `tid`, `org`, `roles`, `pv` (policy version). Held in memory by the SPA — not in localStorage |
| Refresh token | 256-bit random, opaque, stored as SHA-256; `HttpOnly; Secure; SameSite=Strict; Path=/api/v1/auth`. **Rotated on every use**; presenting an already-used token revokes the whole session (theft detection) |
| Keys | Ed25519 key pairs in `signing_keys`, private part encrypted with a KMS-held key (env-provided key in dev). JWKS published; `kid` rotation with overlap |
| Session revocation | `sessions.revoked_at` + Redis set `revoked:{sid}` with TTL = access-token lifetime; checked in the auth middleware |
| Brute force | per-account exponential lockout + per-IP rate limit; identical error and timing for unknown email vs wrong password |
| Email flows | single-use, hashed, expiring tokens; responses never reveal whether an email exists |
| Two-step verification (TOTP) | Built. RFC 6238 (HMAC-SHA1 from `ring`, 6 digits, 30 s, ±1 step) is a dozen lines, so it is implemented directly and checked against the RFC's test vectors rather than adding a crate. Secrets sealed with AES-256-GCM under the key-encryption key, bound to the account id as associated data; a used time step is never accepted again; 10 recovery codes of 60 bits, stored as SHA-256 of account id and code. Sign-in: correct password → `202` with a single-use challenge (5 min, 5 tries); wrong codes count towards the account lockout and the password-stage success does not reset it. Set-up, new codes and turning off need the password again (failures count towards the lockout); turning it on or off ends the other sessions |
| WebAuthn / OIDC / SAML / SCIM | Phase 6, with `webauthn-rs`, `openidconnect`, `samael`. No hand-rolled protocol code for these |
| API tokens | random, prefixed (`mrd_pat_…`) for secret scanning, stored hashed, scoped, expiring, tenant-bound |

CSRF: state-changing API calls require the bearer header, which a cross-site form cannot set. The only
cookie-authenticated endpoints (`/auth/token/refresh`, `/auth/logout`) are `SameSite=Strict`, require
`Content-Type: application/json` and an `Origin` allow-list check.

## 3. Tenant isolation — defence in depth

1. Tenant comes only from a signed token (or signed service metadata, or a validated event envelope).
2. `DbRouter` picks the pool from registry placement resolved by logical name → local secret.
3. `SET LOCAL app.tenant_id` + FORCE RLS on every tenant-owned table; runtime role cannot bypass.
4. Queries still include `tenant_id` predicates explicitly (index use, and a second check).
5. Cache keys, search filters, object keys, realtime topics are all tenant-prefixed server-side.
6. Tests: each service has cross-tenant read/write tests; CI fails if a table with `tenant_id` lacks FORCE RLS.

## 4. Authorization

Model: **RBAC with schemes, evaluated locally from a versioned snapshot.**

- Subjects: user, group, team. Roles: organization roles (owner, admin, member, guest), project roles
  (administrator, member, viewer, custom), team roles.
- A *permission scheme* maps permissions (`project.browse`, `issue.create`, `issue.edit`, `issue.transition`,
  `issue.delete`, `comment.add`, `sprint.manage`, `workflow.administer`, `org.members.manage`, …) to roles/groups.
- Issue security levels and field-level rules refine visibility (Phase 6).
- Decision inputs: user, tenant, organization, project, resource type + id, role bindings, scheme, policy version.

Enforcement path in a service:

```
AuthContext (from token: org roles, pv)
   └─ org-level permission? decided from claims alone
   └─ project/issue-level? PolicySnapshot for (tenant, user):
        in-process LRU (30 s) -> Redis t:{tid}:policy:{user}:{pv} -> permission-service gRPC
```

`pv` is bumped by permission-service on any grant change and announced via `permission.updated`; a snapshot with
an older `pv` than the newest seen for the tenant is discarded. Deny by default. Handlers call
`authz.require(Permission::X, resource)` — there is no handler without an explicit authorization call
(enforced by a test that walks the router and by code review).

Until permission-service exists (Phase 2), Phase 1 endpoints need only organization-level permissions, which are
derived from the `roles` claim by a static table in `auth-middleware::authz`.

### Service-to-service

Internal gRPC requires a 60-second HS256 token (caller in `iss`, callee in `aud`) on top of mesh mTLS
and NetworkPolicies. Trade-off: the signing secret is shared by platform workloads, so a compromised
service could impersonate another; per-workload identity (SPIFFE/SPIRE) is the planned replacement.

## 5. Input and output

| Threat | Control |
|---|---|
| SQL injection | SQLx bind parameters only; dynamic filters via `QueryBuilder::push_bind`; QL → AST → planner |
| XSS | React escaping; rich text stored as TipTap JSON and rendered by schema (no `dangerouslySetInnerHTML`); server validates the document against an allow-listed node/mark set; CSP `default-src 'self'` |
| SSRF | outbound URLs (webhooks, avatars-by-URL, IdP metadata) resolved and checked against private/link-local/metadata ranges at **connect** time (custom DNS resolver), redirects disabled, HTTPS only |
| File upload | size limit at gateway and service; content sniffing (`infer`) vs declared type; extension allow-list; object key generated server-side; malware scan hook sets `scan_status`; downloads via short-lived pre-signed URL issued after an authorization check, `Content-Disposition: attachment` |
| Mass assignment | separate request DTOs; `deny_unknown_fields` |
| Headers | HSTS, `X-Content-Type-Options`, `Referrer-Policy`, `frame-ancestors 'none'`; strict CORS allow-list |
| Outgoing email | names and summaries HTML-escaped and flattened to one line in subjects (no header injection); links only to well-formed issue keys under the configured `APP_URL`; recipient addresses never logged; finished deliveries deleted after 7 days |
| Search queries (MQL) | parsed and compiled server-side into a query built from fixed shapes: values become term values, analysed match text, or date math assembled from validated parts (no query-string or script syntax reaches OpenSearch); always nested inside the tenant and visible-project filters; bounded size, depth, list length and sort fields; cursors must match the query's sort shape. Looking people up by email or name requires the member-directory permission (guests cannot), so the query box is not a way around the directory |
| Unsubscribe links | public endpoint (gateway exempts only `/api/v1/notifications/unsubscribe`); HMAC-SHA256 token over (tenant, user, kind, issued-at) with a purpose-derived key, 90-day expiry, constant-time verification; the token can only turn email **off**; `GET` only describes the link (mail scanners prefetch links), `POST` acts (RFC 8058) |
| Abuse | gateway global + per-IP limits; service per-user/tenant token bucket in Redis; body size limits; pagination caps |

## 6. Secrets and data protection

- No secrets in the repository; `.env.example` contains only obviously fake development values.
- Kubernetes: External Secrets Operator → cloud secret manager; workload identity for cloud APIs.
- Integration credentials and IdP secrets: envelope encryption (AES-256-GCM data key wrapped by KMS), per tenant.
- TLS everywhere outside the pod; encryption at rest on all managed stores; encrypted backups.
- Logs never contain credentials, tokens, cookies or request bodies; emails are logged hashed.

## 7. Audit

Admin-relevant actions are recorded from domain events and `audit.recorded` (actor, action, resource, tenant,
request id, details) by a consumer in user-service (ADR-0009).

- **Append-only.** A trigger refuses UPDATE, DELETE and TRUNCATE on `audit_log` for every role, and the migration
  revokes those grants from every role but the owner. Forced row-level security scopes reads to the tenant.
- **Tamper-evident.** Each tenant has one chain: `hash = SHA-256(prev_hash || canonical JSON of the row)`, starting
  from 32 zero bytes. Appends take a per-tenant advisory lock, so sequence numbers have no gaps.
  `GET /organizations/current/audit/verify` recomputes the chain and reports the first entry that is missing,
  out of order or altered (tested by editing a row as the owner with the trigger disabled).
- **Export.** CSV for admins and owners only, `Cache-Control: no-store`; every cell is quoted and a leading
  `= + - @` tab or carriage return is prefixed with `'` so a spreadsheet never evaluates it.
- **Not yet.** Client IP address, per-organization retention, a scheduled verifier, sign-in events.

## 8. Supply chain

`cargo audit` + `cargo deny`, `pnpm audit`, Trivy image scan and SBOM generation in CI; distroless non-root
images, read-only root filesystem, dropped capabilities.

## 9. Not yet done

Tracked honestly in the roadmap: everything marked Phase 6 above, KMS integration (dev uses an env key),
pen-test, and the security test suite beyond the per-service cross-tenant tests.
