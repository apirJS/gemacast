//! In-memory proof challenges layered over the persistent trusted-device store.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine;
use gemacast_core::control::device_auth::{
    build_device_auth_transcript, device_public_key_fingerprint, pairing_code, random_auth_value,
    validate_device_public_key, verify_device_auth_signature,
};
use gemacast_core::control::types::{DeviceAuthChallenge, DeviceAuthRequest};
use gemacast_core::domain::types::DeviceId;

const CHALLENGE_TTL: Duration = Duration::from_secs(90);
const PENDING_PAIRING_TTL: Duration = Duration::from_secs(65);
const MAX_PENDING_CHALLENGES: usize = 256;

#[derive(Debug, Clone)]
pub struct VerifiedDeviceIdentity {
    pub public_key: String,
    pub fingerprint: String,
    pub pairing_code: String,
}

struct PendingChallenge {
    device_id: DeviceId,
    pc_id: DeviceId,
    pc_certificate_fingerprint: String,
    public_key: String,
    phone_nonce: String,
    challenge: String,
    created_at: Instant,
}

struct PendingPairing {
    device_id: DeviceId,
    identity: VerifiedDeviceIdentity,
    created_at: Instant,
}

#[derive(Default)]
struct DeviceAuthState {
    challenges: HashMap<String, PendingChallenge>,
    pending_pairings: HashMap<String, PendingPairing>,
}

#[derive(Clone)]
pub struct DeviceAuthManager {
    state: Arc<Mutex<DeviceAuthState>>,
    challenge_ttl: Duration,
}

impl Default for DeviceAuthManager {
    fn default() -> Self {
        Self {
            state: Arc::new(Mutex::new(DeviceAuthState::default())),
            challenge_ttl: CHALLENGE_TTL,
        }
    }
}

impl DeviceAuthManager {
    fn prune_expired(&self, state: &mut DeviceAuthState) {
        let challenge_ttl = self.challenge_ttl;
        state
            .challenges
            .retain(|_, pending| pending.created_at.elapsed() <= challenge_ttl);
        state
            .pending_pairings
            .retain(|_, pending| pending.created_at.elapsed() <= PENDING_PAIRING_TTL);
    }

    pub fn begin(
        &self,
        device_id: DeviceId,
        pc_id: DeviceId,
        pc_certificate_fingerprint: String,
        requires_approval: bool,
        public_key: String,
        phone_nonce: String,
    ) -> Result<DeviceAuthChallenge, String> {
        validate_device_public_key(&public_key)?;
        let decoded_nonce = base64::engine::general_purpose::STANDARD
            .decode(&phone_nonce)
            .map_err(|error| format!("device nonce is not valid Base64: {error}"))?;
        if decoded_nonce.len() != 32 {
            return Err("device nonce must contain exactly 32 bytes".into());
        }
        let challenge_id = random_auth_value(16)?;
        let challenge = random_auth_value(32)?;
        let transcript = build_device_auth_transcript(
            &device_id,
            &pc_id,
            &pc_certificate_fingerprint,
            &public_key,
            &phone_nonce,
            &challenge_id,
            &challenge,
        );
        let pairing_code = pairing_code(&transcript);
        let mut state = self
            .state
            .lock()
            .map_err(|_| "device authentication state is unavailable".to_string())?;
        self.prune_expired(&mut state);
        if state.challenges.len() >= MAX_PENDING_CHALLENGES {
            return Err("too many pending device-authentication requests".into());
        }
        state.challenges.insert(
            challenge_id.clone(),
            PendingChallenge {
                device_id,
                pc_id,
                pc_certificate_fingerprint: pc_certificate_fingerprint.clone(),
                public_key,
                phone_nonce,
                challenge: challenge.clone(),
                created_at: Instant::now(),
            },
        );
        Ok(DeviceAuthChallenge {
            challenge_id,
            challenge,
            pc_certificate_fingerprint,
            pairing_code,
            requires_approval,
            expires_in_seconds: self.challenge_ttl.as_secs(),
        })
    }

