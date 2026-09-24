use crate::control::SessionGeneration;
use crate::domain::error::ProtocolError;
use crate::domain::types::DeviceId;

pub(crate) const ADB_HANDSHAKE_VERSION: u8 = 1;
pub(crate) const MAX_DEVICE_ID_LENGTH: usize = 128;
pub(crate) const MAX_SESSION_TOKEN_LENGTH: usize = 255;

/// Owns the authenticated ADB audio handshake wire format.
pub struct AdbAudioHandshake;

impl AdbAudioHandshake {
    pub fn encode(
        device_id: &DeviceId,
        session_token: &str,
        generation: SessionGeneration,
    ) -> Result<Vec<u8>, ProtocolError> {
        let device_id = device_id.0.as_bytes();
        let token = session_token.as_bytes();
        let mut handshake = Vec::with_capacity(3 + device_id.len() + token.len() + 8);
        handshake.push(ADB_HANDSHAKE_VERSION);
        Self::write_field(&mut handshake, device_id, "device ID", MAX_DEVICE_ID_LENGTH)?;
        Self::write_field(
            &mut handshake,
            token,
            "session token",
            MAX_SESSION_TOKEN_LENGTH,
        )?;
        handshake.extend_from_slice(&generation.0.to_be_bytes());
        Ok(handshake)
    }

    fn write_field(
        output: &mut Vec<u8>,
        value: &[u8],
        field: &'static str,
        max: usize,
    ) -> Result<(), ProtocolError> {
        if value.is_empty() {
            return Err(ProtocolError::EmptyAdbHandshakeField { field });
        }
        if value.len() > max {
            return Err(ProtocolError::AdbHandshakeFieldTooLong {
                field,
                got: value.len(),
                max,
            });
        }
        output.push(value.len() as u8);
        output.extend_from_slice(value);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_classify_an_empty_session_token() {
        let result = AdbAudioHandshake::encode(&DeviceId("phone".into()), "", SessionGeneration(1));

        assert!(matches!(
            result,
            Err(ProtocolError::EmptyAdbHandshakeField {
                field: "session token"
            })
        ));
    }

    #[test]
    fn should_classify_an_oversized_device_id() {
        let result = AdbAudioHandshake::encode(
            &DeviceId("x".repeat(MAX_DEVICE_ID_LENGTH + 1)),
            "token",
            SessionGeneration(1),
        );

        assert!(matches!(
            result,
            Err(ProtocolError::AdbHandshakeFieldTooLong {
                field: "device ID",
                ..
            })
        ));
    }
}
