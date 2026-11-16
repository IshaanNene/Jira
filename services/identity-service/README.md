# identity-service

**Responsibility.** Accounts, password authentication, sessions, access-token issuance, refresh-token
rotation, organization-scoped token exchange, signing keys (JWKS).

**API.** REST under `/api/v1/auth` — contract at `/openapi.json` (`/api/docs/identity/openapi.json`
through the gateway): `register`, `login`, `token/refresh`, `token/exchange`, `logout`, `me`,
`sessions`, `.well-known/jwks.json`, and two-step verification: `login/mfa`, `mfa`, `mfa/totp/setup`,
`mfa/totp/confirm`, `mfa/totp/disable`, `mfa/recovery-codes`, and account email: `email/verification`
(send a link), `email/verify`, `password/forgot`, `password/reset`, `email/change`, `email/change/confirm`. Calls user-service over gRPC (`CheckMembership`) before signing
an organization token.

**Security notes.** Argon2id (19 MiB, t=2) off the async runtime; identical response for unknown
email and wrong password; lockout from the 5th failure; refresh tokens stored as SHA-256, rotated on
every use, reuse after a 10 s grace revokes the session; Ed25519 keys sealed with AES-256-GCM;
cookie endpoints check `Origin`; revoked sessions are published to Redis (`revoked:{sid}`).
Two-step verification: TOTP secrets sealed under the same key-encryption key and bound to the account;
no code is accepted twice; wrong codes count towards the lockout; see SECURITY_DESIGN.md §2.

**Email verification and password reset.** Registering emails a link to `/verify-email`; a signed-in person
can ask for another. A forgotten password is reset through a link to `/reset-password` sent to the account's
address. Each link carries a 256-bit random token of which only the SHA-256 is stored (`email_tokens`); it works
once (using one spends the account's other open links of that kind), expires (verification 24 hours, reset 1 hour)
and stops working if the account's address changes. At most one link per minute and five per hour per account and
kind. `password/forgot` answers `202` for any address and does the lookup and sending in the background, so neither
the answer nor its timing says whether an account exists. A reset sets the password, clears the lockout, marks the
address verified and ends every session; two-step verification still applies at the next sign-in, and API tokens
keep working. Access tokens carry `email_verified`, and user-service refuses an invitation to anyone whose token
lacks it, so registering an invited address first no longer gets anyone in. Emails go out over SMTP through the
shared `smtp-mailer` crate; logs name the user id and the kind of link, never the address or the token. The gateway
puts `login/mfa`, `password/forgot`, `password/reset`, `email/verify` and both `email/change` routes in the tight
credential bucket.

**Changing the address.** `email/change` takes the new address and the current password and emails a link to the
new address (the same rules as other links; asking again voids the previous change link, so only the latest address
can be confirmed). Confirming moves the account, marks the address verified, emits `identity.email_changed` with the
previous address, and tells the old address, showing only the first letter and domain of the new one. Links sent to
the old address stop working with it.

**Passkeys.** `passkeys` (list, add), `passkeys/options` (start adding; needs the password), `passkeys/{id}`
(rename), `passkeys/{id}/remove` (needs the password), and the unauthenticated `passkeys/sign-in/options` and
`passkeys/sign-in`. Discoverable credentials with user verification required; the checks are in `src/webauthn.rs` (see
SECURITY_DESIGN.md §2). Passkeys are bound to `WEBAUTHN_RP_ID` (default: the host of the first allowed origin) and
accepted from `WEBAUTHN_ORIGINS` (default: `ALLOWED_ORIGINS`); origins that are not https (other than localhost) or not
on that domain are left out with a warning. Sessions record how they were signed in to (`password`, `two_step`,
`passkey`); a passkey session meets a workspace's two-step requirement.

**Single sign-on (OIDC).** `sso/connection` (GET, PUT, DELETE; workspace admins), `sso/workspace` (any member: is it
on, the sign-in name, am I linked), `sso/link` and `sso/links` (a signed-in person links or unlinks their own account),
`sso/start` (unauthenticated, by sign-in name) and `sso/callback` (the provider sends the browser back; always answers
with a redirect into the web app). The redirect URI is `{APP_URL}/api/v1/auth/sso/callback`. Protocol work is in
`src/sso.rs` with the `openidconnect` crate; requests to providers refuse private addresses (`SSO_ALLOW_HTTP` and
`SSO_ALLOW_PRIVATE_NETWORKS` relax this for development only). See SECURITY_DESIGN.md §2 for linking, login CSRF,
SSRF and how an SSO session is scoped.

**Single sign-on (SAML 2.0).** The same `sso/connection` takes `"protocol": "saml"` with the provider's metadata XML
(blank on a later save keeps it). `sso/saml/{organization_id}` serves the workspace's service-provider metadata (its
entity ID is that URL); the provider posts to `sso/saml/acs`, which parks the response on the sign-in named by
`RelayState` and redirects to `sso/saml/continue`, where the browser's cookie is checked and the response verified
with `samael` (`src/saml.rs`). Building needs libxml2, xmlsec1 and OpenSSL (see the root README); the image is the
Dockerfile's `xmlsec` runtime.

**Admin console API.** `admin/two-step` and `admin/api-tokens` let a workspace's admins and owners (signed in, not
through an API token) see which active members have two-step verification on and list or revoke the API tokens issued
for their workspace. Membership is checked over gRPC, so ids outside the workspace are silently left out; a revocation
is written to the audit log and published to Redis like any other. There is no admin reset of someone's two-step
verification: accounts are global.

**Database.** Owns `identity_db`: `accounts`, `sessions`, `refresh_tokens`, `signing_keys`,
`login_events`, `email_tokens`, `totp_factors`, `recovery_codes`, `mfa_challenges`, `passkeys`, `webauthn_challenges`, `sso_connections`, `sso_identities`, `sso_states`, `outbox`, `inbox`. Accounts are global, not tenant-owned.

**Not built yet.** An email when the password or two-step verification
changes; suspicious-login detection; signing-key rotation command; signed SAML requests and encrypted assertions.

| Variable | Required | Default | Purpose |
|---|---|---|---|
| `DATABASE_URL`, `DATABASE_MIGRATION_URL` | yes / for `migrate` | | database |
| `REDIS_URL` | yes | | revocation list (service still starts if Redis is down) |
| `TOKEN_ISSUER` | yes | | `iss` claim; must equal the public gateway origin |
| `IDENTITY_KEY_ENCRYPTION_KEY` | yes | | base64 32-byte key sealing signing keys (KMS in production) |
| `INTERNAL_SERVICE_SECRET` | yes | | signs service tokens |
| `USER_SERVICE_GRPC_URL` | no | `http://127.0.0.1:9103` | membership checks |
| `ALLOWED_ORIGINS` | yes in browsers | | CORS + Origin allow-list |
| `COOKIE_SECURE` | no | `true` | set `false` only for local http |
| `SMTP_URL` | no | unset: no verification or reset emails (`503`) | mail server, as for notification-service |
| `EMAIL_FROM`, `APP_URL` | with `SMTP_URL` | `Meridian <accounts@meridian.localhost>`, – | sender; web app base for links |
| `SESSION_TTL_DAYS`, `TRUST_FORWARDED_FOR`, `HTTP_ADDR` | no | `30`, `false`, `127.0.0.1:8101` | |

**Run / test / observe / image.** As for tenant-service, with `identity-service`.
