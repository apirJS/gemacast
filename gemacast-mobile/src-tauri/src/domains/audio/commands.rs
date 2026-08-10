//! Thin Tauri command wrappers that delegate to [`super::service::AudioService`].
//!
//! Each `#[tauri::command]` handler extracts the `AudioService` from
//! [`crate::state::AppState`] and forwards to the corresponding method.
//! No I/O or business logic lives here.

use crate::state::AppState;
use crate::traits::{ConnectParams, ResumeParams};
use gemacast_core::domain::types::{DeviceId, TransportType};
use tauri::State;

#[tauri::command]
pub fn notify_streaming_stopped(state: State<'_, AppState>) -> Result<(), String> {
    state.audio.notify_streaming_stopped();
    Ok(())
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn connect_to_sender(
    ip: String,
    device_id: DeviceId,
    device_name: String,
    mode: gemacast_core::domain::types::ConnectionMode,
    exclusive_mode: bool,
    jitter_config: gemacast_core::domain::types::JitterConfig,
    bitrate: Option<i32>,
    _transport: Option<TransportType>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    tracing::info!(
        "[Cmd] connect_to_sender: ip={}, device={:?}, mode={:?}, exclusive={}",
        ip,
        device_id,
        mode,
        exclusive_mode,
    );
    // Detect the phone's network link at connection time
    let mode_str = match mode {
        gemacast_core::domain::types::ConnectionMode::Adb => "adb",
        gemacast_core::domain::types::ConnectionMode::Usb => "usb",
        gemacast_core::domain::types::ConnectionMode::Wifi => "wifi",
    };
    let phone_link = crate::domains::discovery::service::detect_phone_link(
        state.network.as_ref(),
        state.platform.as_ref(),
        mode_str,
    );

    state
        .audio
        .connect_to_sender(ConnectParams {
            ip,
            device_id,
            device_name,
            mode,
            exclusive_mode,
            jitter_config,
            bitrate,
            phone_network_link: Some(phone_link),
        })
        .await
}

#[tauri::command]
pub async fn disconnect_from_sender(
    ip: String,
    device_id: DeviceId,
    state: State<'_, AppState>,
) -> Result<(), String> {
    tracing::info!(
        "[Cmd] disconnect_from_sender: ip={}, device={:?}",
        ip,
        device_id
    );
    let ip_addr = ip
        .parse()
        .map_err(|e: std::net::AddrParseError| e.to_string())?;
    state.audio.disconnect_from_sender(ip_addr, device_id).await
}

#[tauri::command]
pub async fn stop_audio_playback(
    ip: Option<String>,
    device_id: Option<DeviceId>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    tracing::info!(
        "[Cmd] stop_audio_playback: ip={:?}, device={:?}",
        ip,
        device_id
    );
    let ip_parsed = ip
        .map(|s| {
            s.parse()
                .map_err(|e: std::net::AddrParseError| e.to_string())
        })
        .transpose()?;
    state.audio.stop_audio_playback(ip_parsed, device_id).await
}

#[tauri::command]
pub async fn kill_playback(state: State<'_, AppState>) -> Result<(), String> {
    tracing::info!("[Cmd] kill_playback");
    state.audio.kill_playback().await
}

#[tauri::command]
pub async fn start_audio_playback(
    ip: Option<String>,
    device_id: Option<DeviceId>,
    device_name: Option<String>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    tracing::info!(
        "[Cmd] start_audio_playback: ip={:?}, device={:?}",
        ip,
        device_id
    );
    let resume = if let (Some(ip_str), Some(did), Some(dname)) = (ip, device_id, device_name) {
        let ip_addr = ip_str
            .parse()
            .map_err(|e: std::net::AddrParseError| e.to_string())?;
        Some(ResumeParams {
            ip: ip_addr,
            device_id: did,
            device_name: dname,
        })
    } else {
        None
    };
    state.audio.start_audio_playback(resume).await
}

#[tauri::command]
pub async fn update_jitter_config(
    jitter_config: gemacast_core::domain::types::JitterConfig,
    state: State<'_, AppState>,
) -> Result<(), String> {
    tracing::info!(
        "[Cmd] update_jitter_config: min_depth={}ms, cap={}ms, static={:?}",
        jitter_config.min_depth_ms,
        jitter_config.comfort_cap_ms,
        jitter_config.static_target_ms,
    );
    state.audio.update_jitter_config(jitter_config).await
}

#[tauri::command]
pub async fn get_audio_sources(
    ip: String,
    state: State<'_, AppState>,
) -> Result<
    (
        Vec<gemacast_core::domain::types::AudioSource>,
        gemacast_core::domain::types::SenderCapabilities,
    ),
    String,
> {
    let ip_addr = ip
        .parse()
        .map_err(|e: std::net::AddrParseError| e.to_string())?;
    state.audio.get_audio_sources(ip_addr).await
}

#[tauri::command]
pub async fn probe_sender(
    ip: String,
    device_id: DeviceId,
    state: State<'_, AppState>,
) -> Result<gemacast_core::control::types::PresenceResponse, String> {
    tracing::info!("[Cmd] probe_sender: ip={}, device={:?}", ip, device_id);
    let ip_addr = ip
        .parse()
        .map_err(|e: std::net::AddrParseError| e.to_string())?;
    state.audio.probe_sender(ip_addr, device_id).await
}

#[tauri::command]
pub async fn start_link_recovery(
    ip: String,
    device_id: DeviceId,
    state: State<'_, AppState>,
) -> Result<(), String> {
    tracing::info!(
        "[Cmd] start_link_recovery: ip={}, device={:?}",
        ip,
        device_id
    );
    let ip_addr = ip
        .parse()
        .map_err(|e: std::net::AddrParseError| e.to_string())?;
    state.audio.start_link_recovery(ip_addr, device_id);
    Ok(())
}

#[tauri::command]
pub async fn stop_link_recovery(state: State<'_, AppState>) -> Result<(), String> {
    tracing::info!("[Cmd] stop_link_recovery");
    state.audio.stop_link_recovery();
    Ok(())
}

#[tauri::command]
pub async fn change_audio_source(
    ip: String,
    device_id: DeviceId,
    source: gemacast_core::domain::types::AudioSource,
    state: State<'_, AppState>,
) -> Result<(), String> {
    tracing::info!(
        "[Cmd] change_audio_source: ip={}, device={:?}, source={:?}",
        ip,
        device_id,
        source
    );
    let ip_addr = ip
        .parse()
        .map_err(|e: std::net::AddrParseError| e.to_string())?;
    state
        .audio
        .change_audio_source(ip_addr, device_id, source)
        .await
}

#[tauri::command]
pub async fn change_audio_bitrate(
    ip: String,
    device_id: DeviceId,
    bitrate: Option<i32>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    tracing::info!(
        "[Cmd] change_audio_bitrate: ip={}, device={:?}, bitrate={:?}",
        ip,
        device_id,
        bitrate
    );
    let ip_addr = ip
        .parse()
        .map_err(|e: std::net::AddrParseError| e.to_string())?;
    state
        .audio
        .change_audio_bitrate(ip_addr, device_id, bitrate)
        .await
}

#[tauri::command]
pub async fn get_process_list(
    ip: String,
    state: State<'_, AppState>,
) -> Result<Vec<gemacast_core::domain::types::ProcessInfo>, String> {
    let ip_addr = ip
        .parse()
        .map_err(|e: std::net::AddrParseError| e.to_string())?;
    state.audio.get_process_list(ip_addr).await
}

#[tauri::command]
pub async fn establish_websocket(
    sender_ip: String,
    device_id: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    tracing::info!(
        "[Cmd] establish_websocket: ip={}, device={}",
        sender_ip,
        device_id
    );
    let ip_addr = sender_ip
        .parse()
        .map_err(|e: std::net::AddrParseError| e.to_string())?;
    state.audio.establish_websocket(ip_addr, device_id).await
}

#[tauri::command]
pub fn check_exclusive_support() -> bool {
    gemacast_core::stream::receiver::stream::probe_exclusive_support()
}

#[tauri::command]
pub async fn restart_session(
    exclusive_mode: bool,
    state: State<'_, AppState>,
) -> Result<(), String> {
    tracing::info!("[Cmd] restart_session: exclusive_mode={}", exclusive_mode,);
    state.audio.restart_session(exclusive_mode).await
}

#[tauri::command]
pub async fn set_audio_gain(gain_db: f32, state: State<'_, AppState>) -> Result<(), String> {
    tracing::info!("[Cmd] set_audio_gain: {}dB", gain_db);
    // Convert dB to linear multiplier: 10^(dB/20)
    // Clamp to safe range: -24 dB (0.063) to +12 dB (3.98)
    let clamped_db = gain_db.clamp(-24.0, 12.0);
    let linear = 10f32.powf(clamped_db / 20.0);
    state.audio.set_volume(linear).await
}

/// Response for the `get_network_link_pair` command.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkLinkPairInfo {
    pub phone: gemacast_core::domain::types::NetworkLink,
    pub pc: gemacast_core::domain::types::NetworkLink,
    pub effective: gemacast_core::domain::types::NetworkLink,
    /// Human-readable label for the effective link (e.g., "WiFi 5 GHz")
    pub effective_label: String,
}

fn network_link_label(link: gemacast_core::domain::types::NetworkLink) -> String {
    use gemacast_core::domain::types::NetworkLink;
    match link {
        NetworkLink::Adb => "ADB (localhost)".to_string(),
        NetworkLink::UsbTether => "USB Tether".to_string(),
        NetworkLink::Wifi5Ghz => "WiFi 5 GHz".to_string(),
        NetworkLink::Wifi2_4Ghz => "WiFi 2.4 GHz".to_string(),
        NetworkLink::Ethernet => "Ethernet".to_string(),
        NetworkLink::WifiUnknown => "WiFi".to_string(),
        NetworkLink::Unknown => "Unknown".to_string(),
    }
}

#[tauri::command]
pub fn get_network_link_pair(state: State<'_, AppState>) -> Option<NetworkLinkPairInfo> {
    state.audio.get_cached_link_pair().map(|pair| {
        let effective = pair.effective_link();
        NetworkLinkPairInfo {
            phone: pair.phone,
            pc: pair.pc,
            effective,
            effective_label: network_link_label(effective),
        }
    })
}
