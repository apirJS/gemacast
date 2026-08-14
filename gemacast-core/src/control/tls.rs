//! TLS helpers for Gemacast's self-signed, certificate-pinned control channel.

use std::sync::Arc;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::WebPkiSupportedAlgorithms;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};

use crate::control::device_auth::sha256_fingerprint;

#[derive(Debug)]
struct CertificateFingerprintVerifier {
    expected_fingerprint: Option<String>,
    signature_algorithms: WebPkiSupportedAlgorithms,
}

impl CertificateFingerprintVerifier {
    fn new(expected_fingerprint: Option<&str>) -> Result<Self, String> {
        let expected_fingerprint = expected_fingerprint
            .map(normalize_fingerprint)
            .transpose()?;
        Ok(Self {
            expected_fingerprint,
            signature_algorithms: rustls::crypto::ring::default_provider()
                .signature_verification_algorithms,
        })
    }
}

impl ServerCertVerifier for CertificateFingerprintVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        if let Some(expected) = self.expected_fingerprint.as_deref() {
            let actual = sha256_fingerprint(end_entity.as_ref());
            if !constant_time_eq(expected.as_bytes(), actual.as_bytes()) {
                return Err(rustls::Error::General(
                    "Gemacast PC certificate fingerprint changed".into(),
                ));
            }
        }
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.signature_algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.signature_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.signature_algorithms.supported_schemes()
    }
}

pub fn client_config(expected_fingerprint: Option<&str>) -> Result<rustls::ClientConfig, String> {
    let provider = rustls::crypto::ring::default_provider();
    let verifier = Arc::new(CertificateFingerprintVerifier::new(expected_fingerprint)?);
    rustls::ClientConfig::builder_with_provider(Arc::new(provider))
        .with_protocol_versions(&[&rustls::version::TLS13, &rustls::version::TLS12])
        .map_err(|error| format!("failed to enable TLS 1.2/1.3: {error}"))
        .map(|builder| {
            builder
                .dangerous()
                .with_custom_certificate_verifier(verifier)
                .with_no_client_auth()
        })
}

pub fn response_certificate_fingerprint(response: &reqwest::Response) -> Result<String, String> {
    let certificate = response
        .extensions()
        .get::<reqwest::tls::TlsInfo>()
        .and_then(reqwest::tls::TlsInfo::peer_certificate)
        .ok_or_else(|| "HTTPS response did not expose the PC certificate".to_string())?;
    Ok(sha256_fingerprint(certificate))
}

fn normalize_fingerprint(fingerprint: &str) -> Result<String, String> {
    let fingerprint = fingerprint.to_ascii_lowercase();
    let bytes = hex::decode(&fingerprint)
        .map_err(|_| "PC certificate fingerprint must be hexadecimal".to_string())?;
    if bytes.len() != 32 {
        return Err("PC certificate fingerprint must contain exactly 32 bytes".into());
    }
    Ok(fingerprint)
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .fold(0u8, |difference, (left, right)| difference | (left ^ right))
            == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_should_require_a_sha256_hex_value() {
        assert!(client_config(Some("not-a-fingerprint")).is_err());
        assert!(client_config(Some(&"00".repeat(32))).is_ok());
    }

    #[test]
    fn certificate_pin_should_accept_a_match_and_reject_a_mismatch() {
        let certificate = CertificateDer::from(vec![1, 2, 3, 4]);
        let fingerprint = sha256_fingerprint(certificate.as_ref());
        let server_name = ServerName::try_from("gemacast.local").unwrap();
        let now = UnixTime::since_unix_epoch(std::time::Duration::ZERO);

        let matching = CertificateFingerprintVerifier::new(Some(&fingerprint)).unwrap();
        assert!(
            matching
                .verify_server_cert(&certificate, &[], &server_name, &[], now)
                .is_ok()
        );

        let mismatched = CertificateFingerprintVerifier::new(Some(&"00".repeat(32))).unwrap();
        assert!(
            mismatched
                .verify_server_cert(&certificate, &[], &server_name, &[], now)
                .is_err()
        );
    }
}
