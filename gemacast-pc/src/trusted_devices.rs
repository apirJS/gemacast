//! Persistent LAN-device trust stored beside `config.json`.
//!
//! The file contains public keys only. Bearer tokens and proof challenges stay
//! in memory and are rotated for every connection.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use gemacast_core::domain::types::DeviceId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrustedDevice {
    pub device_id: DeviceId,
    pub device_name: String,
    /// Base64-encoded SEC1 uncompressed P-256 public point.
    pub public_key: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TrustedDeviceFile {
    #[serde(default)]
    pc_certificate_fingerprint: Option<String>,
    #[serde(default)]
    devices: Vec<TrustedDevice>,
}

#[derive(Default)]
struct TrustedDeviceState {
    pc_certificate_fingerprint: Option<String>,
    devices: HashMap<DeviceId, TrustedDevice>,
}

#[derive(Clone)]
pub struct TrustedDeviceStore {
    path: Option<PathBuf>,
    state: Arc<Mutex<TrustedDeviceState>>,
}

impl TrustedDeviceStore {
    pub fn load_default() -> Self {
        Self::load(crate::config::trusted_devices_path())
    }

    pub fn load(path: PathBuf) -> Self {
        let file = std::fs::read_to_string(&path)
            .ok()
            .and_then(|contents| serde_json::from_str::<TrustedDeviceFile>(&contents).ok())
            .unwrap_or_default();
        Self {
            path: Some(path),
            state: Arc::new(Mutex::new(TrustedDeviceState {
                pc_certificate_fingerprint: file.pc_certificate_fingerprint,
                devices: file
                    .devices
                    .into_iter()
                    .map(|device| (device.device_id.clone(), device))
                    .collect(),
            })),
        }
    }

    #[cfg(test)]
    pub fn in_memory() -> Self {
        Self {
            path: None,
            state: Arc::new(Mutex::new(TrustedDeviceState {
                pc_certificate_fingerprint: Some("test-pc-certificate".into()),
                devices: HashMap::new(),
            })),
        }
    }

    /// Bind this allowlist to the persistent PC certificate.
    ///
    /// Phone trust cannot survive a PC identity rotation: the comparison code
    /// would otherwise have no matching PC-side prompt. A missing or changed
    /// fingerprint therefore clears the old allowlist before the control
    /// server starts.
    pub fn bind_pc_identity(&self, fingerprint: &str) -> io::Result<()> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| io::Error::other("trusted-device store is unavailable"))?;
        if state.pc_certificate_fingerprint.as_deref() == Some(fingerprint) {
            return Ok(());
        }
        let previous_fingerprint = state.pc_certificate_fingerprint.replace(fingerprint.into());
        let previous_devices = std::mem::take(&mut state.devices);
        if let Some(path) = self.path.as_deref()
            && let Err(error) = save_state(path, &state)
        {
            state.pc_certificate_fingerprint = previous_fingerprint;
            state.devices = previous_devices;
            return Err(error);
        }
        Ok(())
    }

    pub fn is_trusted(&self, device_id: &DeviceId, public_key: &str) -> bool {
        self.state
            .lock()
            .ok()
            .and_then(|state| state.devices.get(device_id).cloned())
            .is_some_and(|device| {
                constant_time_eq(device.public_key.as_bytes(), public_key.as_bytes())
            })
    }

    pub fn public_key(&self, device_id: &DeviceId) -> Option<String> {
        self.state.lock().ok().and_then(|state| {
            state
                .devices
                .get(device_id)
                .map(|device| device.public_key.clone())
        })
    }

    pub fn trust(
        &self,
        device_id: DeviceId,
        device_name: String,
        public_key: String,
    ) -> io::Result<()> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| io::Error::other("trusted-device store is unavailable"))?;
        if state.pc_certificate_fingerprint.is_none() {
            return Err(io::Error::other(
                "trusted-device store is not bound to a PC identity",
            ));
        }
        let previous = state.devices.insert(
            device_id.clone(),
            TrustedDevice {
                device_id: device_id.clone(),
                device_name,
                public_key,
            },
        );
        if let Some(path) = self.path.as_deref()
            && let Err(error) = save_state(path, &state)
        {
            match previous {
                Some(device) => {
                    state.devices.insert(device_id, device);
                }
                None => {
                    state.devices.remove(&device_id);
                }
            }
            return Err(error);
        }
        Ok(())
    }

    pub fn forget(&self, device_id: &DeviceId) -> io::Result<bool> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| io::Error::other("trusted-device store is unavailable"))?;
        let Some(previous) = state.devices.remove(device_id) else {
            return Ok(false);
        };
        if let Some(path) = self.path.as_deref()
            && let Err(error) = save_state(path, &state)
        {
            state.devices.insert(device_id.clone(), previous);
            return Err(error);
        }
        Ok(true)
    }
}

fn save_state(path: &Path, state: &TrustedDeviceState) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut devices: Vec<_> = state.devices.values().cloned().collect();
    devices.sort_by(|left, right| left.device_id.0.cmp(&right.device_id.0));
    let json = serde_json::to_vec_pretty(&TrustedDeviceFile {
        pc_certificate_fingerprint: state.pc_certificate_fingerprint.clone(),
        devices,
    })
    .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    let tmp_path = path.with_extension("json.tmp");
    std::fs::write(&tmp_path, json)?;
    std::fs::rename(tmp_path, path)
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .fold(0u8, |difference, (left, right)| difference | (left ^ right))
            == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir()
            .join("gemacast-trusted-device-tests")
            .join(format!("{name}-{}-{unique}.json", std::process::id()))
    }

    #[test]
    fn trust_should_survive_reload_and_forget_should_remove_it() {
        let path = temp_path("round-trip");
        let device_id = DeviceId("phone-1".into());
        let store = TrustedDeviceStore::load(path.clone());
        store.bind_pc_identity("pc-certificate").unwrap();
        store
            .trust(device_id.clone(), "Phone".into(), "public-key".into())
            .unwrap();

        let reloaded = TrustedDeviceStore::load(path.clone());
        reloaded.bind_pc_identity("pc-certificate").unwrap();
        assert!(reloaded.is_trusted(&device_id, "public-key"));
        assert!(reloaded.forget(&device_id).unwrap());
        assert!(!TrustedDeviceStore::load(path.clone()).is_trusted(&device_id, "public-key"));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn malformed_file_should_fall_back_to_an_empty_store() {
        let path = temp_path("malformed");
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&path, b"not json").unwrap();

        let store = TrustedDeviceStore::load(path.clone());
        assert!(!store.is_trusted(&DeviceId("phone-1".into()), "key"));
        store.bind_pc_identity("pc-certificate").unwrap();
        store
            .trust(DeviceId("phone-1".into()), "Phone".into(), "key".into())
            .unwrap();
        assert!(
            TrustedDeviceStore::load(path.clone()).is_trusted(&DeviceId("phone-1".into()), "key")
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn identity_rotation_should_clear_persisted_device_trust() {
        let path = temp_path("identity-rotation");
        let device_id = DeviceId("phone-1".into());
        let store = TrustedDeviceStore::load(path.clone());
        store.bind_pc_identity("old-certificate").unwrap();
        store
            .trust(device_id.clone(), "Phone".into(), "public-key".into())
            .unwrap();

        let reloaded = TrustedDeviceStore::load(path.clone());
        reloaded.bind_pc_identity("new-certificate").unwrap();

        assert!(!reloaded.is_trusted(&device_id, "public-key"));
        assert!(!TrustedDeviceStore::load(path.clone()).is_trusted(&device_id, "public-key"));
        let _ = std::fs::remove_file(path);
    }
}
