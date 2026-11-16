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
| Two-step verification (TOTP) | Built. RFC 6238 (HMAC-SHA1 from `ring`, 6 digits, 30 s, ±1 step) is a dozen lines, so it is implemented directly and checked against the RFC's test vectors rather than adding a crate. Secrets sealed with AES-256-GCM under the key-encryption key, bound to the account id as associated data; a used time step is never accepted again; 10 recovery codes of 60 bits, stored as SHA-256 of account id and code. Sign-in: correct password → `202` with a single-use challenge (5 min, 5 tries); wrong codes count towards the account lockout and the password-stage success does not reset it. Set-up, new codes and turning off need the password again (failures count towards the lockout); turning it on or off ends the other sessions. A workspace can require it: identity-service learns this from the membership check and issues no workspace token (session, refresh or API-token exchange) to a member without it (`403 two_step_required`) |
| Passkeys (WebAuthn) | Built. **A deviation from the plan to use `webauthn-rs`:** that crate needs the `openssl` crate, and the service images are deliberately built without OpenSSL, so the relying-party checks are written in `services/identity-service/src/webauthn.rs` over `ring`, with a bounded CBOR reader (`cbor.rs`: definite lengths only, lengths checked against the input, depth 8, 256 items). It follows the spec's verification steps: client data type, challenge (constant-time) and exact origin, no cross-origin iframes, RP id hash, user presence **and** user verification required, signature over `authData ‖ SHA-256(clientDataJSON)`, signature counter must rise when either side is non-zero (a possible clone is refused and logged). ES256, EdDSA and RS256 keys. Attestation is requested as `none` and never relied on, so any authenticator is accepted. Discoverable credentials only: the sign-in options name no account, and the returned user handle must be the owning account's id. Challenges are 256-bit, stored as SHA-256, single use (spent before the response is checked), 5 minutes. Adding or removing a passkey needs the password; at most 20 per account. A passkey sign-in starts a session marked `passkey`, which meets a workspace's two-step requirement; a password session does not, even if the account has a passkey. Both sign-in endpoints are in the gateway's credential rate-limit bucket. Covered by unit tests with a software authenticator (wrong origin, challenge, RP, key, type, tampered data, missing UV/UP, cross-origin, replayed counter) and HTTP tests; it has not had an external review, and should get one before production use |
| Single sign-on (OIDC) | Built, with the `openidconnect` crate (reqwest + rustls, no OpenSSL): discovery, authorization code flow with PKCE (S256) and a nonce, ID token signature against the provider's JWKS, issuer, audience and expiry; asymmetric algorithms only (RS*, PS256, ES256/384, EdDSA), so an HMAC token keyed with the client secret is refused. One provider per workspace, set up by its admins; the client secret is sealed with AES-256-GCM bound to the organization id and never returned. **Account linking is never automatic:** a provider is run by a workspace's admins, so an email address in its ID token proves nothing about a Meridian account (accounts are global). A link from (workspace, issuer, subject) to an account is made only by that account's owner, signed in, starting the flow from Settings; one identity per account per workspace and one account per identity. **Login CSRF:** the `state` (stored as SHA-256) is single use and spent before anything else is checked, and must arrive with the `HttpOnly; SameSite=Lax` cookie set when the flow started (also hashed), so a callback link from someone else's browser signs nobody in. **SSRF:** the issuer and every endpoint in its discovery document must be https and must not be a literal private address; names resolve through a resolver that refuses the whole name if any address is loopback, private, link-local, CGNAT, documentation, multicast or reserved (the same rule as webhooks, now in `platform-common::net`); no redirects; 10 s timeout. `SSO_ALLOW_HTTP` / `SSO_ALLOW_PRIVATE_NETWORKS` exist for development and tests only. **Scope:** a session signed in through workspace A's provider enters only A (`403 sso_other_workspace` elsewhere), so A's admins cannot reach their members' other workspaces. A workspace can require SSO: other sessions get `403 sso_required`, except owners (break-glass if the provider fails) and API tokens. A sign-in through the workspace's own provider meets its two-step requirement (the provider applies its own policy). Changing the issuer deletes all links. Setting, changing and removing the connection are audited. `sso/start` is in the gateway's credential rate-limit bucket. Covered by HTTP tests against a local fake provider (real ES256 tokens, PKCE and client-secret checks): wrong browser, replayed state, wrong nonce, forged signature, cancelled, unlinked identity, identity already linked elsewhere, cross-workspace use, required SSO |
| SCIM provisioning | Built in user-service, SCIM 2.0 users only (`/scim/v2/Users`, `ServiceProviderConfig`, `ResourceTypes`, `Schemas`; `Groups` answers 404). **Tokens:** a workspace provisioning token (`mscim_` + 32 random bytes) is shown once; only its SHA-256 is stored, in a global index (hash → tenant, like invitation tokens) beside the tenant's `scim_tokens` row under forced RLS. At most 5 per workspace; creating and revoking are audited. A token acts with the role of the admin who created it, **re-read on every request**: when they stop being an active admin or owner, the token stops working. Malformed tokens are refused before any lookup; `/scim/v2/` is outside the platform JWT rules and in its own tight rate-limit bucket at the gateway. **What it can do:** only what that admin could do in the app, through the same membership rules (admins cannot touch owners; the last active owner cannot be suspended or removed; a token cannot remove its own creator). A new person is *invited* by email as a member (role `member`, never higher) and joins only by accepting with that verified address, so provisioning never creates or takes over an account; a person whose address already belongs to a member is linked to that member. `active: false` suspends the member or withdraws the invitation; `DELETE` removes the member or withdraws the invitation. Names from the provider stay on the provisioned record and never change the person's shared profile. Membership changes and invitations are attributed to `scim:<token id>` in the audit log. **Input:** the body is parsed as JSON by the service (any content type), attributes are bounded and control characters refused, unknown attributes ignored, at most 100 PATCH operations, only `eq` filters on `userName`, `externalId`, `emails.value` and `id` (bound parameters). Covered by 6 unit tests and 1 HTTP test (token rules, invite, accept, link, Entra-style patches, owner protection, withdrawn invitation, delete, attribution, another workspace's token, demoted creator, revoked token) |
| SAML | Phase 6, with `samael`. No hand-rolled protocol code |
| API tokens | 256 random bits prefixed `mrd_` (easy to scan for), only the SHA-256 stored, expiring (≤ 365 days), bound to one person in one organization; exchanged for a 10-minute access token marked `api` that cannot manage credentials; membership re-checked at every exchange. Not yet narrower than the person's own access |

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
