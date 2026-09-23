//! MQTT credentials for Xiaomi Home central gateways.
//!
//! Xiaomi's central gateway uses mutual TLS. This module creates the Ed25519
//! private key and CSR locally, then asks the authenticated Xiaomi Home API to
//! sign that CSR. It intentionally does not persist the returned credentials;
//! callers should store the private key and certificate using their platform's
//! secret storage.

mod mips;

pub use mips::{MipsAction, MipsClient, MipsClientConfig, MipsEvent, MipsProperty, MipsTlsConfig};

use rcgen::{CertificateParams, DnType, KeyPair, PKCS_ED25519};
use sha1::{Digest, Sha1};

use crate::{MiHomeApiClient, MiotError};

/// PEM credentials used by a central-gateway MQTT connection.
pub struct MqttClientCertificate {
    private_key_pem: String,
    certificate_pem: String,
}

impl MqttClientCertificate {
    /// Returns the PEM-encoded Ed25519 private key.
    #[must_use]
    pub fn private_key_pem(&self) -> &str {
        &self.private_key_pem
    }

    /// Returns the PEM-encoded client certificate signed by Xiaomi Home.
    #[must_use]
    pub fn certificate_pem(&self) -> &str {
        &self.certificate_pem
    }
}

/// Requests mutual-TLS client certificates for Xiaomi Home central gateways.
#[derive(Clone, Debug)]
pub struct MqttCertificateClient {
    api: MiHomeApiClient,
}

impl MqttCertificateClient {
    /// Creates a certificate client from an OAuth-authenticated Xiaomi Home API client.
    #[must_use]
    pub fn new(api: MiHomeApiClient) -> Self {
        Self { api }
    }

    /// Generates an Ed25519 key pair and requests a certificate bound to `uid` and `virtual_did`.
    ///
    /// The API currently supports this central-gateway certificate flow in the
    /// China region only.
    ///
    /// # Errors
    ///
    /// Returns an error when either identifier is empty, the CSR cannot be
    /// generated, or Xiaomi Home rejects the certificate request.
    pub async fn request_client_certificate(
        &self,
        uid: &str,
        virtual_did: &str,
    ) -> Result<MqttClientCertificate, MiotError> {
        let (private_key_pem, csr_pem) = build_certificate_request(uid, virtual_did)?;
        let certificate_pem = self.api.get_central_certificate(&csr_pem).await?;
        Ok(MqttClientCertificate {
            private_key_pem,
            certificate_pem,
        })
    }
}

fn build_certificate_request(uid: &str, virtual_did: &str) -> Result<(String, String), MiotError> {
    if uid.trim().is_empty() {
        return Err(MiotError::InvalidInput("uid must not be empty"));
    }
    if virtual_did.trim().is_empty() {
        return Err(MiotError::InvalidInput("virtual DID must not be empty"));
    }

    let did_hash = hex::encode(Sha1::digest(virtual_did.as_bytes()));
    let key_pair = KeyPair::generate_for(&PKCS_ED25519)
        .map_err(|error| MiotError::Certificate(error.to_string()))?;
    let mut params = CertificateParams::new(Vec::new())
        .map_err(|error| MiotError::Certificate(error.to_string()))?;
    params.distinguished_name.push(DnType::CountryName, "CN");
    params
        .distinguished_name
        .push(DnType::OrganizationName, "Mijia Device");
    params
        .distinguished_name
        .push(DnType::CommonName, format!("mips.{uid}.{did_hash}.2"));
    let csr_pem = params
        .serialize_request(&key_pair)
        .and_then(|request| request.pem())
        .map_err(|error| MiotError::Certificate(error.to_string()))?;
    Ok((key_pair.serialize_pem(), csr_pem))
}

#[cfg(test)]
mod tests {
    use super::build_certificate_request;

    #[test]
    fn creates_an_ed25519_key_and_pem_csr() {
        let (key, csr) = build_certificate_request("12345", "virtual-did").unwrap();
        assert!(key.contains("BEGIN PRIVATE KEY"));
        assert!(csr.contains("BEGIN CERTIFICATE REQUEST"));
    }

    #[test]
    fn rejects_empty_certificate_subject_parts() {
        assert!(build_certificate_request("", "virtual-did").is_err());
        assert!(build_certificate_request("12345", "").is_err());
    }
}
