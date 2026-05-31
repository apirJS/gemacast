use gemacast_core::types::{ConnectionMode, DeviceId};
use tauri::State;

#[cfg(target_os = "android")]
use super::native::call_native_transport_check;
use crate::state::AppState;

use super::listener::spawn_discovery_listener;

#[tauri::command]
pub fn get_local_ip() -> Result<String, String> {
    gemacast_core::network::get_local_ip()
        .map(|ip| ip.to_string())
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_network_identifier() -> Result<String, String> {
    let iface = netdev::get_default_interface().map_err(|e| e.to_string())?;

    let mac = iface
        .mac_addr
        .map(|m| m.to_string())
        .unwrap_or_else(|| "00:00:00:00:00:00".to_string());

    let ip = if let Some(ip) = iface.ipv4.first() {
        std::net::IpAddr::V4(ip.addr()).to_string()
    } else if let Some(ip) = iface.ipv6.first() {
        std::net::IpAddr::V6(ip.addr()).to_string()
    } else {
        "no-ip".to_string()
    };

    Ok(format!("{}_{}_{}", iface.name, mac, ip))
}

#[tauri::command]
pub async fn start_listening_for_senders(
    device_id: DeviceId,
    mode: ConnectionMode,
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    if let Some(handle) = state.discovery_task.lock().await.take() {
        handle.abort();
    }

    let (presence_message_tx, presence_message_rx) = tokio::sync::mpsc::channel(8);
    let listener = gemacast_core::network::PresenceListener::new(presence_message_tx)
        .await
        .map_err(|e| {
            let e_str = e.to_string();
            if e_str.contains("Address already in use")
                || e_str.contains("10048")
                || e_str.contains("98")
                || e_str.contains("WSAEADDRINUSE")
            {
                "Discovery port is already in use. Is GemaCast already running in the background?"
                    .to_string()
            } else {
                e_str
            }
        })?;

    let handle =
        spawn_discovery_listener(listener, presence_message_rx, app_handle, device_id, mode);
    *state.discovery_task.lock().await = Some(handle);
    Ok(())
}

#[tauri::command]
pub async fn stop_listening_for_senders(state: State<'_, AppState>) -> Result<(), String> {
    if let Some(handle) = state.discovery_task.lock().await.take() {
        handle.abort();
    }
    Ok(())
}

#[tauri::command]
pub async fn get_connection_status(
    _app: tauri::AppHandle,
) -> Result<gemacast_core::types::ConnectionModes, String> {
    let modes = gemacast_core::types::get_available_connection_modes();

    #[cfg(target_os = "android")]
    {
        let mut modes = modes;
        if let Ok(transport_str) = call_native_transport_check(&_app) {
            modes.wifi = false;
            modes.usb = false;

            let parts: Vec<&str> = transport_str.split('|').collect();
            let network_type = parts.get(0).unwrap_or(&"");
            let adb_status = parts.get(1).unwrap_or(&"");

            if *adb_status == "ADB_OFF" {
                modes.adb = false;
            }

            for transport in network_type.split(',') {
                match transport {
                    "WIFI" => {
                        modes.wifi = true;
                    }
                    "ETHERNET" => {
                        modes.usb = true;
                    }
                    _ => {}
                }
            }
        }

        let interfaces = tokio::task::spawn_blocking(|| netdev::get_interfaces())
            .await
            .unwrap_or_default();

        for iface in interfaces {
            let (is_wifi, is_usb) = gemacast_core::network::classify_interface(&iface);
            if is_wifi && !iface.ipv4.is_empty() {
                modes.wifi = true;
            }
            if is_usb && !iface.ipv4.is_empty() {
                modes.usb = true;
            }
        }

        Ok(modes)
    }

    #[cfg(not(target_os = "android"))]
    Ok(modes)
}

#[tauri::command]
pub async fn get_network_state(_app: tauri::AppHandle) -> Result<NetworkState, String> {
    let local_ip = gemacast_core::network::get_local_ip()
        .map(|ip| ip.to_string())
        .unwrap_or_else(|_| "127.0.0.1".to_string());

    let network_id = match netdev::get_default_interface() {
        Ok(iface) => {
            let mac = iface
                .mac_addr
                .map(|m| m.to_string())
                .unwrap_or_else(|| "00:00:00:00:00:00".to_string());
            let ip = if let Some(ip) = iface.ipv4.first() {
                std::net::IpAddr::V4(ip.addr()).to_string()
            } else if let Some(ip) = iface.ipv6.first() {
                std::net::IpAddr::V6(ip.addr()).to_string()
            } else {
                "no-ip".to_string()
            };
            format!("{}_{}_{}", iface.name, mac, ip)
        }
        Err(_) => local_ip.clone(),
    };

    let modes =
        get_connection_status(_app)
            .await
            .unwrap_or(gemacast_core::types::ConnectionModes {
                wifi: true,
                usb: false,
                adb: false,
            });

    Ok(NetworkState {
        local_ip,
        network_id,
        modes,
    })
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkState {
    pub local_ip: String,
    pub network_id: String,
    pub modes: gemacast_core::types::ConnectionModes,
}
