//! Adapter implementations — concrete I/O wiring for port traits.
//!
//! These are the "driven" (secondary) adapters in hexagonal architecture.
//! Each adapter implements a port trait from [`crate::ports`] and
//! connects it to a real dependency (WebSocket map, WASAPI, etc.).
//!
//! # Re-exports
//!
//! For convenience, production adapters are re-exported here so consumers
//! can import from `gemacast_core::adapters::*`.

pub mod capture;
pub mod error_notifier;
pub mod process_lister;
pub mod transport;

pub use capture::{DefaultCaptureFactory, PlatformCaptureBackend};
pub use error_notifier::WsErrorNotifier;
pub use process_lister::DefaultProcessLister;
