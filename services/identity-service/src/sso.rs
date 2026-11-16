//! Single sign-on with OpenID Connect: the protocol half (discovery, the authorization URL,
//! the code exchange and ID token checks), done by the `openidconnect` crate. Storage, links
//! to accounts and sessions are in `http::sso`.
//!
//! The provider's address is chosen by a workspace admin, so every request to it goes through
//! a client that resolves names itself and refuses private, loopback, link-local and other
//! reserved addresses (the same rule as outgoing webhooks), never follows redirects, and gives
//! up after 10 seconds. The URLs a provider advertises in its discovery document are checked
//! the same way before they are used.

use std::{net::SocketAddr, sync::Arc, time::Duration};

use openidconnect::{
    core::{CoreAuthenticationFlow, CoreClient, CoreJwsSigningAlgorithm, CoreProviderMetadata},
    reqwest::{
        self,
        dns::{Addrs, Name, Resolve, Resolving},
        redirect::Policy,
        Url,
    },
    AuthorizationCode, ClientId, ClientSecret, CsrfToken, IssuerUrl, Nonce, PkceCodeChallenge, PkceCodeVerifier, RedirectUrl,
    Scope, TokenResponse,
};
use platform_common::net::is_public;

/// Which providers may be reached. Production: https on public addresses only. The two
/// relaxations are for local development and tests (`SSO_ALLOW_HTTP`,
/// `SSO_ALLOW_PRIVATE_NETWORKS`) and are off unless configured.
#[derive(Debug, Clone, Copy, Default)]
pub struct Reach {
    pub allow_http: bool,
    pub allow_private: bool,
}

