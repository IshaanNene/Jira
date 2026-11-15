-- API tokens: long-lived secrets for scripts and integrations, one person in one organization.
-- A token is exchanged for a short-lived access token (POST /api/v1/auth/token/api); membership
-- and role are checked again at every exchange, so a token never outlives its owner's access.

CREATE TABLE api_tokens (
    id              uuid PRIMARY KEY,
    account_id      uuid NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    organization_id uuid NOT NULL,
    name            text NOT NULL CHECK (char_length(name) BETWEEN 1 AND 100),
    -- SHA-256 of the secret, which has 256 bits of entropy; the secret is shown once.
    token_hash      bytea NOT NULL UNIQUE,
    -- The first characters of the secret, so a person can tell their tokens apart.
    hint            text NOT NULL,
    created_at      timestamptz NOT NULL DEFAULT now(),
    expires_at      timestamptz NOT NULL,
    last_used_at    timestamptz,
    revoked_at      timestamptz,
    CHECK (expires_at > created_at)
);
CREATE INDEX api_tokens_owner_idx ON api_tokens (account_id, organization_id) WHERE revoked_at IS NULL;
