-- Single sign-on (OpenID Connect): one identity provider per workspace, links between a
-- provider's subjects and Meridian accounts, and the short-lived state of each sign-in.

CREATE TABLE sso_connections (
    organization_id    uuid PRIMARY KEY,
    tenant_id          uuid NOT NULL,
    -- What people type to find their workspace's sign-in: first come, first served.
    sign_in_name       text NOT NULL CHECK (sign_in_name ~ '^[a-z0-9]([a-z0-9-]{0,38}[a-z0-9])?$'),
    issuer             text NOT NULL,
    client_id          text NOT NULL CHECK (length(client_id) BETWEEN 1 AND 255),
    -- AES-256-GCM under the key-encryption key, bound to the organization id.
    client_secret_enc  bytea NOT NULL,
    client_secret_nonce bytea NOT NULL,
    enabled            boolean NOT NULL DEFAULT true,
    -- Members (other than owners) may enter the workspace only from an SSO sign-in.
    require_sso        boolean NOT NULL DEFAULT false,
    created_by         uuid NOT NULL,
    created_at         timestamptz NOT NULL DEFAULT now(),
    updated_at         timestamptz NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX sso_connections_name_idx ON sso_connections (sign_in_name);

-- A person linked their account to their identity at the workspace's provider. Created only
-- by the account's owner while signed in, never from what a provider asserts.
CREATE TABLE sso_identities (
    id              uuid PRIMARY KEY,
    organization_id uuid NOT NULL REFERENCES sso_connections (organization_id) ON DELETE CASCADE,
    issuer          text NOT NULL,
    subject         text NOT NULL,
    account_id      uuid NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    -- The address the provider reported when the link was made, for display only.
    email           text,
    created_at      timestamptz NOT NULL DEFAULT now(),
    last_used_at    timestamptz,
    UNIQUE (organization_id, issuer, subject),
    UNIQUE (organization_id, account_id)
);
CREATE INDEX sso_identities_account_idx ON sso_identities (account_id);

CREATE TABLE sso_states (
    id              uuid PRIMARY KEY,
    -- SHA-256 of the `state` parameter, and of the cookie that ties the flow to one browser.
    state_hash      bytea NOT NULL UNIQUE,
    browser_hash    bytea NOT NULL,
    organization_id uuid NOT NULL,
    purpose         text NOT NULL CHECK (purpose IN ('sign_in', 'link')),
    account_id      uuid REFERENCES accounts (id) ON DELETE CASCADE,
    nonce           text NOT NULL,
    pkce_verifier   text NOT NULL,
    expires_at      timestamptz NOT NULL,
    used_at         timestamptz,
    created_at      timestamptz NOT NULL DEFAULT now(),
    CHECK ((purpose = 'link') = (account_id IS NOT NULL))
);
CREATE INDEX sso_states_expiry_idx ON sso_states (expires_at);

ALTER TABLE sessions DROP CONSTRAINT sessions_auth_method_check;
ALTER TABLE sessions ADD CONSTRAINT sessions_auth_method_check CHECK (auth_method IN ('password', 'two_step', 'passkey', 'sso'));
-- An SSO session can enter only the workspace whose provider signed it in.
ALTER TABLE sessions ADD COLUMN sso_organization_id uuid;
ALTER TABLE sessions ADD CONSTRAINT sessions_sso_org_check CHECK ((auth_method = 'sso') = (sso_organization_id IS NOT NULL));

ALTER TABLE login_events DROP CONSTRAINT login_events_outcome_check;
ALTER TABLE login_events ADD CONSTRAINT login_events_outcome_check
    CHECK (outcome IN ('success', 'bad_credentials', 'locked', 'suspended', 'mfa_required', 'mfa_failed', 'passkey_failed', 'sso_failed'));
