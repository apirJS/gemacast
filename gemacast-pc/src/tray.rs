use std::collections::HashMap;

use tray_icon::{
    TrayIcon, TrayIconBuilder,
    menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu},
};

pub struct TrayManager {
    _tray_icon: TrayIcon,
    pub device_buttons: HashMap<String, CheckMenuItem>,
    pub devices_submenu: Submenu,
    pub quality_buttons: Vec<(Option<i32>, CheckMenuItem)>,
    pub scanning_placeholder: MenuItem,
    pub broadcast_toggle: CheckMenuItem,
    pub quit_item: MenuItem,
}

impl TrayManager {
    pub fn new() -> Self {
        let tray_menu = Menu::new();

        let broadcast_toggle = CheckMenuItem::new("Broadcast Presence", true, true, None);
        let devices_submenu = Submenu::new("Connected Phones", true);
        let scanning_placeholder = MenuItem::new("No devices connected yet", false, None);
        let quit_item = MenuItem::new("quit", true, None);

        let _ = devices_submenu.append(&scanning_placeholder);

        let qualities = vec![
            (10, "VoIP"),
            (24, "VoIP"),
            (32, "VoIP"),
            (64, "Standard"),
            (96, "Standard"),
            (128, "High (Default)"),
            (256, "High"),
            (450, "Very High"),
            (512, "Very High"),
            (-1, "Raw PCM"),
        ];

        let quality_submenu = Submenu::new("Audio Quality", true);
        let mut quality_buttons = Vec::new();
        for (kbps, category) in qualities {
            let label = if kbps == -1 {
                format!("Uncompressed - {}", category)
            } else {
                format!("{} Kb/s - {}", kbps, category)
            };
            
            let is_checked = kbps == 128;
            let item = CheckMenuItem::new(label, true, is_checked, None);
            
            let bitrate_val = if kbps == -1 { None } else { Some(kbps * 1000) };
            let _ = quality_submenu.append(&item);
            quality_buttons.push((bitrate_val, item));
        }

        let _ = tray_menu.append(&broadcast_toggle);
        let _ = tray_menu.append(&PredefinedMenuItem::separator());
        let _ = tray_menu.append(&quality_submenu);
        let _ = tray_menu.append(&devices_submenu);
        let _ = tray_menu.append(&PredefinedMenuItem::separator());
        let _ = tray_menu.append(&quit_item);

        let tray_icon = TrayIconBuilder::new()
            .with_menu(Box::new(tray_menu))
            .with_tooltip("Gemacast")
            .build()
            .expect("failed to build tray icon");

        Self {
            _tray_icon: tray_icon,
            device_buttons: HashMap::new(),
            quality_buttons,
            scanning_placeholder,
            devices_submenu,
            broadcast_toggle,
            quit_item,
        }
    }

    pub fn add_device(&mut self, device_id: String, device_name: &str, addr: std::net::SocketAddr) {
        if self.device_buttons.contains_key(&device_id) {
            return;
        }

        if self.device_buttons.is_empty() {
            let _ = self.devices_submenu.remove(&self.scanning_placeholder);
        }

        let connection_type = if gemacast_core::network::is_usb_tether_ip(&addr.ip()) {
            "USB"
        } else {
            "WIFI"
        };
        let display_text = format!("{} ({}) [{}]", device_name, addr.ip(), connection_type);
        let new_device = CheckMenuItem::new(display_text, true, false, None);

        let _ = self.devices_submenu.append(&new_device);
        self.device_buttons.insert(device_id, new_device);
    }

    pub fn set_device_connected(&self, device_id: &str, connected: bool) {
        if let Some(item) = self.device_buttons.get(device_id) {
            item.set_checked(connected);
        }
    }

    pub fn remove_device(&mut self, device_id: &str) {
        if let Some(device) = self.device_buttons.remove(device_id) {
            let _ = self.devices_submenu.remove(&device);
        }

        if self.device_buttons.is_empty() {
            let _ = self.devices_submenu.append(&self.scanning_placeholder);
        }
    }
}
