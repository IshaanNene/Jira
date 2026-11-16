-- Single sign-on with SAML 2.0, beside OpenID Connect. A workspace's connection uses one of the
-- two. For SAML, `issuer` holds the identity provider's entity ID (so links stay keyed by
-- (workspace, issuer, subject)), and the admin's pasted metadata is kept: its certificates are
-- what signatures are checked against. Nothing secret is stored for SAML.

ALTER TABLE sso_connections ADD COLUMN protocol text NOT NULL DEFAULT 'oidc' CHECK (protocol IN ('oidc', 'saml'));
ALTER TABLE sso_connections ADD COLUMN saml_metadata text CHECK (length(saml_metadata) <= 200000);
ALTER TABLE sso_connections ADD COLUMN saml_sso_url text;
ALTER TABLE sso_connections ALTER COLUMN client_id DROP NOT NULL;
ALTER TABLE sso_connections ALTER COLUMN client_secret_enc DROP NOT NULL;
ALTER TABLE sso_connections ALTER COLUMN client_secret_nonce DROP NOT NULL;
ALTER TABLE sso_connections ADD CONSTRAINT sso_connections_protocol_fields CHECK (
    CASE protocol
        WHEN 'oidc' THEN client_id IS NOT NULL AND client_secret_enc IS NOT NULL AND client_secret_nonce IS NOT NULL
                         AND saml_metadata IS NULL AND saml_sso_url IS NULL
        ELSE saml_metadata IS NOT NULL AND saml_sso_url IS NOT NULL
                         AND client_id IS NULL AND client_secret_enc IS NULL AND client_secret_nonce IS NULL
    END);

-- For SAML, `nonce` holds the AuthnRequest ID the response must answer, there is no PKCE, and
-- the provider's POST to the assertion consumer service is kept here until the same browser
-- comes back with its cookie (a cross-site POST does not carry a SameSite=Lax cookie).
ALTER TABLE sso_states ALTER COLUMN pkce_verifier DROP NOT NULL;
ALTER TABLE sso_states ADD COLUMN protocol text NOT NULL DEFAULT 'oidc' CHECK (protocol IN ('oidc', 'saml'));
ALTER TABLE sso_states ADD COLUMN saml_response text CHECK (length(saml_response) <= 300000);
ALTER TABLE sso_states ADD CONSTRAINT sso_states_protocol_fields CHECK ((protocol = 'oidc') = (pkce_verifier IS NOT NULL));
