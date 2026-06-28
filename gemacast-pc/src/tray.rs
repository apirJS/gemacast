//! System tray icon and menu management.
//!
//! [`TrayManager`] owns the system tray icon and dynamically updates its
//! context menu as devices connect and disconnect.

use std::collections::HashMap;

use gemacast_core::domain::types::{DeviceId, TransportType};
use tray_icon::{
    Icon, TrayIcon, TrayIconBuilder,
    menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu},
};

/// Load the tray icon from the embedded PNG.
fn load_icon() -> Result<Icon, Box<dyn std::error::Error>> {
    let image_bytes = include_bytes!("../../gemacast-mobile/src-tauri/icons/gemacast-pc.png");
    let image = image::load_from_memory(image_bytes)?.into_rgba8();
    let (width, height) = image.dimensions();
    let rgba = image.into_raw();
    let icon = Icon::from_rgba(rgba, width, height)?;
    Ok(icon)
}

/// Manages the system tray icon and its context menu.
///
/// The menu layout is:
/// ```text
/// ┌──────────────────────┐
/// │ Install Update (v…)  │  ← update_menu_item (only when ready)
/// │ ──────────────────── │
/// │ Stop Stream          │  ← broadcast_toggle_item
/// │ ──────────────────── │
/// │ Connected Phones ►   │  ← connected_devices_submenu
/// │   Phone 1 (IP) [WIFI]│    ← device_menu_items entries
/// │   Phone 2 (IP) [ADB] │
/// │ ──────────────────── │
/// │ Check for Updates    │  ← check_update_menu_item
/// │ quit                 │  ← quit_menu_item
/// └──────────────────────┘
/// ```
pub struct TrayManager {
    _tray_icon: TrayIcon,
    tray_menu: Menu,
    pub device_menu_items: HashMap<DeviceId, CheckMenuItem>,
    pub connected_devices_submenu: Submenu,
    pub no_devices_placeholder: MenuItem,
    pub broadcast_toggle_item: MenuItem,
    pub quit_menu_item: MenuItem,
    /// Manual "Check for Updates" menu item (always visible).
    pub check_update_menu_item: MenuItem,
    /// The "Install Update" menu item, shown only when a download is ready.
    pub update_menu_item: Option<MenuItem>,
    /// Separator placed after the update item (removed together with it).
    update_separator: Option<PredefinedMenuItem>,
    /// "Update check failed — click to retry" item (shown on failure).
    pub update_failed_menu_item: Option<MenuItem>,
    /// Separator placed after the failed item.
    update_failed_separator: Option<PredefinedMenuItem>,
}

impl TrayManager {
    /// Create the tray icon and initial menu.
    pub fn new() -> Result<Self, tray_icon::Error> {
        let tray_menu = Menu::new();
        let broadcast_toggle_item = MenuItem::new("Stop Stream", true, None);
        let connected_devices_submenu = Submenu::new("Connected Phones", true);
        let no_devices_placeholder = MenuItem::new("No devices connected yet", false, None);
        let check_update_menu_item = MenuItem::new("Check for Updates", true, None);
        let quit_menu_item = MenuItem::new("quit", true, None);

        let _ = connected_devices_submenu.append(&no_devices_placeholder);

        let _ = tray_menu.append(&broadcast_toggle_item);
        let _ = tray_menu.append(&PredefinedMenuItem::separator());
        let _ = tray_menu.append(&connected_devices_submenu);
        let _ = tray_menu.append(&PredefinedMenuItem::separator());
        let _ = tray_menu.append(&check_update_menu_item);
        let _ = tray_menu.append(&quit_menu_item);

        let mut builder = TrayIconBuilder::new()
            .with_menu(Box::new(tray_menu.clone()))
            .with_tooltip("Gemacast");

        let icon = load_icon().map_err(|e| {
            tray_icon::Error::OsError(std::io::Error::other(format!("Icon load failed: {e}")))
        })?;

        builder = builder.with_icon(icon);
        let tray_icon = builder.build()?;

        Ok(Self {
            _tray_icon: tray_icon,
            tray_menu,
            device_menu_items: HashMap::new(),
            no_devices_placeholder,
            connected_devices_submenu,
            broadcast_toggle_item,
            quit_menu_item,
            check_update_menu_item,
            update_menu_item: None,
            update_separator: None,
            update_failed_menu_item: None,
            update_failed_separator: None,
        })
    }

    /// Add or update a device in the "Connected Phones" submenu.
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

    /// Update the check mark for a device (checked = connected).
    pub fn set_device_connected(&self, device_id: &DeviceId, connected: bool) {
        if let Some(item) = self.device_menu_items.get(device_id) {
            item.set_checked(connected);
        }
    }

    /// Remove a device from the "Connected Phones" submenu.
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

    /// Find which device (if any) corresponds to a clicked menu item.
    pub fn find_device_by_menu_id(&self, menu_id: &tray_icon::menu::MenuId) -> Option<DeviceId> {
        for (device_id, menu_item) in &self.device_menu_items {
            if *menu_id == menu_item.id() {
                return Some(device_id.clone());
            }
        }
        None
    }

    /// Show the "Install Update" menu item at the very top of the tray menu.
    pub fn show_update_ready(&mut self, version: &str) {
        // Remove any previous update or failure items first.
        self.remove_update_item();
        self.remove_update_failed_item();

        let item = MenuItem::new(format!("Install Update (v{version})"), true, None);
        let sep = PredefinedMenuItem::separator();

        let _ = self.tray_menu.prepend(&sep);
        let _ = self.tray_menu.prepend(&item);

        self.update_menu_item = Some(item);
        self.update_separator = Some(sep);
    }

    /// Remove the update menu item and its separator.
    pub fn remove_update_item(&mut self) {
        if let Some(item) = self.update_menu_item.take() {
            let _ = self.tray_menu.remove(&item);
        }
        if let Some(sep) = self.update_separator.take() {
            let _ = self.tray_menu.remove(&sep);
        }
    }

    /// Show an "Update failed — click to retry" item at the top of the menu.
    pub fn show_update_failed(&mut self) {
        self.remove_update_failed_item();

        let item = MenuItem::new("Update failed — click to retry", true, None);
        let sep = PredefinedMenuItem::separator();

        let _ = self.tray_menu.prepend(&sep);
        let _ = self.tray_menu.prepend(&item);

        self.update_failed_menu_item = Some(item);
        self.update_failed_separator = Some(sep);
    }

    /// Remove the update-failed menu item and its separator.
    pub fn remove_update_failed_item(&mut self) {
        if let Some(item) = self.update_failed_menu_item.take() {
            let _ = self.tray_menu.remove(&item);
        }
        if let Some(sep) = self.update_failed_separator.take() {
            let _ = self.tray_menu.remove(&sep);
        }
    }
}