    pub fn verify(
        &self,
        device_id: &DeviceId,
        pc_id: &DeviceId,
        pc_certificate_fingerprint: &str,
        auth: &DeviceAuthRequest,
    ) -> Result<VerifiedDeviceIdentity, String> {
        let challenge_id = auth
            .challenge_id
            .as_deref()
            .ok_or_else(|| "device authentication challenge is missing".to_string())?;
        let signature = auth
            .signature
            .as_deref()
            .ok_or_else(|| "device authentication signature is missing".to_string())?;
        let mut state = self
            .state
            .lock()
            .map_err(|_| "device authentication state is unavailable".to_string())?;
        self.prune_expired(&mut state);
        let pending = state.challenges.remove(challenge_id).ok_or_else(|| {
            "device authentication challenge is invalid or already used".to_string()
        })?;
        if pending.created_at.elapsed() > self.challenge_ttl {
            return Err("device authentication challenge expired".into());
        }
        if &pending.device_id != device_id
            || &pending.pc_id != pc_id
            || pending.pc_certificate_fingerprint != pc_certificate_fingerprint
            || pending.public_key != auth.public_key
            || pending.phone_nonce != auth.phone_nonce
        {
            return Err("device authentication challenge does not match this device".into());
        }
        let transcript = build_device_auth_transcript(
            device_id,
            pc_id,
            pc_certificate_fingerprint,
            &auth.public_key,
            &auth.phone_nonce,
            challenge_id,
            &pending.challenge,
        );
        verify_device_auth_signature(&auth.public_key, signature, &transcript)?;
        Ok(VerifiedDeviceIdentity {
            public_key: auth.public_key.clone(),
            fingerprint: device_public_key_fingerprint(&auth.public_key)?,
            pairing_code: pairing_code(&transcript),
        })
    }

