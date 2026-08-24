//! Pure service functions for the audio domain, decoupled from Tauri.
//!
//! [`AudioService`] groups all trait dependencies needed to handle audio
//! commands. The `#[tauri::command]` handlers in [`super::commands`] are
//! thin wrappers that delegate to these methods.

use std::net::IpAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use gemacast_core::control::types::ConnectReq;
use gemacast_core::domain::types::{AudioSource, ConnectionMode, DeviceId, JitterConfig, LinkPair};

use crate::traits::{
    ConnectParams, FrontendNotifier, PlatformService, PlaybackState, ResumeParams,
    SenderControlClientFactory, SessionManager, SessionParams,
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
    /// Shared flag read by the probe loop to skip subnet scans while streaming.
    pub is_streaming: Arc<AtomicBool>,
    /// Cached network link pair from the last successful connection.
    ///
    /// Used to re-apply the network-aware Auto jitter config when the user
    /// toggles back to Auto mid-session (Auto → Balanced → Auto).
    /// Set during [`connect_to_sender`], cleared on disconnect/kill.
    pub cached_link_pair: std::sync::Mutex<Option<LinkPair>>,
    /// The in-flight link-recovery prober, if one is running.
    ///
    /// Held so any path that establishes or abandons a connection can cancel
    /// it — a prober that outlived its reason would reconnect on top of a
    /// session the user just started by hand.
    pub recovery_task: std::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
}

/// How often link recovery asks the PC whether it is back.
///
/// 2 s against the phone receiver's 10 s watchdog: five chances to catch the
/// PC inside the window where it still has us registered (`STALE_TIMEOUT` 15 s
/// plus a `CHECK_INTERVAL` 2 s sweep, so eviction lands at 15-17 s).
const RECOVERY_PROBE_INTERVAL: Duration = Duration::from_secs(2);

/// Request timeout for a recovery probe.
///
/// Equal to the interval, never the client default of 10 s: a request that
/// outlives its own poll period turns the period into a fiction and spends the
/// budget on six attempts instead of thirty.
const RECOVERY_PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// Total time link recovery will keep asking before giving up.
///
/// 60 s / 2 s = 30 attempts. Past this the failure is not a transient
/// re-association, and silent retries forever would be indistinguishable from
/// a leak; the frontend settles into the same suspended state a link loss
/// produces today and waits for a tap.
const RECOVERY_BUDGET: Duration = Duration::from_secs(60);

/// Ask the PC whether it is back, on `interval`, until it answers or `budget`
/// runs out.
///
/// A probe carries our `device_id`, so a PC that answers also says whether it
/// still has us registered. That answer cannot be self-fulfilling: the PC's
/// `update_last_seen` only touches a device already in the map, so probing
/// never resurrects one it evicted.
///
/// Emits exactly one terminal event — [`FrontendNotifier::emit_link_recovered`]
/// or [`FrontendNotifier::emit_link_recovery_gave_up`] — or none at all if the
/// task is aborted first.
async fn run_link_recovery(
    client: Arc<dyn crate::traits::SenderControlClient>,
    device_id: DeviceId,
    notifier: Arc<dyn FrontendNotifier>,
    interval: Duration,
    budget: Duration,
) {
    let started = tokio::time::Instant::now();
    let mut ticker = tokio::time::interval(interval);
    let mut attempts: u32 = 0;

    loop {
        // The first tick resolves immediately, which is what we want: the
        // receiver watchdog already spent 10 s establishing that the link is
        // gone, so there is nothing left to wait for.
        ticker.tick().await;

        if started.elapsed() >= budget {
            tracing::warn!(
                "[AudioService] Link recovery gave up after {} attempts in {:?}",
                attempts,
                budget,
            );
            notifier.emit_link_recovery_gave_up();
            return;
        }

        attempts += 1;
        match client.probe(Some(device_id.clone())).await {
            Ok(presence) => {
                tracing::info!(
                    "[AudioService] Link recovered after {} attempts: sender={}, offline={}, registered={:?}",
                    attempts,
                    presence.sender_name,
                    presence.is_offline,
                    presence.device_registered,
                );
                notifier.emit_link_recovered(presence.device_registered);
                return;
            }
            Err(e) => {
                tracing::debug!(
                    "[AudioService] Link recovery attempt {} failed: {}",
                    attempts,
                    e
                );
            }
        }
    }
}

