//! Network utilities, port definitions, and ADB transport support.

pub mod adb;
pub mod interface;
pub mod ports;

/// Re-export discovery types for convenience — consumers can import
/// from either `discovery::` or `network::`.
pub use crate::discovery::{PresenceBroadcaster, PresenceListener};

pub use interface::{classify_interface, get_broadcast_addrs, get_local_ip, is_usb_tether_ip};
pub use ports::Ports;

pub use crate::domain::types::{ConnectionModes, get_available_connection_modes};
