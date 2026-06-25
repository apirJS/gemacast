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
        })
        .await
}

#[tauri::command]
pub async fn disconnect_from_sender(
    ip: String,
    device_id: DeviceId,
    state: State<'_, AppState>,
) -> Result<(), String> {
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
    state.audio.kill_playback().await
}

#[tauri::command]
pub async fn start_audio_playback(
    ip: Option<String>,
    device_id: Option<DeviceId>,
    device_name: Option<String>,
    state: State<'_, AppState>,
) -> Result<(), String> {
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
    let ip_addr = ip
        .parse()
        .map_err(|e: std::net::AddrParseError| e.to_string())?;
    state.audio.probe_sender(ip_addr, device_id).await
}

#[tauri::command]
pub async fn change_audio_source(
    ip: String,
    device_id: DeviceId,
    source: gemacast_core::domain::types::AudioSource,
    state: State<'_, AppState>,
) -> Result<(), String> {
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
    let ip_addr = sender_ip
        .parse()
        .map_err(|e: std::net::AddrParseError| e.to_string())?;
    state.audio.establish_websocket(ip_addr, device_id).await
}

#[tauri::command]
pub async fn set_audio_gain(gain_db: f32, state: State<'_, AppState>) -> Result<(), String> {
    // Convert dB to linear multiplier: 10^(dB/20)
    // Clamp to safe range: -24 dB (0.063) to +12 dB (3.98)
    let clamped_db = gain_db.clamp(-24.0, 12.0);
    let linear = 10f32.powf(clamped_db / 20.0);
    state.audio.set_volume(linear).await
}