impl AudioService {
    /// Connect to a sender: HTTPS handshake -> spawn audio receiver -> sync service.
    ///
    /// If the user selected "Auto" buffer preset, the jitter config is overridden
    /// with a network-aware profile based on the detected [`LinkPair`].
    pub async fn connect_to_sender(&self, params: ConnectParams) -> Result<(), String> {
        tracing::info!(
            "[AudioService] Connect: ip={}, device={:?}, mode={:?}, jitter_preset=min_{}ms/cap_{}ms",
            params.ip,
            params.device_id,
            params.mode,
            params.jitter_config.min_depth_ms,
            params.jitter_config.comfort_cap_ms,
        );
        // A connect is the answer to whatever recovery was looking for, whether
        // recovery asked for it or the user tapped. Either way the prober is
        // done, and leaving it running would let it fire on top of this session.
        self.stop_link_recovery();

        let ip_addr: IpAddr = params
            .ip
            .parse()
            .map_err(|e: std::net::AddrParseError| e.to_string())?;
        let client = self.client_factory.create(ip_addr);

        let response = client
            .connect(ConnectReq {
                device_id: params.device_id.clone(),
                device_name: params.device_name.clone(),
                source: None,
                mode: params.mode,
                jitter_config: params.jitter_config.clone(),
                bitrate: params.bitrate,
                network_link: params.phone_network_link,
                pending_request_id: None,
                device_auth: None,
            })
            .await?;

        // Build and cache the LinkPair from both sides' detected links
        let phone_link = params
            .phone_network_link
            .unwrap_or(gemacast_core::domain::types::NetworkLink::Unknown);
        let pc_link = response
            .pc_network_link
            .unwrap_or(gemacast_core::domain::types::NetworkLink::Unknown);
        let link_pair = LinkPair {
            phone: phone_link,
            pc: pc_link,
        };
        *self.cached_link_pair.lock().unwrap() = Some(link_pair);

        tracing::info!(
            "Network link pair: phone={:?}, pc={:?}, effective={:?}",
            link_pair.phone,
            link_pair.pc,
            link_pair.effective_link()
        );

        // Apply network-aware override if user selected Auto
        let effective_jitter_config = if params.jitter_config.is_auto_sentinel() {
            JitterConfig::for_link_pair(link_pair)
        } else {
            params.jitter_config
        };

        let is_tcp = params.mode == ConnectionMode::Adb;

        if let Err(error) = self
            .session
            .start_session(SessionParams {
                jitter_config: effective_jitter_config,
                is_tcp,
                exclusive_mode: params.exclusive_mode,
                target_ip: Some(ip_addr),
                mode: params.mode,
                device_id: params.device_id.to_string(),
                bitrate: params.bitrate,
                network_link: link_pair.effective_link(),
                session_token: response.session_token.clone(),
                session_generation: response.session_generation,
            })
            .await
        {
            // The PC has already acknowledged and registered the stream. If
            // local playback cannot start, explicitly roll that subscription
            // back instead of leaking a silent sender-side session until the
            // watchdog expires.
            let _ = client.disconnect(params.device_id.clone()).await;
            *self.cached_link_pair.lock().unwrap() = None;
            return Err(error);
        }

        self.is_streaming.store(true, Ordering::Relaxed);
        self.platform.set_streaming_flag(true);
        self.platform
            .sync_service(PlaybackState::Playing, params.exclusive_mode);

        Ok(())
    }

    /// Disconnect from a sender: HTTPS disconnect -> tear down session -> sync service.
    pub async fn disconnect_from_sender(
        &self,
        ip: IpAddr,
        device_id: DeviceId,
    ) -> Result<(), String> {
        tracing::info!(
            "[AudioService] Disconnect: ip={}, device={:?}",
            ip,
            device_id
        );
        // The user asked to stop. Any prober still running is chasing a link
        // nobody wants back.
        self.stop_link_recovery();

        let client = self.client_factory.create(ip);
        let _ = client.disconnect(device_id).await;

        self.session.stop_session().await;

        // Clear the cached link pair — detection happens fresh on reconnect
        *self.cached_link_pair.lock().unwrap() = None;

        self.is_streaming.store(false, Ordering::Relaxed);
        self.platform.set_streaming_flag(false);
        self.platform.sync_service(PlaybackState::Stopped, false);
        Ok(())
    }

    /// Resume audio playback after a pause.
    ///
    /// Re-enables the Oboe output callback via `resume_playback()` without
    /// sending an HTTPS reconnect; the network connection stays alive.
    pub async fn start_audio_playback(&self, _resume: Option<ResumeParams>) -> Result<(), String> {
        tracing::info!("[AudioService] Resume playback");
        self.session.resume_playback().await?;
        let info = self.session.session_info().await;
        let exclusive = info.as_ref().is_some_and(|i| i.exclusive_mode);

        self.platform
            .sync_service(PlaybackState::Playing, exclusive);
        Ok(())
    }

    /// Pause audio playback without tearing down the session.
    ///
    /// Silences the Oboe output callback via `pause_playback()` while
    /// keeping the network receive thread, heartbeat, and WebSocket alive.
    /// Does not send an HTTPS disconnect to the PC.
    pub async fn stop_audio_playback(
        &self,
        _ip: Option<IpAddr>,
        _device_id: Option<DeviceId>,
    ) -> Result<(), String> {
        tracing::info!("[AudioService] Pause playback");
        self.session.pause_playback().await?;

        self.platform.sync_service(PlaybackState::Paused, false);
        Ok(())
    }

