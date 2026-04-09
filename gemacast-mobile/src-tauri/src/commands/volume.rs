/// Sets the PC sender's system volume over a `SetSystemVolume` control message.
#[tauri::command]
pub async fn set_remote_system_volume(
    ip: String,
    device_id: String,
    level: f32,
) -> Result<(), String> {
    let ip_addr = ip.parse::<std::net::IpAddr>().map_err(|e| e.to_string())?;
    gemacast_core::network::send_control_message(
        ip_addr,
        gemacast_core::types::ControlMessage::SetSystemVolume { device_id, level },
    )
    .await
    .map_err(|e| e.to_string())
}

/// Toggles mute on the PC sender over a `SetSystemMute` control message.
#[tauri::command]
pub async fn set_remote_system_mute(
    ip: String,
    device_id: String,
    muted: bool,
) -> Result<(), String> {
    let ip_addr = ip.parse::<std::net::IpAddr>().map_err(|e| e.to_string())?;
    gemacast_core::network::send_control_message(
        ip_addr,
        gemacast_core::types::ControlMessage::SetSystemMute { device_id, muted },
    )
    .await
    .map_err(|e| e.to_string())
}


