//! Hand-written mock implementations for unit testing.
//!
//! Each mock records calls in a `Mutex<Vec<..>>` so tests can assert
//! what was called and with which arguments. Mirrors the pattern from

pub mod mocks {
    use std::net::IpAddr;
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use gemacast_core::control::types::{ConnectReq, PresenceResponse};
    use gemacast_core::domain::types::{
        AudioSource, ConnectionMode, DeviceId, DiscoveredDevice, JitterConfig, ProcessInfo,
        SenderCapabilities,
    };

    use crate::traits::{
        FrontendNotifier, InterfaceInfo, NetworkInfoProvider, PlatformService, SenderControlClient,
        SenderControlClientFactory, SessionInfo, SessionManager, SessionParams,
    };

    // -------------------------------------------------------------------
    // FrontendEvent + MockFrontendNotifier
    // -------------------------------------------------------------------

    #[allow(dead_code)]
    #[derive(Debug, Clone)]
    pub enum FrontendEvent {
        SenderDiscovered(DiscoveredDevice),
        SenderTimeout(DeviceId),
        ForceDisconnect,
        LinkLost,
        LinkRecovered { device_registered: Option<bool> },
        LinkRecoveryGaveUp,
        SenderConnected(String),
        AudioTelemetry { latency: f32, is_active: bool },
        PlaybackError(String),
        WsDisconnect,
        WsError(String),
        ServiceCommand(String),
    }

    /// Records every frontend event for later assertion.
    pub struct MockFrontendNotifier {
        pub events: Mutex<Vec<FrontendEvent>>,
    }

    impl MockFrontendNotifier {
        pub fn new() -> Self {
            Self {
                events: Mutex::new(Vec::new()),
            }
        }

        pub fn take_events(&self) -> Vec<FrontendEvent> {
            self.events.lock().unwrap().drain(..).collect()
        }
    }

    impl FrontendNotifier for MockFrontendNotifier {
        fn emit_sender_discovered(&self, device: DiscoveredDevice) {
            self.events
                .lock()
                .unwrap()
                .push(FrontendEvent::SenderDiscovered(device));
        }

        fn emit_sender_timeout(&self, sender_id: &DeviceId) {
            self.events
                .lock()
                .unwrap()
                .push(FrontendEvent::SenderTimeout(sender_id.clone()));
        }

        fn emit_force_disconnect(&self) {
            self.events
                .lock()
                .unwrap()
                .push(FrontendEvent::ForceDisconnect);
        }

        fn emit_link_lost(&self) {
            self.events.lock().unwrap().push(FrontendEvent::LinkLost);
        }

        fn emit_link_recovered(&self, device_registered: Option<bool>) {
            self.events
                .lock()
                .unwrap()
                .push(FrontendEvent::LinkRecovered { device_registered });
        }

        fn emit_link_recovery_gave_up(&self) {
            self.events
                .lock()
                .unwrap()
                .push(FrontendEvent::LinkRecoveryGaveUp);
        }

        fn emit_sender_connected(&self, ip: String) {
            self.events
                .lock()
                .unwrap()
                .push(FrontendEvent::SenderConnected(ip));
        }

        fn emit_audio_telemetry(&self, latency: f32, is_active: bool) {
            self.events
                .lock()
                .unwrap()
                .push(FrontendEvent::AudioTelemetry { latency, is_active });
        }

        fn emit_playback_error(&self, error: String) {
            self.events
                .lock()
                .unwrap()
                .push(FrontendEvent::PlaybackError(error));
        }

        fn emit_ws_disconnect(&self) {
            self.events
                .lock()
                .unwrap()
                .push(FrontendEvent::WsDisconnect);
        }

        fn emit_ws_error(&self, message: String) {
            self.events
                .lock()
                .unwrap()
                .push(FrontendEvent::WsError(message));
        }

        fn emit_service_command(&self, command: String) {
            self.events
                .lock()
                .unwrap()
                .push(FrontendEvent::ServiceCommand(command));
        }
    }

    // -------------------------------------------------------------------
    // SessionCall + MockSessionManager
    // -------------------------------------------------------------------

    #[allow(dead_code)]
    #[derive(Debug, Clone)]
    pub enum SessionCall {
        StartSession {
            mode: ConnectionMode,
            exclusive_mode: bool,
            is_tcp: bool,
        },
        StopSession,
        SetPlaying {
            playing: bool,
        },
        PausePlayback,
        ResumePlayback,
        UpdateJitterConfig,
        SessionInfo,
        UpdateBitrate {
            bitrate: Option<i32>,
        },
        StartWsClient,
        StopWsClient,
    }

    /// Records every session lifecycle call for later assertion.
    pub struct MockSessionManager {
        pub calls: Mutex<Vec<SessionCall>>,
        start_result: Mutex<Result<(), String>>,
        session_info_value: Mutex<Option<SessionInfo>>,
    }