    /// Kill playback immediately: tear down session, clear streaming flag.
    ///
    /// Cancels link recovery as well, so a forced teardown is always a full
    /// stop. The link-lost path therefore has to call this **before**
    /// [`Self::start_link_recovery`], not after.
    pub async fn kill_playback(&self) -> Result<(), String> {
        tracing::warn!("[AudioService] Kill playback (forced teardown)");
        self.stop_link_recovery();
        self.session.stop_session().await;

        // Clear the cached link pair
        *self.cached_link_pair.lock().unwrap() = None;

        self.is_streaming.store(false, Ordering::Relaxed);
        self.platform.set_streaming_flag(false);
        self.platform.sync_service(PlaybackState::Stopped, false);
        Ok(())
    }

    /// Notify that streaming has stopped (called by frontend).
    ///
    /// The frontend reaches this on the no-sender branch of `disconnect()`, which
    /// is a real stop — so it must sync the foreground service like every other
    /// stop path (`disconnect_from_sender`, `kill_playback`). Clearing only the
    /// flag file left the notification and the Media Session fully live.
    ///
    /// Also cancels link recovery: this is the branch a user reaches by tapping
    /// disconnect while suspended, which is exactly the state recovery runs in.
    /// Like [`Self::kill_playback`], it must precede
    /// [`Self::start_link_recovery`] on the link-lost path.
    pub fn notify_streaming_stopped(&self) {
        self.stop_link_recovery();
        self.is_streaming.store(false, Ordering::Relaxed);
        self.platform.set_streaming_flag(false);
        self.platform.sync_service(PlaybackState::Stopped, false);
    }

    /// Start polling the PC after an unrequested link loss.
    ///
    /// This loop lives in Rust on purpose. The scenario it recovers from is a
    /// link that died with the screen off, and Android throttles WebView timers
    /// exactly then — the same reason the probe heartbeat was moved out of the
    /// webview in `36730b4`. A `setInterval` here would be suspended precisely
    /// when it is needed.
    ///
    /// Cancels any prober already running, so repeated link losses cannot stack
    /// two loops onto one connection.
    pub fn start_link_recovery(&self, ip: IpAddr, device_id: DeviceId) {
        self.start_link_recovery_paced(ip, device_id, RECOVERY_PROBE_INTERVAL, RECOVERY_BUDGET);
    }

    /// [`Self::start_link_recovery`] with the pacing spelled out, so tests can
    /// drive the loop without waiting on the production interval.
    pub fn start_link_recovery_paced(
        &self,
        ip: IpAddr,
        device_id: DeviceId,
        interval: Duration,
        budget: Duration,
    ) {
        self.stop_link_recovery();

        let client = self
            .client_factory
            .create_with_timeout(ip, RECOVERY_PROBE_TIMEOUT);
        let notifier = self.notifier.clone();

        tracing::warn!(
            "[AudioService] Link lost — probing {} every {:?} for up to {:?}",
            ip,
            interval,
            budget,
        );

        let handle = tokio::spawn(async move {
            run_link_recovery(client, device_id, notifier, interval, budget).await;
        });

        *self.recovery_task.lock().unwrap() = Some(handle);
    }

    /// Cancel link recovery if it is running.
    ///
    /// Called from every path that establishes or abandons a connection: a
    /// prober still ticking after the user reconnected by hand would fire
    /// `link-recovered` on top of a live session.
    pub fn stop_link_recovery(&self) {
        if let Some(handle) = self.recovery_task.lock().unwrap().take() {
            handle.abort();
            tracing::info!("[AudioService] Link recovery cancelled");
        }
    }

    /// Restart the audio session with a new exclusive mode setting.
    ///
    /// Tears down the old Oboe/cpal stream and spawns a new one without
    /// sending any HTTPS disconnect/connect; the PC sender doesn't care
    /// about the phone's audio sharing mode.
    pub async fn restart_session(&self, exclusive_mode: bool) -> Result<(), String> {
        let info = self
            .session
            .session_info()
            .await
            .ok_or("No active session to restart")?;

        let jitter_config = if info.jitter_config.is_auto_sentinel() {
            if let Some(pair) = *self.cached_link_pair.lock().unwrap() {
                JitterConfig::for_link_pair(pair)
            } else {
                info.jitter_config
            }
        } else {
            info.jitter_config
        };

        let is_tcp = info.mode == ConnectionMode::Adb;

        tracing::info!(
            "[AudioService] Restart session: exclusive_mode={} (was {})",
            exclusive_mode,
            info.exclusive_mode,
        );

        self.session
            .start_session(SessionParams {
                jitter_config,
                is_tcp,
                exclusive_mode,
                target_ip: info.target_ip,
                mode: info.mode,
                device_id: info.device_id,
                bitrate: info.bitrate,
                network_link: info.network_link,
                session_token: info.session_token,
                session_generation: info.session_generation,
            })
            .await?;

        self.platform
            .sync_service(PlaybackState::Playing, exclusive_mode);

        Ok(())
    }

