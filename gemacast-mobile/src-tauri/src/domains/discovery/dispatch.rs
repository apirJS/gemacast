use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tauri::Emitter;

use gemacast_core::types::{
    ConnectionMode, DiscoveredDevice, SenderId, TransportType,
};

use crate::SENDER_HEARTBEAT_TIMEOUT_SECS;

pub struct DispatchContext {
    pub last_seen: Arc<Mutex<HashMap<SenderId, Instant>>>,
    pub active_usb_ips: Arc<Mutex<HashMap<SenderId, Instant>>>,
    pub app_handle: tauri::AppHandle,
}

impl Clone for DispatchContext {
    fn clone(&self) -> Self {
        Self {
            last_seen: self.last_seen.clone(),
            active_usb_ips: self.active_usb_ips.clone(),
            app_handle: self.app_handle.clone(),
        }
    }
}

impl DispatchContext {
    pub fn new(app_handle: tauri::AppHandle) -> Self {
        Self {
            last_seen: Arc::new(Mutex::new(HashMap::new())),
            active_usb_ips: Arc::new(Mutex::new(HashMap::new())),
            app_handle,
        }
    }

    pub fn dispatch(
        &self,
        message: gemacast_core::types::ControlMessage,
        addr: std::net::SocketAddr,
        mode: ConnectionMode,
    ) {
        match message {
            gemacast_core::types::ControlMessage::Presence {
                sender_id,
                sender_name,
                is_offline,
                transport,
            } => {
                self.handle_presence(
                    sender_id,
                    sender_name,
                    is_offline,
                    transport,
                    addr,
                    mode,
                );
            }
            gemacast_core::types::ControlMessage::Disconnect { .. } => {
                let _ = self.app_handle.emit("force-disconnect", ());
            }
            _ => {}
        }
    }

    fn handle_presence(
        &self,
        sender_id: SenderId,
        sender_name: String,
        is_offline: bool,
        transport: Option<TransportType>,
        addr: std::net::SocketAddr,
        mode: ConnectionMode,
    ) {
        if is_offline {
            self.last_seen.lock().unwrap().remove(&sender_id);
            self.active_usb_ips.lock().unwrap().remove(&sender_id);
        } else {
            self.last_seen
                .lock()
                .unwrap()
                .insert(sender_id.clone(), Instant::now());
        }

        let mut audio_addr = addr;
        audio_addr.set_port(gemacast_core::network::Ports::AUDIO_UDP);

        let mut is_usb = transport == Some(TransportType::Usb);
        if !is_usb {
            is_usb = gemacast_core::network::is_usb_tether_ip(&addr.ip());
        }

        match mode {
            ConnectionMode::Wifi => {
                if is_usb {
                    return;
                }
            }
            ConnectionMode::Usb => {
                if !is_usb {
                    return;
                }
            }
            ConnectionMode::Adb => {
                if !audio_addr.ip().is_loopback() {
                    return;
                }
            }
        }

        if !is_offline {
            if is_usb {
                self.active_usb_ips
                    .lock()
                    .unwrap()
                    .insert(sender_id.clone(), Instant::now());
            } else {
                let has_recent_usb = self
                    .active_usb_ips
                    .lock()
                    .unwrap()
                    .get(&sender_id)
                    .is_some_and(|ts| {
                        Instant::now().duration_since(*ts).as_secs()
                            < SENDER_HEARTBEAT_TIMEOUT_SECS
                    });
                if has_recent_usb {
                    return;
                }
            }
        }

        let device = DiscoveredDevice::from_presence(
            sender_id,
            sender_name,
            is_offline,
            audio_addr,
            transport,
        );
        let _ = self.app_handle.emit("sender-discovered", device);
    }
}
