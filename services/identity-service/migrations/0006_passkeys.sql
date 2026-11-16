-- Passkeys (WebAuthn discoverable credentials): sign in without a password, with the
-- device's PIN or biometric standing in for the second step.

CREATE TABLE passkeys (
    id              uuid PRIMARY KEY,
    account_id      uuid NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    -- Chosen by the authenticator; globally unique, and how a sign-in finds the passkey.
    credential_id   bytea NOT NULL UNIQUE CHECK (length(credential_id) BETWEEN 1 AND 1023),
    -- COSE public key as the authenticator sent it. Public, so not sealed.
    public_key      bytea NOT NULL,
    algorithm       integer NOT NULL,
    sign_count      bigint NOT NULL DEFAULT 0,
    name            text NOT NULL CHECK (length(name) BETWEEN 1 AND 60),
    backup_eligible boolean NOT NULL,
    backed_up       boolean NOT NULL,
    created_at      timestamptz NOT NULL DEFAULT now(),
    last_used_at    timestamptz
);
CREATE INDEX passkeys_account_idx ON passkeys (account_id);

-- One row per ceremony. Only the SHA-256 of the challenge is kept; the response carries
-- the challenge back, which is how it is found again. Single use, five minutes.
CREATE TABLE webauthn_challenges (
    id             uuid PRIMARY KEY,
    challenge_hash bytea NOT NULL UNIQUE,
    purpose        text NOT NULL CHECK (purpose IN ('register', 'sign_in')),
    -- The account adding a passkey; NULL for a sign-in, where nobody is known yet.
    account_id     uuid REFERENCES accounts (id) ON DELETE CASCADE,
    expires_at     timestamptz NOT NULL,
    used_at        timestamptz,
    created_at     timestamptz NOT NULL DEFAULT now(),
    CHECK ((purpose = 'register') = (account_id IS NOT NULL))
);
CREATE INDEX webauthn_challenges_expiry_idx ON webauthn_challenges (expires_at);

-- How each session was signed in to: a passkey satisfies a workspace's two-step requirement.
ALTER TABLE sessions ADD COLUMN auth_method text NOT NULL DEFAULT 'password'
    CHECK (auth_method IN ('password', 'two_step', 'passkey'));

ALTER TABLE login_events DROP CONSTRAINT login_events_outcome_check;
ALTER TABLE login_events ADD CONSTRAINT login_events_outcome_check
    CHECK (outcome IN ('success', 'bad_credentials', 'locked', 'suspended', 'mfa_required', 'mfa_failed', 'passkey_failed'));
