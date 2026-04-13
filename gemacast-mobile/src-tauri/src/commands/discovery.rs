use tauri::State;

use crate::discovery::{spawn_discovery_listener, ConnectionMode};
use crate::state::{lock, AppState};

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
    device_id: String,
    mode: ConnectionMode,
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    if let Some(handle) = lock(&state.discovery_handle)?.take() {
        handle.abort();
    }

    let gemacast_core::network::DiscoveryListenerHandles {
        listener,
        discovery_rx,
    } = gemacast_core::network::DiscoveryListener::new()
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
) -> Result<gemacast_core::network::ConnectionModes, String> {
    let modes = gemacast_core::network::get_available_connection_modes();

    #[cfg(target_os = "android")]
    {
        let mut modes = modes;
        if let Ok(transport) = call_native_transport_check(&_app) {
            match transport.as_str() {
                "WIFI" => {
                    modes.wifi = true;
                }
                "ETHERNET" => {
                    modes.usb = true;
                }
                "CELLULAR" | "NONE" | "VPN" | "OTHER" => {
                    modes.wifi = false;

                    if !gemacast_core::network::is_usb_tether_ip(
                        &gemacast_core::network::get_local_ip()
                            .unwrap_or(std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED)),
                    ) {
                        modes.usb = false;
                    }
                }
                _ => {}
            }
        }
        Ok(modes)
    }

    #[cfg(not(target_os = "android"))]
    Ok(modes)
}

#[cfg(target_os = "android")]
fn call_native_transport_check(app: &tauri::AppHandle) -> Result<String, String> {
    use std::sync::mpsc;
    use tauri::Manager;

    let (tx, rx) = mpsc::channel();

    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "Failed to find main webview window".to_string())?;

    window
        .with_webview(move |webview| {
            #[cfg(target_os = "android")]
            {
                let tx = tx.clone();
                let _ = webview.jni_handle().exec(move |env, context, _webview| {
                    let result = (|| -> Result<String, String> {
                        let _class = env
                            .get_object_class(&context)
                            .map_err(|e| format!("Failed to get Activity class: {}", e))?;

                        let transport_obj = env
                            .call_method(&context, "getTransportType", "()Ljava/lang/String;", &[])
                            .map_err(|e| {
                                format!("Failed to call getTransportType on Activity: {}", e)
                            })?;

                        let transport_jstr = transport_obj
                            .l()
                            .map_err(|e| format!("Failed to get transport string object: {}", e))?;

                        let transport: String = env
                            .get_string(&transport_jstr.into())
                            .map_err(|e| format!("Failed to extract string from JNI: {}", e))?
                            .into();

                        Ok(transport)
                    })();

                    let _ = tx.send(result);
                });
            }
        })
        .map_err(|e| format!("WebView JNI execution failed: {}", e))?;

    rx.recv()
        .map_err(|e| format!("Failed to receive JNI result: {}", e))?
}
