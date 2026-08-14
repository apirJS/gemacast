//! Shared wire-format helpers for persistent device authentication.
//!
//! Android signs this exact transcript with its non-exportable device key and
//! the PC verifies it before issuing a short-lived bearer session. Length
//! prefixes make the transcript unambiguous without constraining device IDs.

use crate::domain::types::DeviceId;
use base64::Engine;
use sha2::{Digest, Sha256};

pub const DEVICE_AUTH_DOMAIN: &[u8] = b"gemacast-device-auth-v1";
pub const PAIRING_CODE_DOMAIN: &[u8] = b"gemacast-pairing-code-v1";

pub fn build_device_auth_transcript(
    device_id: &DeviceId,
    pc_id: &DeviceId,
    pc_certificate_fingerprint: &str,
    public_key: &str,
    phone_nonce: &str,
    challenge_id: &str,
    challenge: &str,
) -> Vec<u8> {
    let fields = [
        DEVICE_AUTH_DOMAIN,
        device_id.0.as_bytes(),
        pc_id.0.as_bytes(),
        pc_certificate_fingerprint.as_bytes(),
        public_key.as_bytes(),
        phone_nonce.as_bytes(),
        challenge_id.as_bytes(),
        challenge.as_bytes(),
    ];
    let capacity = fields.iter().map(|field| 4 + field.len()).sum();
    let mut transcript = Vec::with_capacity(capacity);
    for field in fields {
        let length = u32::try_from(field.len()).expect("authentication field is too large");
        transcript.extend_from_slice(&length.to_be_bytes());
        transcript.extend_from_slice(field);
    }
    transcript
}

/// Return a short comparison code for the PC and phone pairing dialogs.
///
/// The code is not an authentication secret. It lets the user detect an
/// active relay during first pairing by comparing two independently computed
/// views of the certificate-bound signed transcript.
pub fn pairing_code(transcript: &[u8]) -> String {
    let mut digest = Sha256::new();
    digest.update(PAIRING_CODE_DOMAIN);
    digest.update(transcript);
    let digest = digest.finalize();
    let value = u32::from_be_bytes([digest[0], digest[1], digest[2], digest[3]]) % 1_000_000;
    format!("{value:06}")
}

pub fn verify_device_auth_signature(
    public_key: &str,
    signature: &str,
    transcript: &[u8],
) -> Result<(), String> {
    let public_key = decode_device_public_key(public_key)?;
    let signature = base64::engine::general_purpose::STANDARD
        .decode(signature)
        .map_err(|error| format!("device signature is not valid Base64: {error}"))?;
    ring::signature::UnparsedPublicKey::new(&ring::signature::ECDSA_P256_SHA256_ASN1, public_key)
        .verify(transcript, &signature)
        .map_err(|_| "device signature verification failed".to_string())
}

pub fn validate_device_public_key(public_key: &str) -> Result<(), String> {
    decode_device_public_key(public_key).map(|_| ())
}

pub fn device_public_key_fingerprint(public_key: &str) -> Result<String, String> {
    let public_key = decode_device_public_key(public_key)?;
    Ok(sha256_fingerprint(&public_key))
}

pub fn sha256_fingerprint(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

pub fn random_auth_value(byte_count: usize) -> Result<String, String> {
    let mut bytes = vec![0u8; byte_count];
    getrandom::fill(&mut bytes)
        .map_err(|error| format!("failed to generate authentication randomness: {error}"))?;
    Ok(base64::engine::general_purpose::STANDARD.encode(bytes))
}

fn decode_device_public_key(public_key: &str) -> Result<Vec<u8>, String> {
    let public_key = base64::engine::general_purpose::STANDARD
        .decode(public_key)
        .map_err(|error| format!("device public key is not valid Base64: {error}"))?;
    if public_key.len() != 65 || public_key.first() != Some(&0x04) {
        return Err("device public key must be an uncompressed P-256 point".into());
    }
    Ok(public_key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transcript_should_be_length_prefixed_and_deterministic() {
        let transcript = build_device_auth_transcript(
            &DeviceId("phone".into()),
            &DeviceId("pc".into()),
            "certificate",
            "key",
            "nonce",
            "request",
            "challenge",
        );

        assert_eq!(
            &transcript[0..4],
            &(DEVICE_AUTH_DOMAIN.len() as u32).to_be_bytes()
        );
        assert_eq!(
            transcript,
            build_device_auth_transcript(
                &DeviceId("phone".into()),
                &DeviceId("pc".into()),
                "certificate",
                "key",
                "nonce",
                "request",
                "challenge",
            )
        );
    }

    #[test]
    fn pairing_code_should_be_six_decimal_digits() {
        let code = pairing_code(b"transcript");
        assert_eq!(code.len(), 6);
        assert!(code.bytes().all(|byte| byte.is_ascii_digit()));
    }
}
