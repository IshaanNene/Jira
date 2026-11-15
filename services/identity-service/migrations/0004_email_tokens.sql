-- Single-use links sent by email: verifying an address and resetting a forgotten password.
-- Only a hash of each token is stored; the token itself exists only in the email.

CREATE TABLE email_tokens (
    id         uuid PRIMARY KEY,
    account_id uuid NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    purpose    text NOT NULL CHECK (purpose IN ('verify_email', 'reset_password')),
    -- SHA-256 of a 256-bit random token.
    token_hash bytea NOT NULL UNIQUE,
    -- The address it was sent to: the link stops working if the account's address changes.
    email      citext NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    expires_at timestamptz NOT NULL,
    used_at    timestamptz,
    CHECK (expires_at > created_at)
);
-- Rate limiting counts recent links per account and purpose.
CREATE INDEX email_tokens_account_idx ON email_tokens (account_id, purpose, created_at DESC);
