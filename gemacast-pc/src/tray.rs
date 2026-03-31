use std::collections::HashMap;

use tray_icon::{
    TrayIcon, TrayIconBuilder,
    menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu},
};

pub struct TrayManager {
    _tray_icon: TrayIcon,
    pub device_buttons: HashMap<String, CheckMenuItem>,
    pub devices_submenu: Submenu,
    pub scanning_placeholder: MenuItem,
    pub quit_item: MenuItem,
    pub active_devices: std::collections::HashSet<String>,
}

impl TrayManager {
    pub fn new() -> Self {
        let tray_menu = Menu::new();

        let devices_submenu = Submenu::new("Available Phones", true);
        let scanning_placeholder = MenuItem::new("Scanning for devices...", false, None);
        let quit_item = MenuItem::new("quit", true, None);

        let _ = devices_submenu.append(&scanning_placeholder);
        let _ = tray_menu.append(&devices_submenu);
        let _ = tray_menu.append(&PredefinedMenuItem::separator());
        let _ = tray_menu.append(&quit_item);

        let tray_icon = TrayIconBuilder::new()
            .with_menu(Box::new(tray_menu))
            .with_tooltip("Gemacast")
            .build()
            .unwrap();

        Self {
            _tray_icon: tray_icon,
            device_buttons: HashMap::new(),
            scanning_placeholder,
            devices_submenu,
            quit_item,
            active_devices: std::collections::HashSet::new(),
        }
    }

    pub fn add_device(&mut self, device_id: String, device_name: &str, addr: std::net::SocketAddr) {
        if self.device_buttons.contains_key(&device_id) {
            return;
        }

        if self.device_buttons.is_empty() {
            let _ = self.devices_submenu.remove(&self.scanning_placeholder);
        }

        let display_text = format!("{} ({})", device_name, addr.ip());
        let new_device = CheckMenuItem::new(display_text, true, false, None);

        let _ = self.devices_submenu.append(&new_device);
        self.device_buttons.insert(device_id, new_device);
    }

    pub fn remove_device(&mut self, device_id: &str) {
        if let Some(device) = self.device_buttons.remove(device_id) {
            let _ = self.devices_submenu.remove(&device);
        }

        self.active_devices.remove(device_id);

        if self.device_buttons.is_empty() {
            let _ = self.devices_submenu.append(&self.scanning_placeholder);
        }
    }
    pub fn toggle_active_device(&mut self, clicked_device_id: &str) -> bool {
        let is_turning_off = self.active_devices.contains(clicked_device_id);

        if is_turning_off {
            self.active_devices.remove(clicked_device_id);
            if let Some(item) = self.device_buttons.get(clicked_device_id) {
                item.set_checked(false);
            }
            false
        } else {
            self.active_devices.insert(clicked_device_id.to_string());
            if let Some(item) = self.device_buttons.get(clicked_device_id) {
                item.set_checked(true);
            }
            true
        }
    }
}
