use tauri::State;

use gemacast_core::types::{ConnectionMode, DeviceId};

#[cfg(target_os = "android")]
use super::native::call_native_transport_check;
use crate::state::{lock, AppState};

use super::listener::spawn_discovery_listener;

/// Returns the device's primary non-loopback IPv4 address as a string.
#[tauri::command]
pub fn get_local_ip() -> Result<String, String> {
    gemacast_core::network::get_local_ip()
        .map(|ip| ip.to_string())
        .map_err(|e| e.to_string())
}

/// Returns a composite identifier for the current default network interface
/// explicitly meant to catch when we stay on the same IP but the interface/network bounces.
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

/// Starts the discovery listener, replacing any currently running one.
///
/// Spawns the full discovery pipeline (listener + watchdog + USB probe)
/// and stores the task handle in [`AppState`].
#[tauri::command]
pub async fn start_listening_for_senders(
    device_id: DeviceId,
    mode: ConnectionMode,
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    if let Some(handle) = lock(&state.discovery_handle)?.take() {
        handle.abort();
    }

    let (discovery_tx, discovery_rx) = tokio::sync::mpsc::channel(8);
    let listener = gemacast_core::network::DiscoveryListener::new(discovery_tx)
        .await
        .map_err(|e| e.to_string())?;

    let handle = spawn_discovery_listener(listener, discovery_rx, app_handle, device_id, mode);
    *lock(&state.discovery_handle)? = Some(handle);
    Ok(())
}

/// Stops the currently running discovery listener, if any.
#[tauri::command]
pub async fn stop_listening_for_senders(state: State<'_, AppState>) -> Result<(), String> {
    if let Some(handle) = lock(&state.discovery_handle)?.take() {
        handle.abort();
    }
    Ok(())
}

/// Returns the currently available connection modes (Wifi, USB).
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

        let interfaces = netdev::get_interfaces();
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
