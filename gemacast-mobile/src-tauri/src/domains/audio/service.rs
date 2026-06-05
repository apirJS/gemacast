//! Pure service functions for the audio domain, decoupled from Tauri.
//!
//! [`AudioService`] groups all trait dependencies needed to handle audio
//! commands. The `#[tauri::command]` handlers in [`super::commands`] are
//! thin wrappers that delegate to these methods.

use std::net::IpAddr;
use std::sync::Arc;

use gemacast_core::control::types::ConnectReq;
use gemacast_core::types::{AudioSource, ConnectionMode, DeviceId, JitterConfig};

use crate::traits::{
    ConnectParams, FrontendNotifier, PlatformService, PlaybackState, ResumeParams, SenderControlClientFactory,
    SessionManager, SessionParams,
};

/// Handles all audio-related operations: connect, disconnect, playback
/// control, source/bitrate changes, and WebSocket management.
///
/// Dependencies are injected as trait objects, making every method
/// independently unit-testable with mock implementations.
pub struct AudioService {
    pub session: Arc<dyn SessionManager>,
    pub client_factory: Arc<dyn SenderControlClientFactory>,
    pub notifier: Arc<dyn FrontendNotifier>,
    pub platform: Arc<dyn PlatformService>,
}

impl AudioService {
    /// Connect to a sender: HTTP handshake → spawn audio receiver → sync service.
    pub async fn connect_to_sender(&self, params: ConnectParams) -> Result<(), String> {
        let ip_addr: IpAddr = params.ip.parse().map_err(|e: std::net::AddrParseError| e.to_string())?;
        let client = self.client_factory.create(ip_addr);

        client
            .connect(ConnectReq {
                device_id: params.device_id.clone(),
                device_name: params.device_name.clone(),
                source: None,
                mode: params.mode,
                jitter_config: params.jitter_config.clone(),
                bitrate: params.bitrate,
            })
            .await?;

        let is_tcp = params.mode == ConnectionMode::Adb;

        self.session
            .start_session(SessionParams {
                jitter_config: params.jitter_config,
                is_tcp,
                exclusive_mode: params.exclusive_mode,
                target_ip: Some(ip_addr),
                mode: params.mode,
                device_id: params.device_id.to_string(),
                bitrate: params.bitrate,
            })
            .await?;

        self.platform.set_streaming_flag(true);
        self.platform.sync_service(PlaybackState::Playing, params.exclusive_mode);

        Ok(())
    }

    /// Disconnect from a sender: HTTP disconnect → tear down session → sync service.
    pub async fn disconnect_from_sender(
        &self,
        ip: IpAddr,
        device_id: DeviceId,
    ) -> Result<(), String> {
        let client = self.client_factory.create(ip);
        let _ = client.disconnect(device_id).await;

        self.session.stop_session().await;

        self.platform.set_streaming_flag(false);
        self.platform.sync_service(PlaybackState::Stopped, false);
        Ok(())
    }

    /// Resume audio playback after a pause.
    ///
    /// Re-enables the Oboe output callback via `resume_playback()` without
    /// sending an HTTP reconnect — the network connection stays alive.
    pub async fn start_audio_playback(
        &self,
        _resume: Option<ResumeParams>,
    ) -> Result<(), String> {
        self.session.resume_playback().await?;
        let info = self.session.session_info().await;
        let exclusive = info.as_ref().is_some_and(|i| i.exclusive_mode);

        self.platform.sync_service(PlaybackState::Playing, exclusive);
        Ok(())
    }

    /// Pause audio playback without tearing down the session.
    ///
    /// Silences the Oboe output callback via `pause_playback()` while
    /// keeping the network receive thread, heartbeat, and WebSocket alive.
    /// Does NOT send an HTTP disconnect to the PC.
    pub async fn stop_audio_playback(
        &self,
        _ip: Option<IpAddr>,
        _device_id: Option<DeviceId>,
    ) -> Result<(), String> {
        self.session.pause_playback().await?;

        self.platform.sync_service(PlaybackState::Paused, false);
        Ok(())
    }

    /// Kill playback immediately: tear down session, clear streaming flag.
    pub async fn kill_playback(&self) -> Result<(), String> {
        self.session.stop_session().await;

        self.platform.set_streaming_flag(false);
        self.platform.sync_service(PlaybackState::Stopped, false);
        Ok(())
    }

    /// Notify that streaming has stopped (called by frontend).
    pub fn notify_streaming_stopped(&self) {
        self.platform.set_streaming_flag(false);
    }

    /// Update the jitter buffer configuration on the active session.
    pub async fn update_jitter_config(&self, config: JitterConfig) -> Result<(), String> {
        self.session.update_jitter_config(config).await;
        Ok(())
    }

    /// Request audio sources from the sender.
    pub async fn get_audio_sources(
        &self,
        ip: IpAddr,
    ) -> Result<
        (
            Vec<AudioSource>,
            gemacast_core::types::SenderCapabilities,
        ),
        String,
    > {
        let client = self.client_factory.create(ip);
        client.get_audio_sources().await
    }

