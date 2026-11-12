# ADR-0003: Short-lived EdDSA access tokens, rotating opaque refresh tokens, org-scoped token exchange

Status: accepted

**Decision.** Identity issues 10-minute Ed25519 JWTs verified locally by every service via JWKS. A login yields an *account* token; an *organization* token (carrying `tid`, `org`, `roles`, `pv`) is obtained by exchange after identity confirms membership with user-service. Refresh tokens are opaque, hashed at rest, rotated on use with reuse detection, delivered in an HttpOnly SameSite=Strict cookie. The SPA keeps the access token in memory only.

**Why.** Local verification removes identity from the hot path. Putting the tenant in a signed claim is what makes "never trust client-supplied tenant ids" enforceable. Asymmetric keys mean no service other than identity can mint tokens.

**Consequences.** Revocation is not instant by signature alone → Redis revocation list keyed by session id with TTL = token lifetime. Role changes take effect at next refresh/exchange or when `pv` is bumped.
