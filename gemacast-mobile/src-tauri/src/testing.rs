//! Hand-written mock implementations for unit testing.
//!
//! Each mock records calls in a `Mutex<Vec<..>>` so tests can assert
//! what was called and with which arguments. Mirrors the pattern from

pub mod mocks {
    use std::net::IpAddr;
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use gemacast_core::control::types::{ConnectReq, PresenceResponse};
    use gemacast_core::types::{
        AudioSource, ConnectionMode, DeviceId, DiscoveredDevice, JitterConfig, ProcessInfo,
        SenderCapabilities,
    };

    use crate::traits::{
        FrontendNotifier, InterfaceInfo, NetworkInfoProvider, PlatformService,
        SenderControlClient, SenderControlClientFactory, SessionInfo, SessionManager,
        SessionParams,
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
            self.calls
                .lock()
                .unwrap()
                .push(SessionCall::StartWsClient);
        }

        async fn stop_ws_client(&self) {
            self.calls
                .lock()
                .unwrap()
                .push(SessionCall::StopWsClient);
        }
    }

    // -------------------------------------------------------------------
    // ControlClientCall + MockSenderControlClient
    // -------------------------------------------------------------------

    #[allow(dead_code)]
    #[derive(Debug, Clone)]
    pub enum ControlClientCall {
        Connect { device_id: DeviceId },
        Disconnect { device_id: DeviceId },
        GetAudioSources,
        Probe { device_id: Option<DeviceId> },
        ChangeSource { device_id: DeviceId, source: AudioSource },
        ChangeBitrate { device_id: DeviceId, bitrate: Option<i32> },
        GetProcessList,
    }

    /// Records every HTTP control call for later assertion.
    pub struct MockSenderControlClient {
        pub calls: Mutex<Vec<ControlClientCall>>,
        connect_result: Mutex<Result<(), String>>,
        disconnect_result: Mutex<Result<(), String>>,
    }

    impl MockSenderControlClient {
        pub fn new() -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                connect_result: Mutex::new(Ok(())),
                disconnect_result: Mutex::new(Ok(())),
            }
        }

        #[allow(dead_code)]
        pub fn with_connect_error(self, err: String) -> Self {
            *self.connect_result.lock().unwrap() = Err(err);
            self
        }

        pub fn take_calls(&self) -> Vec<ControlClientCall> {
            self.calls.lock().unwrap().drain(..).collect()
        }
    }

    #[async_trait]
    impl SenderControlClient for MockSenderControlClient {
        async fn connect(&self, req: ConnectReq) -> Result<(), String> {
            self.calls
                .lock()
                .unwrap()
                .push(ControlClientCall::Connect {
                    device_id: req.device_id.clone(),
                });
            self.connect_result.lock().unwrap().clone()
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
            self.calls
                .lock()
                .unwrap()
                .push(ControlClientCall::Probe {
                    device_id: device_id.clone(),
                });
            Ok(PresenceResponse {
                device_id: DeviceId("test-sender".to_string()),
                sender_name: "Test Sender".to_string(),
                is_offline: false,
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
            Ok(())
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
    #[derive(Debug, Clone)]
    pub enum PlatformCall {
        GetTransportType,
        SyncService { is_playing: bool, is_exclusive: bool },
        SetStreamingFlag { active: bool },
    }

    /// Records every platform call and returns configurable results.
    pub struct MockPlatformService {
        pub calls: Mutex<Vec<PlatformCall>>,
        transport_type: Mutex<Result<String, String>>,
    }

    impl MockPlatformService {
        pub fn new() -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                transport_type: Mutex::new(Err("not android".to_string())),
            }
        }

        pub fn with_transport_type(self, transport: &str) -> Self {
            *self.transport_type.lock().unwrap() = Ok(transport.to_string());
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

        fn sync_service(&self, is_playing: bool, is_exclusive: bool) {
            self.calls.lock().unwrap().push(PlatformCall::SyncService {
                is_playing,
                is_exclusive,
            });
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
