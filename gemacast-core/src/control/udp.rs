use async_trait::async_trait;
use std::net::IpAddr;

use crate::control::handler::ControlHandler;
use crate::discovery::send_control_message;
use crate::error::{ControlError, GemaCastError};
use crate::types::{
    AudioSource, ConnectionMode, ControlMessage, DeviceId, JitterConfig, SenderCapabilities,
};

pub struct UdpControlHandler {
    pub target_ip: IpAddr,
}

impl UdpControlHandler {
    pub fn new(target_ip: IpAddr) -> Self {
        Self { target_ip }
    }
}

#[async_trait]
impl ControlHandler for UdpControlHandler {
    async fn handle_connect(
        &self,
        device_id: DeviceId,
        device_name: String,
        source: AudioSource,
        mode: ConnectionMode,
        jitter_config: JitterConfig,
    ) -> Result<(), GemaCastError> {
        let msg = ControlMessage::Connect {
            device_id,
            device_name,
            mode,
            exclusive_mode: false,
            jitter_config,
            transport: None,
            source,
        };

        send_control_message(self.target_ip, msg)
            .await
            .map_err(|e| ControlError::SendFailed {
                addr: self.target_ip.to_string(),
                source: std::io::Error::other(e.to_string()),
            })?;
        Ok(())
    }

    async fn handle_disconnect(&self, device_id: DeviceId) -> Result<(), GemaCastError> {
        let msg = ControlMessage::Disconnect { device_id };
        send_control_message(self.target_ip, msg)
            .await
            .map_err(|e| ControlError::SendFailed {
                addr: self.target_ip.to_string(),
                source: std::io::Error::other(e.to_string()),
            })?;
        Ok(())
    }

    async fn get_sources(&self) -> Result<(Vec<AudioSource>, SenderCapabilities), GemaCastError> {
        // UDP Control doesn't support request/response pattern properly.
        // For now, we return Desktop only for UDP clients until Axum is ready.
        let caps = SenderCapabilities {
            supports_process_capture: false,
        };
        Ok((vec![AudioSource::Desktop], caps))
    }

    async fn handle_change_source(
        &self,
        device_id: DeviceId,
        source: AudioSource,
    ) -> Result<(), GemaCastError> {
        let msg = ControlMessage::ChangeSource { device_id, source };
        send_control_message(self.target_ip, msg)
            .await
            .map_err(|e| ControlError::SendFailed {
                addr: self.target_ip.to_string(),
                source: std::io::Error::other(e.to_string()),
            })?;
        Ok(())
    }
}
