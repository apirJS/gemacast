use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

pub use gemacast_core::types::DiscoveredDevice;

pub type DeviceList = Arc<Mutex<HashMap<String, DiscoveredDevice>>>;

pub fn create_shared_state() -> DeviceList {
    Arc::new(Mutex::new(HashMap::new()))
}