impl Reach {
    pub fn from_env() -> Self {
        let flag = |name| platform_common::config::optional(name).is_some_and(|v| v == "true");
        Self { allow_http: flag("SSO_ALLOW_HTTP"), allow_private: flag("SSO_ALLOW_PRIVATE_NETWORKS") }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    /// The issuer or an advertised endpoint is not allowed (scheme or address).
    Unreachable(String),
    /// Discovery failed or the provider's metadata is unusable.
    Discovery(String),
    /// The code exchange failed or the ID token did not check out.
    Rejected(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Unreachable(m) | Error::Discovery(m) | Error::Rejected(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for Error {}

/// Check a provider URL: https (unless allowed), no credentials, and no literal private
/// address (names are checked when they are resolved).
pub fn check_url(raw: &str, reach: Reach) -> Result<Url, Error> {
    let url =
        Url::parse(raw.trim()).map_err(|_| Error::Unreachable("Enter the issuer's full URL, starting with https://.".into()))?;
    match url.scheme() {
        "https" => {}
        "http" if reach.allow_http => {}
        _ => return Err(Error::Unreachable("The identity provider must use https://.".into())),
    }
    if !url.username().is_empty() || url.password().is_some() || url.fragment().is_some() || url.query().is_some() {
        return Err(Error::Unreachable("The issuer URL cannot contain credentials, a query or a #fragment.".into()));
    }
    let host = url.host_str().ok_or_else(|| Error::Unreachable("The issuer URL needs a host.".into()))?;
    if !reach.allow_private {
        let internal_name = host.eq_ignore_ascii_case("localhost")
            || host.ends_with(".localhost")
            || host.ends_with(".internal")
            || host.ends_with(".local");
        let literal = host.trim_start_matches('[').trim_end_matches(']').parse().ok();
        if internal_name || literal.is_some_and(|ip| !is_public(ip)) {
            return Err(Error::Unreachable("The identity provider must be on a public address.".into()));
        }
    }
    Ok(url)
}

/// Resolves names for the provider client, refusing the whole name if any address is not
/// public (so one answer cannot mix a public and a private address).
struct PublicOnly;

impl Resolve for PublicOnly {
    fn resolve(&self, name: Name) -> Resolving {
        Box::pin(async move {
            let addrs: Vec<SocketAddr> = tokio::net::lookup_host((name.as_str(), 0)).await?.collect();
            if addrs.is_empty() || addrs.iter().any(|a| !is_public(a.ip())) {
                return Err("the identity provider resolves to a private or reserved address".into());
            }
            Ok(Box::new(addrs.into_iter()) as Addrs)
        })
    }
}

/// Talks to identity providers.
pub struct Provider {
    http: reqwest::Client,
    pub reach: Reach,
}

impl Provider {
    pub fn new(reach: Reach) -> anyhow::Result<Self> {
        let mut builder = reqwest::Client::builder()
            .redirect(Policy::none())
            .timeout(Duration::from_secs(10))
            .connect_timeout(Duration::from_secs(5))
            .user_agent("Meridian-SSO/1");
        if !reach.allow_private {
            builder = builder.dns_resolver(Arc::new(PublicOnly));
        }
        Ok(Self { http: builder.build()?, reach })
    }

    /// Fetch and check the provider's discovery document.
    pub async fn discover(&self, issuer: &str) -> Result<CoreProviderMetadata, Error> {
        check_url(issuer, self.reach)?;
        let issuer_url = IssuerUrl::new(issuer.trim().to_owned()).map_err(|e| Error::Unreachable(e.to_string()))?;
        let metadata = CoreProviderMetadata::discover_async(issuer_url, &self.http).await.map_err(|e| {
            Error::Discovery(format!("Couldn't read the provider's OpenID configuration ({}).", short(&e.to_string())))
        })?;
        // Everything we will call must pass the same checks as the issuer.
        check_url(metadata.jwks_uri().as_str(), self.reach)?;
        // The browser, not this service, visits the authorization endpoint: only its scheme matters.
        let authorize = metadata.authorization_endpoint().url().scheme();
        if !(authorize == "https" || (authorize == "http" && self.reach.allow_http)) {
            return Err(Error::Unreachable("The provider's sign-in page must use https://.".into()));
        }
        let token = metadata.token_endpoint().ok_or_else(|| Error::Discovery("The provider has no token endpoint.".into()))?;
        check_url(token.as_str(), self.reach)?;
        Ok(metadata)
    }

    /// Where to send the browser, with a fresh state, nonce and PKCE verifier to remember.
    pub fn authorization(
        &self,
        metadata: CoreProviderMetadata,
        client_id: &str,
        redirect_uri: &str,
    ) -> Result<Authorization, Error> {
        let client = CoreClient::from_provider_metadata(metadata, ClientId::new(client_id.to_owned()), None)
            .set_redirect_uri(RedirectUrl::new(redirect_uri.to_owned()).map_err(|e| Error::Discovery(e.to_string()))?);
        let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
        let (url, state, nonce) = client
            .authorize_url(CoreAuthenticationFlow::AuthorizationCode, CsrfToken::new_random, Nonce::new_random)
            .add_scope(Scope::new("email".into()))
            .add_scope(Scope::new("profile".into()))
            .set_pkce_challenge(challenge)
            .url();
        Ok(Authorization {
            url: url.to_string(),
            state: state.secret().clone(),
            nonce: nonce.secret().clone(),
            pkce_verifier: verifier.secret().clone(),
        })
    }

    /// Exchange the code and check the ID token: signature against the provider's keys,
    /// issuer, audience, expiry, and the nonce of this sign-in.
    pub async fn complete(
        &self,
        metadata: CoreProviderMetadata,
        client_id: &str,
        client_secret: &str,
        redirect_uri: &str,
        answer: &Answer<'_>,
    ) -> Result<Identity, Error> {
        let client = CoreClient::from_provider_metadata(
            metadata,
            ClientId::new(client_id.to_owned()),
            Some(ClientSecret::new(client_secret.to_owned())),
        )
        .set_redirect_uri(RedirectUrl::new(redirect_uri.to_owned()).map_err(|e| Error::Discovery(e.to_string()))?);
        let token = client
            .exchange_code(AuthorizationCode::new(answer.code.to_owned()))
            .map_err(|e| Error::Discovery(e.to_string()))?
            .set_pkce_verifier(PkceCodeVerifier::new(answer.pkce_verifier.to_owned()))
            .request_async(&self.http)
            .await
            .map_err(|e| Error::Rejected(format!("token exchange failed: {}", short(&e.to_string()))))?;
        let id_token = token.id_token().ok_or_else(|| Error::Rejected("no ID token in the response".into()))?;
        // Asymmetric algorithms only: an HMAC-signed token would be keyed with the client
        // secret, which is not a statement from the provider's own keys.
        let verifier = client.id_token_verifier().set_allowed_algs([
            CoreJwsSigningAlgorithm::RsaSsaPkcs1V15Sha256,
            CoreJwsSigningAlgorithm::RsaSsaPkcs1V15Sha384,
            CoreJwsSigningAlgorithm::RsaSsaPkcs1V15Sha512,
            CoreJwsSigningAlgorithm::RsaSsaPssSha256,
            CoreJwsSigningAlgorithm::EcdsaP256Sha256,
            CoreJwsSigningAlgorithm::EcdsaP384Sha384,
            CoreJwsSigningAlgorithm::EdDsa,
        ]);
        let claims = id_token
            .claims(&verifier, &Nonce::new(answer.nonce.to_owned()))
            .map_err(|e| Error::Rejected(format!("ID token rejected: {}", short(&e.to_string()))))?;
        Ok(Identity {
            issuer: claims.issuer().as_str().to_owned(),
            subject: claims.subject().as_str().to_owned(),
            email: claims.email().map(|e| e.as_str().chars().take(320).collect()),
        })
    }
}

fn short(message: &str) -> String {
    message.chars().take(200).collect()
}

/// The provider's answer, with what was remembered when the sign-in started.
pub struct Answer<'a> {
    pub code: &'a str,
    pub nonce: &'a str,
    pub pkce_verifier: &'a str,
}

pub struct Authorization {
    pub url: String,
    pub state: String,
    pub nonce: String,
    pub pkce_verifier: String,
}

/// Who the provider says signed in.
#[derive(Debug)]
pub struct Identity {
    pub issuer: String,
    pub subject: String,
    pub email: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_urls_must_be_public_https() {
        let prod = Reach::default();
        assert!(check_url("https://login.example.com/tenant", prod).is_ok());
        for bad in [
            "http://login.example.com",
            "https://localhost/x",
            "https://10.0.0.5",
            "https://169.254.169.254/latest",
            "https://[::1]/",
            "https://user:pw@login.example.com",
            "https://login.example.com/?x=1",
            "https://metadata.internal",
            "not a url",
        ] {
            assert!(check_url(bad, prod).is_err(), "{bad}");
        }
        let dev = Reach { allow_http: true, allow_private: true };
        assert!(check_url("http://127.0.0.1:9000", dev).is_ok());
    }
}
