//! A software passkey for tests: makes the same bytes a browser and a platform
//! authenticator would (P-256, attestation `none`), so the server's checks run for real.
//! Shared by the unit tests in `src/webauthn.rs` and the HTTP tests.
#![allow(dead_code)]

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use ring::{
    rand::SystemRandom,
    signature::{EcdsaKeyPair, KeyPair, ECDSA_P256_SHA256_ASN1_SIGNING},
};
use sha2::{Digest, Sha256};

pub struct SoftAuthenticator {
    key: EcdsaKeyPair,
    rng: SystemRandom,
    rp_id: String,
    pub credential_id: Vec<u8>,
    pub user_handle: Vec<u8>,
    pub sign_count: u32,
    /// UP | UV | BE | BS by default.
    pub flags: u8,
    pub cross_origin: bool,
}

/// Minimal CBOR encoding for what an authenticator sends.
pub enum Cbor {
    Int(i64),
    Bytes(Vec<u8>),
    Text(String),
    Map(Vec<(Cbor, Cbor)>),
}

fn head(out: &mut Vec<u8>, major: u8, n: u64) {
    let m = major << 5;
    match n {
        0..=23 => out.push(m | n as u8),
        24..=0xff => out.extend([m | 24, n as u8]),
        0x100..=0xffff => {
            out.push(m | 25);
            out.extend((n as u16).to_be_bytes());
        }
        _ => {
            out.push(m | 26);
            out.extend((n as u32).to_be_bytes());
        }
    }
}

impl Cbor {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.write(&mut out);
        out
    }

    fn write(&self, out: &mut Vec<u8>) {
        match self {
            Cbor::Int(i) if *i >= 0 => head(out, 0, *i as u64),
            Cbor::Int(i) => head(out, 1, (-1 - *i) as u64),
            Cbor::Bytes(b) => {
                head(out, 2, b.len() as u64);
                out.extend(b);
            }
            Cbor::Text(t) => {
                head(out, 3, t.len() as u64);
                out.extend(t.as_bytes());
            }
            Cbor::Map(entries) => {
                head(out, 5, entries.len() as u64);
                for (k, v) in entries {
                    k.write(out);
                    v.write(out);
                }
            }
        }
    }
}

fn sha256(bytes: &[u8]) -> Vec<u8> {
    Sha256::digest(bytes).to_vec()
}

impl SoftAuthenticator {
    pub fn new(rp_id: &str, user_handle: &[u8]) -> Self {
        let rng = SystemRandom::new();
        let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &rng).unwrap();
        let key = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, pkcs8.as_ref(), &rng).unwrap();
        let mut credential_id = vec![0_u8; 32];
        ring::rand::SecureRandom::fill(&rng, &mut credential_id).unwrap();
        Self {
            key,
            rng,
            rp_id: rp_id.to_owned(),
            credential_id,
            user_handle: user_handle.to_vec(),
            sign_count: 0,
            flags: 0x01 | 0x04 | 0x08 | 0x10,
            cross_origin: false,
        }
    }

    pub fn cose_key(&self) -> Vec<u8> {
        let point = self.key.public_key().as_ref();
        Cbor::Map(vec![
            (Cbor::Int(1), Cbor::Int(2)),
            (Cbor::Int(3), Cbor::Int(-7)),
            (Cbor::Int(-1), Cbor::Int(1)),
            (Cbor::Int(-2), Cbor::Bytes(point[1..33].to_vec())),
            (Cbor::Int(-3), Cbor::Bytes(point[33..65].to_vec())),
        ])
        .encode()
    }

    fn client_data(&self, kind: &str, origin: &str, challenge: &[u8]) -> Vec<u8> {
        serde_json::json!({
            "type": kind,
            "challenge": URL_SAFE_NO_PAD.encode(challenge),
            "origin": origin,
            "crossOrigin": self.cross_origin,
        })
        .to_string()
        .into_bytes()
    }

    fn auth_data(&self, flags: u8, attested: bool) -> Vec<u8> {
        let mut data = sha256(self.rp_id.as_bytes());
        data.push(flags);
        data.extend(self.sign_count.to_be_bytes());
        if attested {
            data.extend([0_u8; 16]);
            data.extend((self.credential_id.len() as u16).to_be_bytes());
            data.extend(&self.credential_id);
            data.extend(self.cose_key());
        }
        data
    }

    /// `navigator.credentials.create()`: `(clientDataJSON, attestationObject)`.
    pub fn create(&mut self, origin: &str, challenge: &[u8]) -> (Vec<u8>, Vec<u8>) {
        let client = self.client_data("webauthn.create", origin, challenge);
        let attestation = Cbor::Map(vec![
            (Cbor::Text("fmt".into()), Cbor::Text("none".into())),
            (Cbor::Text("attStmt".into()), Cbor::Map(vec![])),
            (Cbor::Text("authData".into()), Cbor::Bytes(self.auth_data(self.flags | 0x40, true))),
        ])
        .encode();
        (client, attestation)
    }

    /// `navigator.credentials.get()`: `(clientDataJSON, authenticatorData, signature)`.
    /// Each call moves the signature counter on by one.
    pub fn get(&mut self, origin: &str, challenge: &[u8]) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
        self.sign_count += 1;
        let client = self.client_data("webauthn.get", origin, challenge);
        let data = self.auth_data(self.flags, false);
        let message = [data.as_slice(), &sha256(&client)].concat();
        let sig = self.key.sign(&self.rng, &message).unwrap().as_ref().to_vec();
        (client, data, sig)
    }
}
