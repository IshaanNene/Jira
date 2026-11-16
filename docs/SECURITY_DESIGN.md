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
| Passkeys (WebAuthn) | Built. **A deviation from the plan to use `webauthn-rs`:** that crate needs the `openssl` crate, and the service images were then deliberately built without OpenSSL, so the relying-party checks are written in `services/identity-service/src/webauthn.rs` over `ring`, with a bounded CBOR reader (`cbor.rs`: definite lengths only, lengths checked against the input, depth 8, 256 items). It follows the spec's verification steps: client data type, challenge (constant-time) and exact origin, no cross-origin iframes, RP id hash, user presence **and** user verification required, signature over `authData ‖ SHA-256(clientDataJSON)`, signature counter must rise when either side is non-zero (a possible clone is refused and logged). ES256, EdDSA and RS256 keys. Attestation is requested as `none` and never relied on, so any authenticator is accepted. Discoverable credentials only: the sign-in options name no account, and the returned user handle must be the owning account's id. Challenges are 256-bit, stored as SHA-256, single use (spent before the response is checked), 5 minutes. Adding or removing a passkey needs the password; at most 20 per account. A passkey sign-in starts a session marked `passkey`, which meets a workspace's two-step requirement; a password session does not, even if the account has a passkey. Both sign-in endpoints are in the gateway's credential rate-limit bucket. Covered by unit tests with a software authenticator (wrong origin, challenge, RP, key, type, tampered data, missing UV/UP, cross-origin, replayed counter) and HTTP tests; it has not had an external review, and should get one before production use |
| Single sign-on (OIDC) | Built, with the `openidconnect` crate (reqwest + rustls, no OpenSSL): discovery, authorization code flow with PKCE (S256) and a nonce, ID token signature against the provider's JWKS, issuer, audience and expiry; asymmetric algorithms only (RS*, PS256, ES256/384, EdDSA), so an HMAC token keyed with the client secret is refused. One provider per workspace, set up by its admins; the client secret is sealed with AES-256-GCM bound to the organization id and never returned. **Account linking is never automatic:** a provider is run by a workspace's admins, so an email address in its ID token proves nothing about a Meridian account (accounts are global). A link from (workspace, issuer, subject) to an account is made only by that account's owner, signed in, starting the flow from Settings; one identity per account per workspace and one account per identity. **Login CSRF:** the `state` (stored as SHA-256) is single use and spent before anything else is checked, and must arrive with the `HttpOnly; SameSite=Lax` cookie set when the flow started (also hashed), so a callback link from someone else's browser signs nobody in. **SSRF:** the issuer and every endpoint in its discovery document must be https and must not be a literal private address; names resolve through a resolver that refuses the whole name if any address is loopback, private, link-local, CGNAT, documentation, multicast or reserved (the same rule as webhooks, now in `platform-common::net`); no redirects; 10 s timeout. `SSO_ALLOW_HTTP` / `SSO_ALLOW_PRIVATE_NETWORKS` exist for development and tests only. **Scope:** a session signed in through workspace A's provider enters only A (`403 sso_other_workspace` elsewhere), so A's admins cannot reach their members' other workspaces. A workspace can require SSO: other sessions get `403 sso_required`, except owners (break-glass if the provider fails) and API tokens. A sign-in through the workspace's own provider meets its two-step requirement (the provider applies its own policy). Changing the issuer deletes all links. Setting, changing and removing the connection are audited. `sso/start` is in the gateway's credential rate-limit bucket. Covered by HTTP tests against a local fake provider (real ES256 tokens, PKCE and client-secret checks): wrong browser, replayed state, wrong nonce, forged signature, cancelled, unlinked identity, identity already linked elsewhere, cross-workspace use, required SSO |
| SCIM provisioning | Built in user-service, SCIM 2.0 users and groups (`/scim/v2/Users`, `/scim/v2/Groups`, `ServiceProviderConfig`, `ResourceTypes`, `Schemas`). **Tokens:** a workspace provisioning token (`mscim_` + 32 random bytes) is shown once; only its SHA-256 is stored, in a global index (hash → tenant, like invitation tokens) beside the tenant's `scim_tokens` row under forced RLS. At most 5 per workspace; creating and revoking are audited. A token acts with the role of the admin who created it, **re-read on every request**: when they stop being an active admin or owner, the token stops working. Malformed tokens are refused before any lookup; `/scim/v2/` is outside the platform JWT rules and in its own tight rate-limit bucket at the gateway. **What it can do:** only what that admin could do in the app, through the same membership rules (admins cannot touch owners; the last active owner cannot be suspended or removed; a token cannot remove its own creator). A new person is *invited* by email as a member (role `member`, never higher) and joins only by accepting with that verified address, so provisioning never creates or takes over an account; a person whose address already belongs to a member is linked to that member. `active: false` suspends the member or withdraws the invitation; `DELETE` removes the member or withdraws the invitation. Names from the provider stay on the provisioned record and never change the person's shared profile. Membership changes and invitations are attributed to `scim:<token id>` in the audit log. **Groups:** kept as the provider sends them (display name unique per workspace, at most 1,000 groups and 10,000 members each; members must be users this workspace provisioned, no nested groups). A group's *role* is set only by a workspace admin in the app (`admin`, `member` or `guest`, never `owner`, and never stronger than the admin's own; audited as `provisioning.group_role_set`), never by the provider. A provisioned person's role is the strongest role among their groups that have one; `scim_users.group_role` remembers what groups last decided, so leaving every such group returns them to `member` and nobody else's hand-set role is touched. Changes go through the same rule check as the app, with the token creator's current role (or the mapping admin's): owners are never changed by groups, nor is the acting admin themselves; a pending invitation takes the role and the person joins with it. Group writes take the same workspace lock as other provisioning requests and role mapping. A role set by hand on a group-managed member is overwritten by the next group change for them. **Input:** the body is parsed as JSON by the service (any content type), attributes are bounded and control characters refused, unknown attributes ignored, at most 100 PATCH operations, only `eq` filters on `userName`, `externalId`, `emails.value` and `id` (bound parameters). Covered by 8 unit tests and 2 HTTP tests (token rules, invite, accept, link, Entra-style patches, owner protection, withdrawn invitation, delete, attribution, another workspace's token, demoted creator, revoked token; groups: unknown members, duplicate names, filters, mapping refused to members and to `owner`, owner and token creator untouched, invitation carries the role, Okta and Entra member patches, strongest role wins, deleting a group and unmapping restore `member`) |
| SAML single sign-on | Built in identity-service with `samael` (signatures checked by xmlsec1, the C library), beside OIDC: a workspace's connection is one or the other, and links, sessions, "require SSO" and the sign-in-name lookup are shared. **Provider:** described by metadata XML the admin pastes (≤ 200 kB). Nothing is fetched, so there is no request forgery; the metadata must name a signing certificate (without one samael would accept unsigned responses, so such metadata is refused at save and again at every check), an HTTP-Redirect sign-in address that is `https` (and not a private address unless `SSO_ALLOW_PRIVATE_NETWORKS`), and must not require signed requests. **Audience:** each workspace is its own service provider, entity ID `{APP_URL}/api/v1/auth/sso/saml/{organization id}` (its metadata is served there), so an assertion made for one workspace is refused by every other even when both use the same provider. **Flow:** the AuthnRequest ID is `_` + 256 random bits and is remembered with the sign-in as for OIDC (hashed `RelayState`, 10 minutes, single use, `SameSite=Lax` browser cookie). The provider POSTs to the global ACS `{APP_URL}/api/v1/auth/sso/saml/acs`; a cross-site POST carries no Lax cookie, so the response (≤ 300 kB base64) is only parked on its still-open sign-in (once) and the browser is redirected to `sso/saml/continue`, a top-level GET that does carry the cookie; there the sign-in is spent, the cookie compared in constant time (login CSRF), and the response verified. **Checks (samael):** signature with the metadata's certificates, and the document reduced to what the signature covers before anything is read (signature wrapping); only RSA/ECDSA with SHA-256/384/512 for signatures and digests (SHA-1 refused); `InResponseTo` must be the remembered request ID, so unsolicited (IdP-initiated) responses are refused, and a response without `RelayState` is too; issuer, destination and recipient (the ACS), audience, status, `NotOnOrAfter`, issue instant within 90 s and 3 min clock skew. The identity is (workspace, provider entity ID, `NameID`); the email comes from an `email`/`mail`/claims attribute or a `NameID` containing `@`, and is shown only. Verification runs on a blocking thread. **Not supported:** signed AuthnRequests and encrypted assertions (no service-provider key is kept), single logout, IdP-initiated sign-in. **Native code:** xmlsec1, libxml2 and OpenSSL are C; every service image is Debian slim with them from Debian packages (scanned by Trivy) rather than distroless, and every service now links the system OpenSSL rather than a vendored copy so a process never carries two. The `openssl` crate samael brings in is held at ≥ 0.10.80 (RUSTSEC-2026-0463 and 0473–0479). Covered by 2 identity HTTP tests against a fake provider that signs with its own key: link and sign-in, metadata without a certificate or requiring signed requests, wrong key, tampering after signing, another request's ID, another workspace's audience, another browser, unsolicited, a second POST and replay after use, and the OIDC callback refusing a SAML sign-in. Not checked against a real Okta, Entra ID or Google tenant yet |
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
| Automation rules | rules are data from a closed set of types (`deny_unknown_fields` on every struct and tagged enum); nothing in a rule is executed as code, and comment text is plain text placed in text nodes of a validated document. Triggered rules act as the person whose change triggered them, with that person's permissions (a move needs the transition permission and goes through the workflow; a comment needs the comment permission). Scheduled rules act as the administrator who last saved them, re-checked at every run (active membership from user-service, project-administrator role from the permission snapshot); the workspace id comes from that administrator's signed token at save time. Changes made by rules never trigger rules. The schedule queue holds only rule ids, tenant ids and times |
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

`cargo audit` + `cargo deny`, `pnpm audit`, Trivy image scan and SBOM generation in CI; Debian slim images upgraded at
build time, non-root, read-only root filesystem, dropped capabilities.

## 9. Not yet done

Tracked honestly in the roadmap: everything marked Phase 6 above, KMS integration (dev uses an env key),
pen-test, and the security test suite beyond the per-service cross-tenant tests.
