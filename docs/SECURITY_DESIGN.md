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
| MFA / WebAuthn / OIDC / SAML / SCIM | Phase 6, with `totp-rs`, `webauthn-rs`, `openidconnect`, `samael`. No hand-rolled protocol or crypto code |
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
| Abuse | gateway global + per-IP limits; service per-user/tenant token bucket in Redis; body size limits; pagination caps |

## 6. Secrets and data protection

- No secrets in the repository; `.env.example` contains only obviously fake development values.
- Kubernetes: External Secrets Operator → cloud secret manager; workload identity for cloud APIs.
- Integration credentials and IdP secrets: envelope encryption (AES-256-GCM data key wrapped by KMS), per tenant.
- TLS everywhere outside the pod; encryption at rest on all managed stores; encrypted backups.
- Logs never contain credentials, tokens, cookies or request bodies; emails are logged hashed.

## 7. Audit

Security-relevant actions emit `audit.recorded` (actor, action, resource, tenant, request id, metadata, IP).
The audit store is append-only (no UPDATE/DELETE grants) and tamper-evident: each row stores
`hash = SHA-256(prev_hash || canonical_row)` per tenant chain; a verifier job and an export endpoint exist in
Phase 6. Retention is configurable per organization within plan limits.

## 8. Supply chain

`cargo audit` + `cargo deny`, `pnpm audit`, Trivy image scan and SBOM generation in CI; distroless non-root
images, read-only root filesystem, dropped capabilities.

## 9. Not yet done

Tracked honestly in the roadmap: everything marked Phase 6 above, KMS integration (dev uses an env key),
pen-test, and the security test suite beyond the per-service cross-tenant tests.