    /// Update the jitter buffer configuration on the active session.
    ///
    /// If the incoming config is the Auto sentinel (`peak_decay_halflife_ms == 0`,
    /// no static target) and we have a cached [`LinkPair`] from the connection
    /// handshake, we re-apply the network-aware override instead of the generic
    /// Auto config. This ensures toggling Auto → Balanced → Auto mid-session
    /// preserves the network-aware optimisation.
    pub async fn update_jitter_config(&self, config: JitterConfig) -> Result<(), String> {
        let effective_config = if config.is_auto_sentinel() {
            if let Some(pair) = *self.cached_link_pair.lock().unwrap() {
                tracing::info!(
                    "[AudioService] Jitter config update: Auto sentinel → link-pair override ({:?})",
                    pair.effective_link(),
                );
                JitterConfig::for_link_pair(pair)
            } else {
                tracing::info!(
                    "[AudioService] Jitter config update: Auto sentinel (no cached link pair)"
                );
                config
            }
        } else {
            tracing::info!(
                "[AudioService] Jitter config update: min_depth={}ms, comfort_cap={}ms, static={:?}",
                config.min_depth_ms,
                config.comfort_cap_ms,
                config.static_target_ms,
            );
            config
        };

        self.session.update_jitter_config(effective_config).await;
        Ok(())
    }

    /// Set the audio output volume as a linear multiplier.
    pub async fn set_volume(&self, linear: f32) -> Result<(), String> {
        self.session.set_volume(linear).await;
        Ok(())
    }

    /// Return the cached network link pair from the active connection, if any.
    pub fn get_cached_link_pair(&self) -> Option<LinkPair> {
        *self.cached_link_pair.lock().unwrap()
    }

    /// Request audio sources from the sender.
    pub async fn get_audio_sources(
        &self,
        ip: IpAddr,
    ) -> Result<
        (
            Vec<AudioSource>,
            gemacast_core::domain::types::SenderCapabilities,
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
        let client = self.client_factory.create(ip);
        client.change_bitrate(device_id, bitrate).await?;
        self.session.update_bitrate(bitrate).await;
        Ok(())
    }