    impl MockSessionManager {
        pub fn new() -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                start_result: Mutex::new(Ok(())),
                session_info_value: Mutex::new(None),
            }
        }

        pub fn with_session_info(self, info: SessionInfo) -> Self {
            *self.session_info_value.lock().unwrap() = Some(info);
            self
        }

        #[allow(dead_code)]
        pub fn with_start_error(self, error: String) -> Self {
            *self.start_result.lock().unwrap() = Err(error);
            self
        }

        pub fn take_calls(&self) -> Vec<SessionCall> {
            self.calls.lock().unwrap().drain(..).collect()
        }
    }

    #[async_trait]
    impl SessionManager for MockSessionManager {
        async fn start_session(&self, params: SessionParams) -> Result<(), String> {
            self.calls.lock().unwrap().push(SessionCall::StartSession {
                mode: params.mode,
                exclusive_mode: params.exclusive_mode,
                is_tcp: params.is_tcp,
            });
            self.start_result.lock().unwrap().clone()
        }

        async fn stop_session(&self) {
            self.calls.lock().unwrap().push(SessionCall::StopSession);
        }

        async fn set_playing(&self, playing: bool) {
            self.calls
                .lock()
                .unwrap()
                .push(SessionCall::SetPlaying { playing });
        }

        async fn pause_playback(&self) -> Result<(), String> {
            self.calls.lock().unwrap().push(SessionCall::PausePlayback);
            Ok(())
        }

        async fn resume_playback(&self) -> Result<(), String> {
            self.calls.lock().unwrap().push(SessionCall::ResumePlayback);
            Ok(())
        }

        async fn update_jitter_config(&self, _config: JitterConfig) {
            self.calls
                .lock()
                .unwrap()
                .push(SessionCall::UpdateJitterConfig);
        }

        async fn session_info(&self) -> Option<SessionInfo> {
            self.calls.lock().unwrap().push(SessionCall::SessionInfo);
            self.session_info_value.lock().unwrap().clone()
        }

        async fn update_bitrate(&self, bitrate: Option<i32>) {
            self.calls
                .lock()
                .unwrap()
                .push(SessionCall::UpdateBitrate { bitrate });
        }

        async fn start_ws_client(&self, task: tokio::task::JoinHandle<()>) {
            task.abort(); // don't run anything in tests
            self.calls.lock().unwrap().push(SessionCall::StartWsClient);
        }

        async fn stop_ws_client(&self) {
            self.calls.lock().unwrap().push(SessionCall::StopWsClient);
        }

        async fn set_volume(&self, _linear: f32) {
            // No-op in tests
        }
    }

    // -------------------------------------------------------------------
    // ControlClientCall + MockSenderControlClient
    // -------------------------------------------------------------------

    #[allow(dead_code)]
    #[derive(Debug, Clone)]
    pub enum ControlClientCall {
        Connect {
            device_id: DeviceId,
        },
        Disconnect {
            device_id: DeviceId,
        },
        GetAudioSources,
        Probe {
            device_id: Option<DeviceId>,
        },
        ChangeSource {
            device_id: DeviceId,
            source: AudioSource,
        },
        ChangeBitrate {
            device_id: DeviceId,
            bitrate: Option<i32>,
        },
        GetProcessList,
    }

    /// Records every HTTP control call for later assertion.
    pub struct MockSenderControlClient {
        pub calls: Mutex<Vec<ControlClientCall>>,
        connect_result: Mutex<Result<(), String>>,
        disconnect_result: Mutex<Result<(), String>>,
        change_bitrate_result: Mutex<Result<(), String>>,
        /// Number of probes still to be failed before one succeeds.
        /// `u32::MAX` stands for "never succeeds".
        probe_failures: Mutex<u32>,
        probe_device_registered: Mutex<Option<bool>>,
    }

    impl MockSenderControlClient {
        pub fn new() -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                connect_result: Mutex::new(Ok(())),
                disconnect_result: Mutex::new(Ok(())),
                change_bitrate_result: Mutex::new(Ok(())),
                probe_failures: Mutex::new(0),
                probe_device_registered: Mutex::new(Some(true)),
            }
        }

        /// Fail the first `n` probes, then succeed. Models a PC that comes back.
        pub fn with_probe_failures(self, n: u32) -> Self {
            *self.probe_failures.lock().unwrap() = n;
            self
        }

        /// Fail every probe. Models a PC that never comes back.
        pub fn with_unreachable_probe(self) -> Self {
            *self.probe_failures.lock().unwrap() = u32::MAX;
            self
        }

        /// What a successful probe reports for `device_registered`.
        pub fn with_probe_registration(self, registered: Option<bool>) -> Self {
            *self.probe_device_registered.lock().unwrap() = registered;
            self
        }

        #[allow(dead_code)]
        pub fn with_connect_error(self, err: String) -> Self {
            *self.connect_result.lock().unwrap() = Err(err);
            self
        }

        pub fn with_change_bitrate_error(self, err: String) -> Self {
            *self.change_bitrate_result.lock().unwrap() = Err(err);
            self
        }

        pub fn take_calls(&self) -> Vec<ControlClientCall> {
            self.calls.lock().unwrap().drain(..).collect()
        }
    }

    #[async_trait]
    impl SenderControlClient for MockSenderControlClient {
        async fn connect(&self, req: ConnectReq) -> Result<PresenceResponse, String> {
            self.calls.lock().unwrap().push(ControlClientCall::Connect {
                device_id: req.device_id.clone(),
            });
            self.connect_result
                .lock()
                .unwrap()
                .clone()
                .map(|_| PresenceResponse {
                    device_id: DeviceId("test-sender".to_string()),
                    sender_name: "Test Sender".to_string(),
                    is_offline: false,
                    pc_network_link: None,
                    device_registered: Some(true),
                    session_token: None,
                    session_generation: None,
                    pending_request_id: None,
                    device_auth_challenge: None,
                    pc_certificate_fingerprint: None,
                })
        }

        async fn disconnect(&self, device_id: DeviceId) -> Result<(), String> {
            self.calls
                .lock()
                .unwrap()
                .push(ControlClientCall::Disconnect {
                    device_id: device_id.clone(),
                });
            self.disconnect_result.lock().unwrap().clone()
        }

        async fn get_audio_sources(
            &self,
        ) -> Result<(Vec<AudioSource>, SenderCapabilities), String> {
            self.calls
                .lock()
                .unwrap()
                .push(ControlClientCall::GetAudioSources);
            Ok((
                vec![],
                SenderCapabilities {
                    supports_process_capture: false,
                },
            ))
        }

        async fn probe(&self, device_id: Option<DeviceId>) -> Result<PresenceResponse, String> {
            self.calls.lock().unwrap().push(ControlClientCall::Probe {
                device_id: device_id.clone(),
            });

            {
                let mut remaining = self.probe_failures.lock().unwrap();
                if *remaining > 0 {
                    // u32::MAX is the "never succeeds" sentinel, so don't spend it.
                    if *remaining != u32::MAX {
                        *remaining -= 1;
                    }
                    return Err("connection refused".to_string());
                }
            }

            Ok(PresenceResponse {
                device_id: DeviceId("test-sender".to_string()),
                sender_name: "Test Sender".to_string(),
                is_offline: false,
                pc_network_link: None,
                device_registered: *self.probe_device_registered.lock().unwrap(),
                session_token: None,
                session_generation: None,
                pending_request_id: None,
                device_auth_challenge: None,
                pc_certificate_fingerprint: None,
            })
        }

        async fn change_source(
            &self,
            device_id: DeviceId,
            source: AudioSource,
        ) -> Result<(), String> {
            self.calls
                .lock()
                .unwrap()
                .push(ControlClientCall::ChangeSource { device_id, source });
            Ok(())
        }

        async fn change_bitrate(
            &self,
            device_id: DeviceId,
            bitrate: Option<i32>,
        ) -> Result<(), String> {
            self.calls
                .lock()
                .unwrap()
                .push(ControlClientCall::ChangeBitrate { device_id, bitrate });
            self.change_bitrate_result.lock().unwrap().clone()
        }

        async fn get_process_list(&self) -> Result<Vec<ProcessInfo>, String> {
            self.calls
                .lock()
                .unwrap()
                .push(ControlClientCall::GetProcessList);
            Ok(vec![])
        }
    }

    /// Factory that returns a shared mock client, so all calls are recorded
    /// in one place regardless of how many times `create()` is called.
    pub struct MockSenderControlClientFactory {
        pub client: Arc<MockSenderControlClient>,
    }

    impl MockSenderControlClientFactory {
        pub fn new(client: Arc<MockSenderControlClient>) -> Self {
            Self { client }
        }
    }

    impl SenderControlClientFactory for MockSenderControlClientFactory {
        fn create(&self, _ip: IpAddr) -> Arc<dyn SenderControlClient> {
            self.client.clone()
        }
    }

    // -------------------------------------------------------------------
    // PlatformCall + MockPlatformService
    // -------------------------------------------------------------------

    #[allow(dead_code)]
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum PlatformCall {
        GetTransportType,
        DevicePublicKey,
        SignDeviceAuth,
        PairedPcIds,
        ForgetPcIdentity {
            pc_id: DeviceId,
        },
        SyncService {
            state: crate::traits::PlaybackState,
            is_exclusive: bool,
        },
        SetStreamingFlag {
            active: bool,
        },
    }

    /// Records every platform call and returns configurable results.
    pub struct MockPlatformService {
        pub calls: Mutex<Vec<PlatformCall>>,
        transport_type: Mutex<Result<String, String>>,
        paired_pc_ids: Mutex<Result<Vec<DeviceId>, String>>,
    }

    impl MockPlatformService {
        pub fn new() -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                transport_type: Mutex::new(Err("not android".to_string())),
                paired_pc_ids: Mutex::new(Ok(Vec::new())),
            }
        }

        pub fn with_transport_type(self, transport: &str) -> Self {
            *self.transport_type.lock().unwrap() = Ok(transport.to_string());
            self
        }

        pub fn with_paired_pc_ids(self, ids: Vec<DeviceId>) -> Self {
            *self.paired_pc_ids.lock().unwrap() = Ok(ids);
            self
        }

        pub fn take_calls(&self) -> Vec<PlatformCall> {
            self.calls.lock().unwrap().drain(..).collect()
        }
    }

    impl PlatformService for MockPlatformService {
        fn get_transport_type(&self) -> Result<String, String> {
            self.calls
                .lock()
                .unwrap()
                .push(PlatformCall::GetTransportType);
            self.transport_type.lock().unwrap().clone()
        }

        fn sync_service(&self, state: crate::traits::PlaybackState, is_exclusive: bool) {
            self.calls.lock().unwrap().push(PlatformCall::SyncService {
                state,
                is_exclusive,
            });
        }

        fn device_public_key(&self) -> Result<String, String> {
            self.calls
                .lock()
                .unwrap()
                .push(PlatformCall::DevicePublicKey);
            Err("test device identity is not configured".into())
        }

        fn sign_device_auth(&self, _transcript: &[u8]) -> Result<String, String> {
            self.calls
                .lock()
                .unwrap()
                .push(PlatformCall::SignDeviceAuth);
            Err("test device identity is not configured".into())
        }

        fn trusted_pc_fingerprint(&self, _pc_id: &DeviceId) -> Result<Option<String>, String> {
            Ok(None)
        }

        fn paired_pc_ids(&self) -> Result<Vec<DeviceId>, String> {
            self.calls.lock().unwrap().push(PlatformCall::PairedPcIds);
            self.paired_pc_ids.lock().unwrap().clone()
        }

        fn confirm_pc_identity(
            &self,
            _pc_id: &DeviceId,
            _pc_name: &str,
            _fingerprint: &str,
            _pairing_code: &str,
            _requires_approval: bool,
        ) -> Result<bool, String> {
            Ok(true)
        }

        fn remember_pc_identity(
            &self,
            _pc_id: &DeviceId,
            _fingerprint: &str,
        ) -> Result<(), String> {
            Ok(())
        }

        fn forget_pc_identity(&self, pc_id: &DeviceId) -> Result<(), String> {
            self.calls
                .lock()
                .unwrap()
                .push(PlatformCall::ForgetPcIdentity {
                    pc_id: pc_id.clone(),
                });
            Ok(())
        }

        fn set_streaming_flag(&self, active: bool) {
            self.calls
                .lock()
                .unwrap()
                .push(PlatformCall::SetStreamingFlag { active });
        }
    }

    // -------------------------------------------------------------------
    // MockNetworkInfoProvider
    // -------------------------------------------------------------------

    /// Returns configurable network info for testing.
    pub struct MockNetworkInfoProvider {
        local_ip: Mutex<Result<IpAddr, String>>,
        default_interface: Mutex<Result<InterfaceInfo, String>>,
        interfaces: Mutex<Vec<InterfaceInfo>>,
    }

    impl MockNetworkInfoProvider {
        pub fn new() -> Self {
            Self {
                local_ip: Mutex::new(Ok("192.168.1.100".parse().unwrap())),
                default_interface: Mutex::new(Err("no default interface".to_string())),
                interfaces: Mutex::new(Vec::new()),
            }
        }

        pub fn with_default_interface(self, iface: InterfaceInfo) -> Self {
            *self.default_interface.lock().unwrap() = Ok(iface);
            self
        }

        pub fn with_interfaces(self, interfaces: Vec<InterfaceInfo>) -> Self {
            *self.interfaces.lock().unwrap() = interfaces;
            self
        }
    }

    impl NetworkInfoProvider for MockNetworkInfoProvider {
        fn get_local_ip(&self) -> Result<IpAddr, String> {
            self.local_ip.lock().unwrap().clone()
        }

        fn get_default_interface(&self) -> Result<InterfaceInfo, String> {
            self.default_interface.lock().unwrap().clone()
        }

        fn get_interfaces(&self) -> Vec<InterfaceInfo> {
            self.interfaces.lock().unwrap().clone()
        }
    }
}
