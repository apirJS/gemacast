use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use gemacast_core::types::DeviceId;
pub use gemacast_core::types::DiscoveredDevice;

pub type DeviceList = Arc<Mutex<HashMap<DeviceId, DiscoveredDevice>>>;

pub fn create_shared_state() -> DeviceList {
    Arc::new(Mutex::new(HashMap::new()))
}
