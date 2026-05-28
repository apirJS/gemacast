use std::collections::HashMap;

use gemacast_core::types::{DeviceId, TransportType};
use tray_icon::{
    TrayIcon, TrayIconBuilder,
    menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu},
};

pub struct TrayManager {
    _tray_icon: TrayIcon,
    pub device_menu_items: HashMap<DeviceId, CheckMenuItem>,
    pub connected_devices_submenu: Submenu,
    pub no_devices_placeholder: MenuItem,
    pub broadcast_toggle_item: MenuItem,
    pub quit_menu_item: MenuItem,
}

impl TrayManager {
    pub fn new() -> Result<Self, tray_icon::Error> {
        let tray_menu = Menu::new();
        let broadcast_toggle_item = MenuItem::new("Stop Stream", true, None);
        let connected_devices_submenu = Submenu::new("Connected Phones", true);
        let no_devices_placeholder = MenuItem::new("No devices connected yet", false, None);
        let quit_menu_item = MenuItem::new("quit", true, None);

        let _ = connected_devices_submenu.append(&no_devices_placeholder);

        let _ = tray_menu.append(&broadcast_toggle_item);
        let _ = tray_menu.append(&PredefinedMenuItem::separator());
        let _ = tray_menu.append(&connected_devices_submenu);
        let _ = tray_menu.append(&PredefinedMenuItem::separator());
        let _ = tray_menu.append(&quit_menu_item);

        let tray_icon = TrayIconBuilder::new()
            .with_menu(Box::new(tray_menu))
            .with_tooltip("Gemacast")
            .build()?;

        Ok(Self {
            _tray_icon: tray_icon,
            device_menu_items: HashMap::new(),
            no_devices_placeholder,
            connected_devices_submenu,
            broadcast_toggle_item,
            quit_menu_item,
        })
    }

    pub fn add_device(
        &mut self,
        device_id: DeviceId,
        device_name: &str,
        addr: std::net::SocketAddr,
        transport: Option<TransportType>,
    ) {
        let connection_type_label = if addr.ip().is_loopback() {
            "ADB"
        } else {
            match transport {
                Some(TransportType::Usb) => "USB",
                Some(TransportType::Wifi) => "WIFI",
                Some(TransportType::Adb) => "ADB",
                None => "WIFI",
            }
        };
        let display_text = format!(
            "{} ({}) [{}]",
            device_name,
            addr.ip(),
            connection_type_label
        );
        if let Some(existing) = self.device_menu_items.get(&device_id) {
            existing.set_text(&display_text);
            return;
        }

        if self.device_menu_items.is_empty() {
            let _ = self
                .connected_devices_submenu
                .remove(&self.no_devices_placeholder);
        }

        let new_device_item = CheckMenuItem::new(display_text, true, false, None);

        let _ = self.connected_devices_submenu.append(&new_device_item);
        self.device_menu_items.insert(device_id, new_device_item);
    }

    pub fn set_device_connected(&self, device_id: &DeviceId, connected: bool) {
        if let Some(item) = self.device_menu_items.get(device_id) {
            item.set_checked(connected);
        }
    }

    pub fn remove_device(&mut self, device_id: &DeviceId) {
        if let Some(device) = self.device_menu_items.remove(device_id) {
            let _ = self.connected_devices_submenu.remove(&device);
        }

        if self.device_menu_items.is_empty() {
            let _ = self
                .connected_devices_submenu
                .append(&self.no_devices_placeholder);
        }
    }
}
