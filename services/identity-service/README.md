# identity-service

**Responsibility.** Accounts, password authentication, sessions, access-token issuance, refresh-token
rotation, organization-scoped token exchange, signing keys (JWKS).

**API.** REST under `/api/v1/auth` — contract at `/openapi.json` (`/api/docs/identity/openapi.json`
through the gateway): `register`, `login`, `token/refresh`, `token/exchange`, `logout`, `me`,
`sessions`, `.well-known/jwks.json`. Calls user-service over gRPC (`CheckMembership`) before signing
an organization token.

**Security notes.** Argon2id (19 MiB, t=2) off the async runtime; identical response for unknown
email and wrong password; lockout from the 5th failure; refresh tokens stored as SHA-256, rotated on
every use, reuse after a 10 s grace revokes the session; Ed25519 keys sealed with AES-256-GCM;
cookie endpoints check `Origin`; revoked sessions are published to Redis (`revoked:{sid}`).

**Database.** Owns `identity_db`: `accounts`, `sessions`, `refresh_tokens`, `signing_keys`,
`login_events`, `outbox`, `inbox`. Accounts are global, not tenant-owned.

**Not built yet.** Email verification and password reset (need email delivery), MFA/TOTP, WebAuthn,
OIDC/SAML, API tokens, suspicious-login detection, signing-key rotation command.

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
| `SESSION_TTL_DAYS`, `TRUST_FORWARDED_FOR`, `HTTP_ADDR` | no | `30`, `false`, `127.0.0.1:8101` | |

**Run / test / observe / image.** As for tenant-service, with `identity-service`.