    /// Probe a sender for its current state.
    pub async fn probe_sender(
        &self,
        ip: IpAddr,
        device_id: DeviceId,
    ) -> Result<gemacast_core::control::types::PresenceResponse, String> {
        let client = self.client_factory.create(ip);
        client.probe(Some(device_id)).await
    }

    /// Request the sender to change audio source.
    pub async fn change_audio_source(
        &self,
        ip: IpAddr,
        device_id: DeviceId,
        source: AudioSource,
    ) -> Result<(), String> {
        let client = self.client_factory.create(ip);
        client.change_source(device_id, source).await
    }

    /// Request the sender to change encoding bitrate.
    pub async fn change_audio_bitrate(
        &self,
        ip: IpAddr,
        device_id: DeviceId,
        bitrate: Option<i32>,
    ) -> Result<(), String> {
        self.session.update_bitrate(bitrate).await;
        let client = self.client_factory.create(ip);
        client.change_bitrate(device_id, bitrate).await
    }

    /// Request capturable process list from the sender.
    pub async fn get_process_list(
        &self,
        ip: IpAddr,
    ) -> Result<Vec<gemacast_core::types::ProcessInfo>, String> {
        let client = self.client_factory.create(ip);
        client.get_process_list().await
    }

    /// Establish a WebSocket control connection to the sender.
    ///
    /// Spawns a read loop that forwards disconnect/error events to the frontend
    /// and tracks the task handle in the session manager.
    pub async fn establish_websocket(
        &self,
        sender_ip: IpAddr,
        device_id: String,
    ) -> Result<(), String> {
        let ws_client =
            gemacast_core::control::WsControlClient::new(sender_ip, &device_id)
                .await
                .map_err(|e| format!("Failed to establish WebSocket: {}", e))?;

        let notifier = self.notifier.clone();
        let task = tokio::spawn(async move {
            #[allow(clippy::never_loop)]
            loop {
                match ws_client.recv_event().await {
                    Ok(gemacast_core::control::types::WsEvent::Disconnect) => {
                        notifier.emit_ws_disconnect();
                        break;
                    }
                    Ok(gemacast_core::control::types::WsEvent::Error { message }) => {
                        notifier.emit_ws_error(message);
                        notifier.emit_ws_disconnect();
                        break;
                    }
                    Err(_) => {
                        break;
                    }
                }
            }
        });

        self.session.start_ws_client(task).await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::mocks::*;
    use crate::traits::SessionInfo;

    fn make_service(
        session: Arc<MockSessionManager>,
        client: Arc<MockSenderControlClient>,
        platform: Arc<MockPlatformService>,
    ) -> AudioService {
        let notifier = Arc::new(MockFrontendNotifier::new());
        let factory = Arc::new(MockSenderControlClientFactory::new(client));
        AudioService {
            session,
            client_factory: factory,
            notifier,
            platform,
        }
    }

    #[tokio::test]
    async fn connect_should_send_http_then_start_session() {
        let session = Arc::new(MockSessionManager::new());
        let client = Arc::new(MockSenderControlClient::new());
        let platform = Arc::new(MockPlatformService::new());
        let service = make_service(session.clone(), client.clone(), platform.clone());

        service
            .connect_to_sender(ConnectParams {
                ip: "192.168.1.5".to_string(),
                device_id: DeviceId("phone-1".into()),
                device_name: "My Phone".into(),
                mode: ConnectionMode::Wifi,
                exclusive_mode: false,
                jitter_config: JitterConfig::default(),
                bitrate: None,
            })
            .await
            .unwrap();

        // HTTP connect was called
        let client_calls = client.take_calls();
        assert_eq!(client_calls.len(), 1);
        assert!(matches!(
            &client_calls[0],
            ControlClientCall::Connect { device_id } if device_id.0 == "phone-1"
        ));

        // Session was started
        let session_calls = session.take_calls();
        assert!(session_calls
            .iter()
            .any(|c| matches!(c, SessionCall::StartSession { .. })));

        // Platform was synced
        let platform_calls = platform.take_calls();
        assert!(platform_calls
            .iter()
            .any(|c| matches!(c, PlatformCall::SetStreamingFlag { active: true })));
        assert!(platform_calls
            .iter()
            .any(|c| matches!(c, PlatformCall::SyncService { is_playing: true, .. })));
    }

    #[tokio::test]
    async fn disconnect_should_stop_session_and_sync() {
        let session = Arc::new(MockSessionManager::new());
        let client = Arc::new(MockSenderControlClient::new());
        let platform = Arc::new(MockPlatformService::new());
        let service = make_service(session.clone(), client.clone(), platform.clone());

        service
            .disconnect_from_sender(
                "192.168.1.5".parse().unwrap(),
                DeviceId("phone-1".into()),
            )
            .await
            .unwrap();

        // HTTP disconnect was called
        let client_calls = client.take_calls();
        assert!(matches!(
            &client_calls[0],
            ControlClientCall::Disconnect { device_id } if device_id.0 == "phone-1"
        ));

        // Session was stopped
        let session_calls = session.take_calls();
        assert!(session_calls
            .iter()
            .any(|c| matches!(c, SessionCall::StopSession)));

        // Platform streaming flag cleared
        let platform_calls = platform.take_calls();
        assert!(platform_calls
            .iter()
            .any(|c| matches!(c, PlatformCall::SetStreamingFlag { active: false })));
    }

    #[tokio::test]
    async fn start_playback_should_call_resume_playback() {
        let session = Arc::new(MockSessionManager::new());
        let client = Arc::new(MockSenderControlClient::new());
        let platform = Arc::new(MockPlatformService::new());
        let service = make_service(session.clone(), client.clone(), platform.clone());

        service.start_audio_playback(None).await.unwrap();

        let session_calls = session.take_calls();
        assert!(session_calls
            .iter()
            .any(|c| matches!(c, SessionCall::ResumePlayback)));
    }

    #[tokio::test]
    async fn start_playback_should_not_send_http_reconnect() {
        let session = Arc::new(
            MockSessionManager::new().with_session_info(SessionInfo {
                exclusive_mode: false,
                mode: ConnectionMode::Wifi,
                bitrate: Some(128000),
                jitter_config: JitterConfig::default(),
            }),
        );
        let client = Arc::new(MockSenderControlClient::new());
        let platform = Arc::new(MockPlatformService::new());
        let service = make_service(session.clone(), client.clone(), platform.clone());

        service
            .start_audio_playback(Some(ResumeParams {
                ip: "192.168.1.5".parse().unwrap(),
                device_id: DeviceId("phone-1".into()),
                device_name: "My Phone".into(),
            }))
            .await
            .unwrap();

        // No HTTP reconnect should be sent — the connection stays alive
        let client_calls = client.take_calls();
        assert_eq!(client_calls.len(), 0);
    }

    #[tokio::test]
    async fn stop_playback_should_pause_not_stop_session() {
        let session = Arc::new(MockSessionManager::new());
        let client = Arc::new(MockSenderControlClient::new());
        let platform = Arc::new(MockPlatformService::new());
        let service = make_service(session.clone(), client.clone(), platform.clone());

        service
            .stop_audio_playback(
                Some("192.168.1.5".parse().unwrap()),
                Some(DeviceId("phone-1".into())),
            )
            .await
            .unwrap();

        // Should pause, NOT stop the session
        let session_calls = session.take_calls();
        assert!(session_calls
            .iter()
            .any(|c| matches!(c, SessionCall::PausePlayback)));
        assert!(!session_calls
            .iter()
            .any(|c| matches!(c, SessionCall::StopSession)));

        // No HTTP disconnect should be sent
        let client_calls = client.take_calls();
        assert_eq!(client_calls.len(), 0);

        // Platform service should be notified
        let platform_calls = platform.take_calls();
        assert!(platform_calls
            .iter()
            .any(|c| matches!(c, PlatformCall::SyncService { is_playing: false, .. })));
    }

    #[tokio::test]
    async fn kill_playback_should_stop_everything() {
        let session = Arc::new(MockSessionManager::new());
        let client = Arc::new(MockSenderControlClient::new());
        let platform = Arc::new(MockPlatformService::new());
        let service = make_service(session.clone(), client.clone(), platform.clone());

        service.kill_playback().await.unwrap();

        let session_calls = session.take_calls();
        assert!(session_calls
            .iter()
            .any(|c| matches!(c, SessionCall::StopSession)));

        let platform_calls = platform.take_calls();
        assert!(platform_calls
            .iter()
            .any(|c| matches!(c, PlatformCall::SetStreamingFlag { active: false })));
        assert!(platform_calls
            .iter()
            .any(|c| matches!(c, PlatformCall::SyncService { is_playing: false, .. })));
    }

    #[tokio::test]
    async fn change_bitrate_should_update_session_and_send_http() {
        let session = Arc::new(MockSessionManager::new());
        let client = Arc::new(MockSenderControlClient::new());
        let platform = Arc::new(MockPlatformService::new());
        let service = make_service(session.clone(), client.clone(), platform.clone());

        service
            .change_audio_bitrate(
                "192.168.1.5".parse().unwrap(),
                DeviceId("phone-1".into()),
                Some(256000),
            )
            .await
            .unwrap();

        let session_calls = session.take_calls();
        assert!(session_calls.iter().any(
            |c| matches!(c, SessionCall::UpdateBitrate { bitrate: Some(256000) })
        ));

        let client_calls = client.take_calls();
        assert!(matches!(
            &client_calls[0],
            ControlClientCall::ChangeBitrate { bitrate: Some(256000), .. }
        ));
    }
}
