//! Single sign-on with SAML 2.0, the protocol half, done by `samael` (signatures by xmlsec1).
//! Storage, links and sessions are shared with OpenID Connect in `http::sso`.
//!
//! * The identity provider is described by the metadata XML an admin pastes in. Nothing is
//!   fetched from the network, so there is no request forgery to guard against; the metadata
//!   must name a signing certificate and an HTTP-Redirect sign-in address.
//! * Each workspace is its own service provider: its entity ID (the audience every assertion
//!   must name) is `{app}/api/v1/auth/sso/saml/{organization id}`, so an assertion made for one
//!   workspace is refused by every other, even when both use the same provider.
//! * Only sign-ins this service started are accepted: the response must answer the request ID
//!   remembered with the sign-in (`InResponseTo`); unsolicited (IdP-initiated) responses are
//!   refused. The signature is checked with the metadata's certificates, the document is cut
//!   down to what the signature covers before anything is read from it (signature wrapping),
//!   only SHA-256 or stronger RSA / ECDSA signatures are accepted, and issuer, audience,
//!   recipient, destination and validity times are checked.
//! * Requests are not signed and encrypted assertions are not supported (no service-provider
//!   key is kept).

use std::{io::Write, str::FromStr};

use base64::{engine::general_purpose::STANDARD, Engine};
use flate2::{write::DeflateEncoder, Compression};
use samael::{
    crypto::AllowedSignatureAlgorithm,
    metadata::{EntityDescriptor, HTTP_REDIRECT_BINDING},
    schema::Assertion,
    service_provider::ServiceProvider,
    traits::ToXml,
};

use crate::sso::{check_url, Reach};

/// Largest metadata document accepted (providers' are a few kilobytes).
const MAX_METADATA: usize = 200_000;
/// Largest base64 `SAMLResponse` accepted.
pub const MAX_RESPONSE: usize = 300_000;

const ALGORITHMS: [AllowedSignatureAlgorithm; 6] = [
    AllowedSignatureAlgorithm::RsaSha256,
    AllowedSignatureAlgorithm::RsaSha384,
    AllowedSignatureAlgorithm::RsaSha512,
    AllowedSignatureAlgorithm::EcdsaSha256,
    AllowedSignatureAlgorithm::EcdsaSha384,
    AllowedSignatureAlgorithm::EcdsaSha512,
];

/// Attribute names providers use for the email address (Okta, Entra ID, Google, OneLogin).
const EMAIL_ATTRIBUTES: [&str; 5] = [
    "email",
    "mail",
    "emailaddress",
    "http://schemas.xmlsoap.org/ws/2005/05/identity/claims/emailaddress",
    "urn:oid:0.9.2342.19200300.100.1.3",
];

/// This workspace as a service provider.
pub struct Sp {
    pub entity_id: String,
    pub acs_url: String,
}

impl Sp {
    pub fn new(app_url: &str, organization_id: uuid::Uuid) -> Self {
        Self {
            entity_id: format!("{app_url}/api/v1/auth/sso/saml/{organization_id}"),
            acs_url: format!("{app_url}/api/v1/auth/sso/saml/acs"),
        }
    }

    fn provider(&self, idp: EntityDescriptor) -> ServiceProvider {
        ServiceProvider {
            entity_id: Some(self.entity_id.clone()),
            acs_url: Some(self.acs_url.clone()),
            metadata_url: Some(self.entity_id.clone()),
            slo_url: None,
            idp_metadata: idp,
            allow_idp_initiated: false,
            allowed_signature_algorithms: Some(ALGORITHMS.to_vec()),
            ..ServiceProvider::default()
        }
    }

    /// The service provider's own metadata, for the admin to give their identity provider.
    /// (Written out here: samael's builder insists on a logout URL, and there is none.)
    pub fn metadata(&self) -> String {
        let entity_id = xml_escape(&self.entity_id);
        let acs_url = xml_escape(&self.acs_url);
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<md:EntityDescriptor xmlns:md="urn:oasis:names:tc:SAML:2.0:metadata" entityID="{entity_id}">
  <md:SPSSODescriptor AuthnRequestsSigned="false" WantAssertionsSigned="true" protocolSupportEnumeration="urn:oasis:names:tc:SAML:2.0:protocol">
    <md:NameIDFormat>urn:oasis:names:tc:SAML:1.1:nameid-format:emailAddress</md:NameIDFormat>
    <md:NameIDFormat>urn:oasis:names:tc:SAML:2.0:nameid-format:persistent</md:NameIDFormat>
    <md:AssertionConsumerService Binding="urn:oasis:names:tc:SAML:2.0:bindings:HTTP-POST" Location="{acs_url}" index="0" isDefault="true"/>
  </md:SPSSODescriptor>
</md:EntityDescriptor>
"#
        )
    }
}