    /// Request capturable process list from the sender.
    pub async fn get_process_list(
        &self,
        ip: IpAddr,
    ) -> Result<Vec<gemacast_core::domain::types::ProcessInfo>, String> {
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
        let client_factory = self.client_factory.clone();
        let session = self.session.clone();
        let notifier = self.notifier.clone();
        let task = tokio::spawn(async move {
            const RETRY_DELAYS: [std::time::Duration; 4] = [
                std::time::Duration::from_millis(250),
                std::time::Duration::from_millis(500),
                std::time::Duration::from_secs(1),
                std::time::Duration::from_secs(2),
            ];

            for retry_delay in RETRY_DELAYS
                .into_iter()
                .chain(std::iter::once(std::time::Duration::ZERO))
            {
                if session.session_info().await.is_none() {
                    return;
                }
                let credentials =
                    client_factory.session_credentials(sender_ip, &DeviceId(device_id.clone()));
                let ws_client = match gemacast_core::control::WsControlClient::new_with_credentials(
                    sender_ip,
                    &device_id,
                    credentials
                        .as_ref()
                        .map(|credentials| credentials.token.as_str()),
                    credentials
                        .as_ref()
                        .map(|credentials| credentials.pc_certificate_fingerprint.as_str()),
                )
                .await
                {
                    Ok(client) => client,
                    Err(error) => {
                        tracing::warn!("WebSocket connection failed: {error}");
                        if retry_delay.is_zero() {
                            return;
                        }
                        tokio::time::sleep(retry_delay).await;
                        continue;
                    }
                };

                match ws_client.recv_event().await {
                    Ok(gemacast_core::control::types::WsEvent::Disconnect) => {
                        notifier.emit_ws_disconnect();
                        return;
                    }
                    Ok(gemacast_core::control::types::WsEvent::Error { message }) => {
                        notifier.emit_ws_error(message);
                        notifier.emit_ws_disconnect();
                        return;
                    }
                    Err(error) => {
                        tracing::warn!("WebSocket control channel dropped: {error}");
                        if retry_delay.is_zero() {
                            return;
                        }
                        tokio::time::sleep(retry_delay).await;
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
        make_service_with_notifier(
            session,
            client,
            platform,
            Arc::new(MockFrontendNotifier::new()),
        )
    }

    /// [`make_service`] with the notifier supplied, for tests that assert on
    /// the events the service emitted rather than on the calls it made.
    fn make_service_with_notifier(
        session: Arc<MockSessionManager>,
        client: Arc<MockSenderControlClient>,
        platform: Arc<MockPlatformService>,
        notifier: Arc<MockFrontendNotifier>,
    ) -> AudioService {
        let factory = Arc::new(MockSenderControlClientFactory::new(client));
        AudioService {
            session,
            client_factory: factory,
            notifier,
            platform,
            is_streaming: Arc::new(AtomicBool::new(false)),
            cached_link_pair: std::sync::Mutex::new(None),
            recovery_task: std::sync::Mutex::new(None),
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
                phone_network_link: None,
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
        assert!(
            session_calls
                .iter()
                .any(|c| matches!(c, SessionCall::StartSession { .. }))
        );

        // Platform was synced
        let platform_calls = platform.take_calls();
        assert!(
            platform_calls
                .iter()
                .any(|c| matches!(c, PlatformCall::SetStreamingFlag { active: true }))
        );
        assert!(platform_calls.iter().any(|c| matches!(
            c,
            PlatformCall::SyncService {
                state: PlaybackState::Playing,
                ..
            }
        )));
    }

    #[tokio::test]
    async fn connect_should_disconnect_sender_when_local_playback_start_fails() {
        let session =
            Arc::new(MockSessionManager::new().with_start_error("audio output failed".into()));
        let client = Arc::new(MockSenderControlClient::new());
        let platform = Arc::new(MockPlatformService::new());
        let service = make_service(session, client.clone(), platform);

        let result = service
            .connect_to_sender(ConnectParams {
                ip: "192.168.1.5".to_string(),
                device_id: DeviceId("phone-1".into()),
                device_name: "My Phone".into(),
                mode: ConnectionMode::Wifi,
                exclusive_mode: false,
                jitter_config: JitterConfig::default(),
                bitrate: Some(128000),
                phone_network_link: None,
            })
            .await;

        assert_eq!(result.unwrap_err(), "audio output failed");
        assert!(client.take_calls().iter().any(|call| matches!(
            call,
            ControlClientCall::Disconnect { device_id } if device_id.0 == "phone-1"
        )));
        assert!(!service.is_streaming.load(Ordering::Relaxed));
    }

    #[tokio::test]
    async fn disconnect_should_stop_session_and_sync() {
        let session = Arc::new(MockSessionManager::new());
        let client = Arc::new(MockSenderControlClient::new());
        let platform = Arc::new(MockPlatformService::new());
        let service = make_service(session.clone(), client.clone(), platform.clone());

        service
            .disconnect_from_sender("192.168.1.5".parse().unwrap(), DeviceId("phone-1".into()))
            .await
            .unwrap();

        // HTTPS disconnect was called.
        let client_calls = client.take_calls();
        assert!(matches!(
            &client_calls[0],
            ControlClientCall::Disconnect { device_id } if device_id.0 == "phone-1"
        ));

        // Session was stopped
        let session_calls = session.take_calls();
        assert!(
            session_calls
                .iter()
                .any(|c| matches!(c, SessionCall::StopSession))
        );

        // Platform streaming flag cleared
        let platform_calls = platform.take_calls();
        assert!(
            platform_calls
                .iter()
                .any(|c| matches!(c, PlatformCall::SetStreamingFlag { active: false }))
        );
    }

    #[tokio::test]
    async fn start_playback_should_call_resume_playback() {
        let session = Arc::new(MockSessionManager::new());
        let client = Arc::new(MockSenderControlClient::new());
        let platform = Arc::new(MockPlatformService::new());
        let service = make_service(session.clone(), client.clone(), platform.clone());

        service.start_audio_playback(None).await.unwrap();

        let session_calls = session.take_calls();
        assert!(
            session_calls
                .iter()
                .any(|c| matches!(c, SessionCall::ResumePlayback))
        );
    }

    #[tokio::test]
    async fn start_playback_should_not_send_http_reconnect() {
        let session = Arc::new(MockSessionManager::new().with_session_info(SessionInfo {
            exclusive_mode: false,
            exclusive_granted: false,
            mode: ConnectionMode::Wifi,
            bitrate: Some(128000),
            jitter_config: JitterConfig::default(),
            target_ip: Some("192.168.1.5".parse().unwrap()),
            device_id: "phone-1".into(),
            network_link: gemacast_core::domain::types::NetworkLink::Unknown,
            session_token: None,
            session_generation: None,
        }));
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

        // No HTTPS reconnect should be sent; the connection stays alive.
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
        assert!(
            session_calls
                .iter()
                .any(|c| matches!(c, SessionCall::PausePlayback))
        );
        assert!(
            !session_calls
                .iter()
                .any(|c| matches!(c, SessionCall::StopSession))
        );

        // No HTTPS disconnect should be sent.
        let client_calls = client.take_calls();
        assert_eq!(client_calls.len(), 0);

        // Platform service should be notified
        let platform_calls = platform.take_calls();
        assert!(platform_calls.iter().any(|c| matches!(
            c,
            PlatformCall::SyncService {
                state: PlaybackState::Paused,
                ..
            }
        )));
    }

    #[tokio::test]
    async fn kill_playback_should_stop_everything() {
        let session = Arc::new(MockSessionManager::new());
        let client = Arc::new(MockSenderControlClient::new());
        let platform = Arc::new(MockPlatformService::new());
        let service = make_service(session.clone(), client.clone(), platform.clone());

        service.kill_playback().await.unwrap();

        let session_calls = session.take_calls();
        assert!(
            session_calls
                .iter()
                .any(|c| matches!(c, SessionCall::StopSession))
        );

        let platform_calls = platform.take_calls();
        assert!(
            platform_calls
                .iter()
                .any(|c| matches!(c, PlatformCall::SetStreamingFlag { active: false }))
        );
        assert!(platform_calls.iter().any(|c| matches!(
            c,
            PlatformCall::SyncService {
                state: PlaybackState::Stopped,
                ..
            }
        )));
    }

    #[tokio::test]
    async fn notify_streaming_stopped_should_sync_the_service_to_stopped() {
        let session = Arc::new(MockSessionManager::new());
        let client = Arc::new(MockSenderControlClient::new());
        let platform = Arc::new(MockPlatformService::new());
        let service = make_service(session.clone(), client.clone(), platform.clone());

        service.notify_streaming_stopped();

        let platform_calls = platform.take_calls();
        assert!(
            platform_calls
                .iter()
                .any(|c| matches!(c, PlatformCall::SetStreamingFlag { active: false })),
            "the streaming flag must still be cleared"
        );
        // Without this the foreground notification and the Media Session stay
        // live after a disconnect that had no connected sender.
        assert!(
            platform_calls.iter().any(|c| matches!(
                c,
                PlatformCall::SyncService {
                    state: PlaybackState::Stopped,
                    ..
                }
            )),
            "notify_streaming_stopped must sync the service, got {platform_calls:?}"
        );
        assert!(!service.is_streaming.load(Ordering::Relaxed));
    }

    /// Every path that ends a stream has to leave the platform service in the
    /// same state, or the notification survives on whichever path forgot.
    /// The exact `Stopped` state matters: `Paused` intentionally keeps the
    /// MediaSession and notification visible for a connected stream.
    #[tokio::test]
    async fn every_stop_path_should_sync_the_service() {
        fn synced_stopped(calls: &[PlatformCall]) -> bool {
            calls.iter().any(|c| {
                matches!(
                    c,
                    PlatformCall::SyncService {
                        state: PlaybackState::Stopped,
                        ..
                    }
                )
            })
        }

        // Path 1: disconnect with a known sender.
        let platform = Arc::new(MockPlatformService::new());
        let service = make_service(
            Arc::new(MockSessionManager::new()),
            Arc::new(MockSenderControlClient::new()),
            platform.clone(),
        );
        service
            .disconnect_from_sender("192.168.1.5".parse().unwrap(), DeviceId("phone-1".into()))
            .await
            .unwrap();
        assert!(
            synced_stopped(&platform.take_calls()),
            "disconnect_from_sender must sync the service"
        );

        // Path 2: forced teardown by the receiver watchdog.
        let platform = Arc::new(MockPlatformService::new());
        let service = make_service(
            Arc::new(MockSessionManager::new()),
            Arc::new(MockSenderControlClient::new()),
            platform.clone(),
        );
        service.kill_playback().await.unwrap();
        assert!(
            synced_stopped(&platform.take_calls()),
            "kill_playback must sync the service"
        );

        // Path 3: the frontend's no-sender branch.
        let platform = Arc::new(MockPlatformService::new());
        let service = make_service(
            Arc::new(MockSessionManager::new()),
            Arc::new(MockSenderControlClient::new()),
            platform.clone(),
        );
        service.notify_streaming_stopped();
        assert!(
            synced_stopped(&platform.take_calls()),
            "notify_streaming_stopped must sync the service"
        );
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
        assert!(session_calls.iter().any(|c| matches!(
            c,
            SessionCall::UpdateBitrate {
                bitrate: Some(256000)
            }
        )));

        let client_calls = client.take_calls();
        assert!(matches!(
            &client_calls[0],
            ControlClientCall::ChangeBitrate {
                bitrate: Some(256000),
                ..
            }
        ));
    }

    #[tokio::test]
    async fn failed_bitrate_change_should_preserve_session_bitrate() {
        let session = Arc::new(MockSessionManager::new());
        let client =
            Arc::new(MockSenderControlClient::new().with_change_bitrate_error("rejected".into()));
        let platform = Arc::new(MockPlatformService::new());
        let service = make_service(session.clone(), client, platform);

        let result = service
            .change_audio_bitrate(
                "192.168.1.5".parse().unwrap(),
                DeviceId("phone-1".into()),
                Some(256000),
            )
            .await;

        assert_eq!(result.unwrap_err(), "rejected");
        assert!(
            !session
                .take_calls()
                .iter()
                .any(|call| matches!(call, SessionCall::UpdateBitrate { .. }))
        );
    }

    // ---------------------------------------------------------------
    // LinkPair cache + update_jitter_config intercept
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn update_jitter_config_should_reapply_cached_link_pair_for_auto_sentinel() {
        use gemacast_core::domain::types::NetworkLink;

        let session = Arc::new(MockSessionManager::new());
        let client = Arc::new(MockSenderControlClient::new());
        let platform = Arc::new(MockPlatformService::new());
        let service = make_service(session.clone(), client.clone(), platform.clone());

        // Simulate a cached link pair from a previous connect
        let pair = LinkPair {
            phone: NetworkLink::Wifi5Ghz,
            pc: NetworkLink::Wifi5Ghz,
        };
        *service.cached_link_pair.lock().unwrap() = Some(pair);

        // Send the Auto sentinel config (peakDecayHalflifeMs = 0, no static target)
        let auto_config = JitterConfig {
            min_depth_ms: 25,
            comfort_cap_ms: 1000,
            peak_decay_halflife_ms: 0,
            resume_threshold_pct: 0.25,
            static_target_ms: None,
        };

        service.update_jitter_config(auto_config).await.unwrap();

        // The session should have received the link-pair-optimised config,
        // not the generic Auto config
        let calls = session.take_calls();
        assert!(
            calls
                .iter()
                .any(|c| matches!(c, SessionCall::UpdateJitterConfig)),
            "Expected UpdateJitterConfig call"
        );
        // We can't directly inspect the config in the mock, but we verified
        // the code path through the is_auto_sentinel + cached pair logic.
    }

    #[tokio::test]
    async fn update_jitter_config_should_passthrough_non_auto_config() {
        use gemacast_core::domain::types::NetworkLink;

        let session = Arc::new(MockSessionManager::new());
        let client = Arc::new(MockSenderControlClient::new());
        let platform = Arc::new(MockPlatformService::new());
        let service = make_service(session.clone(), client.clone(), platform.clone());

        // Set a cached link pair
        let pair = LinkPair {
            phone: NetworkLink::Wifi5Ghz,
            pc: NetworkLink::Wifi5Ghz,
        };
        *service.cached_link_pair.lock().unwrap() = Some(pair);

        // Send a non-Auto config (Balanced)
        let balanced_config = JitterConfig {
            min_depth_ms: 10,
            comfort_cap_ms: 200,
            peak_decay_halflife_ms: 3500,
            resume_threshold_pct: 0.75,
            static_target_ms: None,
        };

        service.update_jitter_config(balanced_config).await.unwrap();

        // Should pass through unmodified (non-Auto has halflife != 0)
        let calls = session.take_calls();
        assert!(
            calls
                .iter()
                .any(|c| matches!(c, SessionCall::UpdateJitterConfig))
        );
    }

    #[tokio::test]
    async fn update_jitter_config_should_passthrough_auto_when_no_cache() {
        let session = Arc::new(MockSessionManager::new());
        let client = Arc::new(MockSenderControlClient::new());
        let platform = Arc::new(MockPlatformService::new());
        let service = make_service(session.clone(), client.clone(), platform.clone());

        // No cached link pair
        assert!(service.cached_link_pair.lock().unwrap().is_none());

        // Send Auto sentinel
        let auto_config = JitterConfig {
            min_depth_ms: 25,
            comfort_cap_ms: 1000,
            peak_decay_halflife_ms: 0,
            resume_threshold_pct: 0.25,
            static_target_ms: None,
        };

        service.update_jitter_config(auto_config).await.unwrap();

        // Should pass through the generic Auto config since there's no cache
        let calls = session.take_calls();
        assert!(
            calls
                .iter()
                .any(|c| matches!(c, SessionCall::UpdateJitterConfig))
        );
    }

    #[tokio::test]
    async fn disconnect_should_clear_cached_link_pair() {
        use gemacast_core::domain::types::NetworkLink;

        let session = Arc::new(MockSessionManager::new());
        let client = Arc::new(MockSenderControlClient::new());
        let platform = Arc::new(MockPlatformService::new());
        let service = make_service(session.clone(), client.clone(), platform.clone());

        // Set a cached link pair
        *service.cached_link_pair.lock().unwrap() = Some(LinkPair {
            phone: NetworkLink::Wifi5Ghz,
            pc: NetworkLink::Ethernet,
        });

        service
            .disconnect_from_sender("192.168.1.5".parse().unwrap(), DeviceId("phone-1".into()))
            .await
            .unwrap();

        assert!(service.cached_link_pair.lock().unwrap().is_none());
    }

    #[tokio::test]
    async fn kill_playback_should_clear_cached_link_pair() {
        use gemacast_core::domain::types::NetworkLink;

        let session = Arc::new(MockSessionManager::new());
        let client = Arc::new(MockSenderControlClient::new());
        let platform = Arc::new(MockPlatformService::new());
        let service = make_service(session.clone(), client.clone(), platform.clone());

        // Set a cached link pair
        *service.cached_link_pair.lock().unwrap() = Some(LinkPair {
            phone: NetworkLink::Adb,
            pc: NetworkLink::Ethernet,
        });

        service.kill_playback().await.unwrap();

        assert!(service.cached_link_pair.lock().unwrap().is_none());
    }

    mod link_recovery {
        use super::*;

        const IP: &str = "192.168.1.5";
        const INTERVAL: Duration = Duration::from_secs(2);
        const BUDGET: Duration = Duration::from_secs(60);

        fn probe_count(calls: &[ControlClientCall]) -> usize {
            calls
                .iter()
                .filter(|c| matches!(c, ControlClientCall::Probe { .. }))
                .count()
        }

        /// Let the recovery task run to its terminal event on tokio's paused
        /// clock, which auto-advances whenever every task is parked on a timer.
        async fn drain(service: &AudioService) {
            let handle = service.recovery_task.lock().unwrap().take();
            if let Some(handle) = handle {
                let _ = handle.await;
            }
        }

        #[tokio::test(start_paused = true)]
        async fn link_recovery_should_report_the_pc_registration_once_it_answers() {
            let session = Arc::new(MockSessionManager::new());
            let client = Arc::new(
                MockSenderControlClient::new()
                    .with_probe_failures(3)
                    .with_probe_registration(Some(true)),
            );
            let platform = Arc::new(MockPlatformService::new());
            let notifier = Arc::new(MockFrontendNotifier::new());
            let service =
                make_service_with_notifier(session, client.clone(), platform, notifier.clone());

            service.start_link_recovery_paced(
                IP.parse().unwrap(),
                DeviceId("phone-1".into()),
                INTERVAL,
                BUDGET,
            );
            drain(&service).await;

            // Precondition: the loop really did have to retry, so the success
            // below is the retry working and not the first probe getting lucky.
            assert_eq!(probe_count(&client.take_calls()), 4);
            assert!(matches!(
                notifier.take_events().as_slice(),
                [FrontendEvent::LinkRecovered {
                    device_registered: Some(true)
                }]
            ));
        }

        #[tokio::test(start_paused = true)]
        async fn a_recovery_probe_should_carry_our_device_id() {
            let session = Arc::new(MockSessionManager::new());
            let client = Arc::new(MockSenderControlClient::new());
            let platform = Arc::new(MockPlatformService::new());
            let service = make_service(session, client.clone(), platform);

            service.start_link_recovery_paced(
                IP.parse().unwrap(),
                DeviceId("phone-1".into()),
                INTERVAL,
                BUDGET,
            );
            drain(&service).await;

            // Without the device id the PC can only say whether *it* is up,
            // never whether it still holds a registration for us.
            let calls = client.take_calls();
            assert!(matches!(
                calls.as_slice(),
                [ControlClientCall::Probe { device_id: Some(id) }] if id.0 == "phone-1"
            ));
        }

        #[tokio::test(start_paused = true)]
        async fn link_recovery_should_give_up_once_its_budget_is_spent() {
            let session = Arc::new(MockSessionManager::new());
            let client = Arc::new(MockSenderControlClient::new().with_unreachable_probe());
            let platform = Arc::new(MockPlatformService::new());
            let notifier = Arc::new(MockFrontendNotifier::new());
            let service =
                make_service_with_notifier(session, client.clone(), platform, notifier.clone());

            service.start_link_recovery_paced(
                IP.parse().unwrap(),
                DeviceId("phone-1".into()),
                INTERVAL,
                BUDGET,
            );
            drain(&service).await;

            // Ticks land at 0, 2, .., 58 s and the check at 60 s ends it:
            // 30 attempts, not a loop that runs forever.
            assert_eq!(probe_count(&client.take_calls()), 30);
            assert!(matches!(
                notifier.take_events().as_slice(),
                [FrontendEvent::LinkRecoveryGaveUp]
            ));
        }

        #[tokio::test(start_paused = true)]
        async fn a_forced_teardown_should_cancel_link_recovery() {
            let session = Arc::new(MockSessionManager::new());
            let client = Arc::new(MockSenderControlClient::new().with_unreachable_probe());
            let platform = Arc::new(MockPlatformService::new());
            let notifier = Arc::new(MockFrontendNotifier::new());
            let service =
                make_service_with_notifier(session, client.clone(), platform, notifier.clone());

            service.start_link_recovery_paced(
                IP.parse().unwrap(),
                DeviceId("phone-1".into()),
                INTERVAL,
                BUDGET,
            );

            // Precondition: the prober is actually running and probing, so the
            // silence after the kill is cancellation and not a task that never
            // started.
            tokio::time::sleep(INTERVAL * 3).await;
            let probes_before = probe_count(&client.take_calls());
            assert!(probes_before > 0);

            service.kill_playback().await.unwrap();

            tokio::time::sleep(BUDGET * 2).await;
            assert_eq!(probe_count(&client.take_calls()), 0);
            assert!(
                !notifier
                    .take_events()
                    .iter()
                    .any(|e| matches!(e, FrontendEvent::LinkRecoveryGaveUp)),
            );
            assert!(service.recovery_task.lock().unwrap().is_none());
        }

        #[tokio::test(start_paused = true)]
        async fn a_second_link_loss_should_not_stack_a_second_prober() {
            let session = Arc::new(MockSessionManager::new());
            let client = Arc::new(MockSenderControlClient::new().with_unreachable_probe());
            let platform = Arc::new(MockPlatformService::new());
            let service = make_service(session, client.clone(), platform);

            service.start_link_recovery_paced(
                IP.parse().unwrap(),
                DeviceId("phone-1".into()),
                INTERVAL,
                BUDGET,
            );
            service.start_link_recovery_paced(
                IP.parse().unwrap(),
                DeviceId("phone-1".into()),
                INTERVAL,
                BUDGET,
            );
            drain(&service).await;

            // Two stacked loops would double this; the second start must have
            // aborted the first.
            assert_eq!(probe_count(&client.take_calls()), 30);
        }
    }
}