    pub fn hold_pending_pairing(
        &self,
        request_id: String,
        device_id: DeviceId,
        identity: VerifiedDeviceIdentity,
    ) -> Result<(), String> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| "device authentication state is unavailable".to_string())?;
        self.prune_expired(&mut state);
        state.pending_pairings.insert(
            request_id,
            PendingPairing {
                device_id,
                identity,
                created_at: Instant::now(),
            },
        );
        Ok(())
    }

    pub fn pending_pairing(
        &self,
        request_id: &str,
        device_id: &DeviceId,
        public_key: &str,
    ) -> Option<VerifiedDeviceIdentity> {
        self.state.lock().ok().and_then(|mut state| {
            self.prune_expired(&mut state);
            state
                .pending_pairings
                .get(request_id)
                .filter(|pending| {
                    &pending.device_id == device_id && pending.identity.public_key == public_key
                })
                .map(|pending| pending.identity.clone())
        })
    }

    pub fn remove_pending_pairing(&self, request_id: &str) {
        if let Ok(mut state) = self.state.lock() {
            state.pending_pairings.remove(request_id);
        }
    }

    pub fn cancel_pending_for_device(&self, device_id: &DeviceId) -> usize {
        let Ok(mut state) = self.state.lock() else {
            return 0;
        };
        self.prune_expired(&mut state);
        let before = state.pending_pairings.len() + state.challenges.len();
        state
            .pending_pairings
            .retain(|_, pending| &pending.device_id != device_id);
        state
            .challenges
            .retain(|_, pending| &pending.device_id != device_id);
        before - state.pending_pairings.len() - state.challenges.len()
    }

    pub fn clear_pending(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.challenges.clear();
            state.pending_pairings.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use ring::rand::SystemRandom;
    use ring::signature::{EcdsaKeyPair, KeyPair};

    fn signed_auth(
        manager: &DeviceAuthManager,
        device_id: &DeviceId,
        pc_id: &DeviceId,
    ) -> (DeviceAuthRequest, DeviceAuthChallenge) {
        let random = SystemRandom::new();
        let pkcs8 =
            EcdsaKeyPair::generate_pkcs8(&ring::signature::ECDSA_P256_SHA256_ASN1_SIGNING, &random)
                .unwrap();
        let key_pair = EcdsaKeyPair::from_pkcs8(
            &ring::signature::ECDSA_P256_SHA256_ASN1_SIGNING,
            pkcs8.as_ref(),
            &random,
        )
        .unwrap();
        let public_key = base64::engine::general_purpose::STANDARD.encode(key_pair.public_key());
        let phone_nonce = random_auth_value(32).unwrap();
        let challenge = manager
            .begin(
                device_id.clone(),
                pc_id.clone(),
                "pc-certificate".into(),
                true,
                public_key.clone(),
                phone_nonce.clone(),
            )
            .unwrap();
        let transcript = build_device_auth_transcript(
            device_id,
            pc_id,
            "pc-certificate",
            &public_key,
            &phone_nonce,
            &challenge.challenge_id,
            &challenge.challenge,
        );
        let signature = key_pair.sign(&random, &transcript).unwrap();
        (
            DeviceAuthRequest {
                public_key,
                phone_nonce,
                challenge_id: Some(challenge.challenge_id.clone()),
                signature: Some(
                    base64::engine::general_purpose::STANDARD.encode(signature.as_ref()),
                ),
                phone_confirmation: Some(true),
            },
            challenge,
        )
    }

    #[test]
    fn proof_should_verify_once_and_replay_should_fail() {
        let manager = DeviceAuthManager::default();
        let device_id = DeviceId("phone-1".into());
        let pc_id = DeviceId("pc-1".into());
        let (auth, _) = signed_auth(&manager, &device_id, &pc_id);

        manager
            .verify(&device_id, &pc_id, "pc-certificate", &auth)
            .unwrap();
        assert!(
            manager
                .verify(&device_id, &pc_id, "pc-certificate", &auth)
                .is_err()
        );
    }

    #[test]
    fn proof_should_be_bound_to_the_device_id() {
        let manager = DeviceAuthManager::default();
        let device_id = DeviceId("phone-1".into());
        let pc_id = DeviceId("pc-1".into());
        let (auth, _) = signed_auth(&manager, &device_id, &pc_id);

        assert!(
            manager
                .verify(&DeviceId("phone-2".into()), &pc_id, "pc-certificate", &auth,)
                .is_err()
        );
    }

    #[test]
    fn challenge_should_require_a_32_byte_base64_nonce() {
        let manager = DeviceAuthManager::default();
        let device_id = DeviceId("phone-1".into());
        let public_key = base64::engine::general_purpose::STANDARD.encode([4_u8; 65]);

        assert!(
            manager
                .begin(
                    device_id.clone(),
                    DeviceId("pc-1".into()),
                    "pc-certificate".into(),
                    true,
                    public_key.clone(),
                    "not-base64".into(),
                )
                .is_err()
        );
        assert!(
            manager
                .begin(
                    device_id,
                    DeviceId("pc-1".into()),
                    "pc-certificate".into(),
                    true,
                    public_key,
                    base64::engine::general_purpose::STANDARD.encode([1_u8; 31]),
                )
                .is_err()
        );
    }

    #[test]
    fn cancelling_a_device_should_remove_its_challenges_and_pairings_only() {
        let manager = DeviceAuthManager::default();
        let phone_one = DeviceId("phone-1".into());
        let phone_two = DeviceId("phone-2".into());
        let pc_id = DeviceId("pc-1".into());
        let (auth_one, _) = signed_auth(&manager, &phone_one, &pc_id);
        let (auth_two, _) = signed_auth(&manager, &phone_two, &pc_id);
        let identity_one = manager
            .verify(&phone_one, &pc_id, "pc-certificate", &auth_one)
            .unwrap();
        let identity_two = manager
            .verify(&phone_two, &pc_id, "pc-certificate", &auth_two)
            .unwrap();
        manager
            .hold_pending_pairing("request-1".into(), phone_one.clone(), identity_one)
            .unwrap();
        manager
            .hold_pending_pairing("request-2".into(), phone_two.clone(), identity_two.clone())
            .unwrap();

        assert_eq!(manager.cancel_pending_for_device(&phone_one), 1);
        assert!(
            manager
                .pending_pairing("request-1", &phone_one, &auth_one.public_key)
                .is_none()
        );
        assert!(
            manager
                .pending_pairing("request-2", &phone_two, &identity_two.public_key)
                .is_some()
        );
    }

    #[test]
    fn expired_pending_pairing_should_be_pruned() {
        let manager = DeviceAuthManager::default();
        let device_id = DeviceId("phone-1".into());
        let identity = VerifiedDeviceIdentity {
            public_key: "key".into(),
            fingerprint: "fingerprint".into(),
            pairing_code: "123456".into(),
        };
        manager.state.lock().unwrap().pending_pairings.insert(
            "expired".into(),
            PendingPairing {
                device_id: device_id.clone(),
                identity,
                created_at: Instant::now() - PENDING_PAIRING_TTL - Duration::from_secs(1),
            },
        );

        assert!(
            manager
                .pending_pairing("expired", &device_id, "key")
                .is_none()
        );
    }
}