fn xml_escape(value: &str) -> String {
    value.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// What a workspace's identity provider metadata says, checked.
#[derive(Debug, Clone)]
pub struct IdpMetadata {
    pub entity_id: String,
    pub sso_url: String,
}

/// Read and check pasted identity provider metadata.
pub fn parse_metadata(xml: &str, reach: Reach) -> Result<IdpMetadata, String> {
    let xml = xml.trim();
    if xml.is_empty() || xml.len() > MAX_METADATA {
        return Err("Paste your identity provider's SAML metadata XML.".into());
    }
    let descriptor = EntityDescriptor::from_str(xml)
        .map_err(|_| "That isn't SAML metadata. Paste the XML document from your provider's SAML app.".to_string())?;
    let entity_id = descriptor.entity_id.clone().map(|e| e.trim().to_owned()).filter(|e| !e.is_empty() && e.len() <= 1024);
    let entity_id = entity_id.ok_or_else(|| "The metadata has no entityID.".to_string())?;
    if descriptor.idp_sso_descriptors.as_ref().is_none_or(Vec::is_empty) {
        return Err("The metadata doesn't describe an identity provider (no IDPSSODescriptor).".into());
    }
    if descriptor.idp_sso_descriptors.iter().flatten().any(|d| d.want_authn_requests_signed == Some(true)) {
        return Err(
            "The provider requires signed sign-in requests, which aren't supported yet. Turn that off in its SAML app.".into()
        );
    }
    let sp = Sp { entity_id: String::new(), acs_url: String::new() }.provider(descriptor);
    // Without a certificate samael would accept unsigned responses: refuse such metadata.
    match sp.idp_signing_certs() {
        Ok(Some(certs)) if !certs.is_empty() => {}
        _ => return Err("The metadata has no signing certificate.".into()),
    }
    let sso_url = sp
        .sso_binding_location(HTTP_REDIRECT_BINDING)
        .ok_or_else(|| "The metadata has no HTTP-Redirect sign-in address (SingleSignOnService).".to_string())?;
    // The browser, not this service, visits it; only the scheme and form matter.
    let url = reqwest::Url::parse(sso_url.trim()).map_err(|_| "The sign-in address in the metadata isn't a URL.".to_string())?;
    if !(url.scheme() == "https" || (url.scheme() == "http" && reach.allow_http)) || url.fragment().is_some() {
        return Err("The provider's sign-in address must use https://.".into());
    }
    // Same rule as OIDC for where the browser may be sent.
    if check_url(&format!("{}://{}", url.scheme(), url.host_str().unwrap_or_default()), reach).is_err() {
        return Err("The provider's sign-in address must be on a public address.".into());
    }
    Ok(IdpMetadata { entity_id, sso_url: url.to_string() })
}

/// A sign-in request: where to send the browser, and the request ID to expect back.
pub struct Request {
    pub url: String,
    pub request_id: String,
}

/// Build an HTTP-Redirect AuthnRequest. `relay_state` comes back with the response.
pub fn authn_request(sp: &Sp, idp: &IdpMetadata, metadata_xml: &str, relay_state: &str) -> Result<Request, String> {
    let descriptor = EntityDescriptor::from_str(metadata_xml).map_err(|e| e.to_string())?;
    let mut request = sp.provider(descriptor).make_authentication_request(&idp.sso_url).map_err(|e| e.to_string())?;
    // samael's own IDs carry 32 random bits; use 256 (base64url is a valid XML ID after "_").
    request.id = format!("_{}", crate::crypto::random_token());
    let xml = request.to_string().map_err(|e| format!("{e:?}"))?;
    let mut deflater = DeflateEncoder::new(Vec::new(), Compression::default());
    deflater.write_all(xml.as_bytes()).map_err(|e| e.to_string())?;
    let encoded = STANDARD.encode(deflater.finish().map_err(|e| e.to_string())?);
    let mut url = reqwest::Url::parse(&idp.sso_url).map_err(|e| e.to_string())?;
    url.query_pairs_mut().append_pair("SAMLRequest", &encoded).append_pair("RelayState", relay_state);
    Ok(Request { url: url.to_string(), request_id: request.id })
}

/// Who the provider says signed in.
#[derive(Debug)]
pub struct Identity {
    pub issuer: String,
    pub subject: String,
    pub email: Option<String>,
}

/// Check a `SAMLResponse` against the workspace's provider and the request it answers.
pub fn verify(sp: &Sp, metadata_xml: &str, saml_response: &str, request_id: &str) -> Result<Identity, String> {
    let encoded: String = saml_response.chars().filter(|c| !c.is_ascii_whitespace()).collect();
    if encoded.is_empty() || encoded.len() > MAX_RESPONSE {
        return Err("missing or oversized SAMLResponse".into());
    }
    let descriptor = EntityDescriptor::from_str(metadata_xml).map_err(|e| e.to_string())?;
    let issuer = descriptor.entity_id.clone().unwrap_or_default();
    let sp = sp.provider(descriptor);
    if !matches!(sp.idp_signing_certs(), Ok(Some(ref certs)) if !certs.is_empty()) {
        return Err("no signing certificate for the provider".into());
    }
    let assertion = sp.parse_base64_response(&encoded, Some(&[request_id])).map_err(|e| short(&e.to_string()))?;
    let subject = assertion
        .subject
        .as_ref()
        .and_then(|s| s.name_id.as_ref())
        .map(|n| n.value.trim().to_owned())
        .filter(|v| !v.is_empty() && v.len() <= 512 && !v.chars().any(char::is_control))
        .ok_or_else(|| "the assertion names no subject".to_string())?;
    let email = email_of(&assertion).or_else(|| subject.contains('@').then(|| subject.clone()));
    Ok(Identity { issuer, subject, email: email.map(|e| e.chars().take(320).collect()) })
}

fn email_of(assertion: &Assertion) -> Option<String> {
    assertion.attribute_statements.iter().flatten().flat_map(|s| s.attributes.iter()).find_map(|a| {
        let named = [a.name.as_deref(), a.friendly_name.as_deref()]
            .into_iter()
            .flatten()
            .any(|n| EMAIL_ATTRIBUTES.iter().any(|known| n.eq_ignore_ascii_case(known)));
        if named {
            a.values.iter().find_map(|v| v.value.clone()).map(|v| v.trim().to_owned()).filter(|v| v.contains('@'))
        } else {
            None
        }
    })
}

fn short(message: &str) -> String {
    message.chars().take(200).collect()
}
