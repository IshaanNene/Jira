-- Two-step verification: an authenticator-app secret per account, single-use recovery
-- codes, and short-lived sign-in challenges between the password and the code.

CREATE TABLE totp_factors (
    account_id     uuid PRIMARY KEY REFERENCES accounts (id) ON DELETE CASCADE,
    -- The shared secret, AES-256-GCM sealed under the key-encryption key and bound to
    -- the account id, so a copied row does not open for another account.
    secret_enc     bytea NOT NULL,
    nonce          bytea NOT NULL,
    -- NULL while being set up: not used at sign-in until a code has proved the app has it.
    confirmed_at   timestamptz,
    -- Highest time step accepted so far; codes at or before it are refused (no replay).
    last_used_step bigint NOT NULL DEFAULT 0,
    setup_attempts integer NOT NULL DEFAULT 0,
    created_at     timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE recovery_codes (
    id         uuid PRIMARY KEY,
    account_id uuid NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    -- SHA-256 of account id and the normalized code; the code itself is shown once.
    code_hash  bytea NOT NULL UNIQUE,
    used_at    timestamptz,
    created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX recovery_codes_account_idx ON recovery_codes (account_id) WHERE used_at IS NULL;

CREATE TABLE mfa_challenges (
    id         uuid PRIMARY KEY,
    account_id uuid NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    -- SHA-256 of the opaque token handed to the browser after a correct password.
    token_hash bytea NOT NULL UNIQUE,
    attempts   integer NOT NULL DEFAULT 0,
    expires_at timestamptz NOT NULL,
    used_at    timestamptz,
    created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX mfa_challenges_expiry_idx ON mfa_challenges (expires_at);

ALTER TABLE login_events DROP CONSTRAINT login_events_outcome_check;
ALTER TABLE login_events ADD CONSTRAINT login_events_outcome_check
    CHECK (outcome IN ('success', 'bad_credentials', 'locked', 'suspended', 'mfa_required', 'mfa_failed'));
