//! A SAML identity provider for tests: a key and certificate, its metadata, and responses
//! signed the way real providers sign them (RSA-SHA256, SHA-256 digests).

use std::io::Read;

use base64::{engine::general_purpose::STANDARD, Engine};
use samael::{
    crypto::{CertificateDer, Crypto, CryptoProvider},
    idp::{
        response_builder::{build_response_template, ResponseAttribute},
        sp_extractor::RequiredAttribute,
        CertificateParams, IdentityProvider, KeyType, Rsa,
    },
    schema::SubjectConfirmationData,
    signature::DigestAlgorithm,
    traits::ToXml,
};

pub const ENTITY_ID: &str = "https://idp.corp.example/saml";
pub const SSO_URL: &str = "https://idp.corp.example/saml/sso";

pub struct FakeSamlIdp {
    key: IdentityProvider,
    cert: CertificateDer,
}

/// What a sign-in request carried to the provider.
pub struct Received {
    pub request_id: String,
    pub relay_state: String,
}

impl FakeSamlIdp {
    pub fn new() -> Self {
        let key = IdentityProvider::generate_new(KeyType::Rsa(Rsa::Rsa2048)).unwrap();
        let cert = key
            .create_certificate(&CertificateParams { common_name: "idp", issuer_name: "idp", days_until_expiration: 30 })
            .unwrap();
        Self { key, cert }
    }

    /// Metadata as an admin would paste it; `want_signed` marks it as requiring signed requests.
    pub fn metadata_with(&self, with_cert: bool, want_signed: bool) -> String {
        let cert = STANDARD.encode(self.cert.der_data());
        let key = if with_cert {
            format!(
                r#"<md:KeyDescriptor use="signing"><ds:KeyInfo><ds:X509Data><ds:X509Certificate>{cert}</ds:X509Certificate></ds:X509Data></ds:KeyInfo></md:KeyDescriptor>"#
            )
        } else {
            String::new()
        };
        format!(
            r#"<?xml version="1.0"?>
<md:EntityDescriptor xmlns:md="urn:oasis:names:tc:SAML:2.0:metadata" xmlns:ds="http://www.w3.org/2000/09/xmldsig#" entityID="{ENTITY_ID}">
  <md:IDPSSODescriptor WantAuthnRequestsSigned="{want_signed}" protocolSupportEnumeration="urn:oasis:names:tc:SAML:2.0:protocol">
    {key}
    <md:NameIDFormat>urn:oasis:names:tc:SAML:1.1:nameid-format:unspecified</md:NameIDFormat>
    <md:SingleSignOnService Binding="urn:oasis:names:tc:SAML:2.0:bindings:HTTP-Redirect" Location="{SSO_URL}"/>
  </md:IDPSSODescriptor>
</md:EntityDescriptor>"#
        )
    }

    pub fn metadata(&self) -> String {
        self.metadata_with(true, false)
    }

    /// Read the sign-in request the browser was sent to the provider with.
    pub fn receive(url: &str) -> Received {
        let url = reqwest::Url::parse(url).unwrap();
        assert!(url.as_str().starts_with(SSO_URL), "{url}");
        let param = |name: &str| url.query_pairs().find(|(k, _)| k == name).map(|(_, v)| v.into_owned()).unwrap();
        let mut xml = String::new();
        flate2::read::DeflateDecoder::new(STANDARD.decode(param("SAMLRequest")).unwrap().as_slice())
            .read_to_string(&mut xml)
            .unwrap();
        let request_id = xml.split(" ID=\"").nth(1).unwrap().split('"').next().unwrap().to_owned();
        assert!(xml.contains("AuthnRequest"), "{xml}");
        Received { request_id, relay_state: param("RelayState") }
    }

    /// A signed, base64 `SAMLResponse` for `audience` (the workspace's entity ID).
    pub fn respond(&self, subject: &str, email: &str, audience: &str, acs_url: &str, in_response_to: &str) -> String {
        let attributes =
            [ResponseAttribute { required_attribute: RequiredAttribute { name: "email".into(), format: None }, value: email }];
        let mut response =
            build_response_template(&self.cert, subject, audience, ENTITY_ID, acs_url, in_response_to, &attributes);
        // What real providers add, and samael requires: who the assertion is for, and until when.
        let assertion = response.assertion.as_mut().unwrap();
        for confirmation in assertion.subject.as_mut().unwrap().subject_confirmations.iter_mut().flatten() {
            confirmation.subject_confirmation_data = Some(SubjectConfirmationData {
                not_before: None,
                not_on_or_after: Some(chrono::Utc::now() + chrono::Duration::minutes(5)),
                recipient: Some(acs_url.to_owned()),
                in_response_to: Some(in_response_to.to_owned()),
                address: None,
                content: None,
            });
        }
        for reference in &mut response.signature.as_mut().unwrap().signed_info.reference {
            reference.digest_method.algorithm = DigestAlgorithm::Sha256;
        }
        let signed = Crypto::sign_xml(response.to_string().unwrap(), &self.key.export_private_key_der().unwrap()).unwrap();
        STANDARD.encode(signed)
    }
}
