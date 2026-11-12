-- Identity service. Accounts are global (one person, many organizations), so these
-- tables are not tenant-owned. Tenant-scoped identity configuration (SSO providers,
-- API tokens) arrives with its own RLS-protected tables in Phase 6.

CREATE TABLE accounts (
    id                 uuid PRIMARY KEY,
    email              citext NOT NULL UNIQUE,
    display_name       text NOT NULL,
    -- Argon2id PHC string.
    password_hash      text NOT NULL,
    email_verified_at  timestamptz,
    status             text NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'suspended')),
    failed_login_count integer NOT NULL DEFAULT 0,
    locked_until       timestamptz,
    created_at         timestamptz NOT NULL DEFAULT now(),
    updated_at         timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE sessions (
    id             uuid PRIMARY KEY,
    account_id     uuid NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    created_at     timestamptz NOT NULL DEFAULT now(),
    last_seen_at   timestamptz NOT NULL DEFAULT now(),
    expires_at     timestamptz NOT NULL,
    revoked_at     timestamptz,
    revoked_reason text,
    ip             text,
    user_agent     text
);
CREATE INDEX sessions_account_active_idx ON sessions (account_id, last_seen_at DESC) WHERE revoked_at IS NULL;

CREATE TABLE refresh_tokens (
    id         uuid PRIMARY KEY,
    session_id uuid NOT NULL REFERENCES sessions (id) ON DELETE CASCADE,
    -- SHA-256 of the opaque token; the token itself is never stored.
    token_hash bytea NOT NULL UNIQUE,
    created_at timestamptz NOT NULL DEFAULT now(),
    used_at    timestamptz,
    expires_at timestamptz NOT NULL
);
CREATE INDEX refresh_tokens_session_idx ON refresh_tokens (session_id);

CREATE TABLE signing_keys (
    kid             text PRIMARY KEY,
    algorithm       text NOT NULL CHECK (algorithm = 'EdDSA'),
    public_x        text NOT NULL,
    -- PKCS#8 private key sealed with AES-256-GCM under the key-encryption key.
    private_key_enc bytea NOT NULL,
    nonce           bytea NOT NULL,
    status          text NOT NULL CHECK (status IN ('active', 'retiring', 'retired')),
    created_at      timestamptz NOT NULL DEFAULT now()
);
-- Exactly one key signs at a time; retiring keys stay in the JWKS until their tokens expire.
CREATE UNIQUE INDEX signing_keys_single_active_idx ON signing_keys ((true)) WHERE status = 'active';

CREATE TABLE login_events (
    id         uuid PRIMARY KEY,
    account_id uuid REFERENCES accounts (id) ON DELETE CASCADE,
    -- SHA-256 of the lower-cased email: lets failed attempts on unknown addresses be
    -- correlated without storing addresses that are not accounts.
    email_hash bytea NOT NULL,
    outcome    text NOT NULL CHECK (outcome IN ('success', 'bad_credentials', 'locked', 'suspended')),
    ip         text,
    user_agent text,
    created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX login_events_account_idx ON login_events (account_id, created_at DESC);

CREATE TABLE outbox (
    id            uuid PRIMARY KEY,
    tenant_id     uuid,
    topic         text NOT NULL,
    partition_key text NOT NULL,
    event_type    text NOT NULL,
    payload       jsonb NOT NULL,
    created_at    timestamptz NOT NULL DEFAULT now(),
    published_at  timestamptz,
    attempts      integer NOT NULL DEFAULT 0,
    last_error    text
);
CREATE INDEX outbox_unpublished_idx ON outbox (created_at) WHERE published_at IS NULL;

CREATE TABLE inbox (
    consumer     text NOT NULL,
    event_id     uuid NOT NULL,
    processed_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (consumer, event_id)
);
