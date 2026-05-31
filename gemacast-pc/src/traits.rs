//! Trait abstractions for all I/O boundaries in the PC sender.
//!
//! These traits decouple business logic from concrete dependencies
//! (`EventLoopProxy`, `mpsc::Sender`, `Arc<Mutex<HashMap>>`, HTTP clients),
//! making every handler unit-testable with mock implementations.
//!
//! # Production implementations
//!
//! See [`crate::adapters`] for the concrete adapters used at runtime.
//!
//! # Testing
//!
//! See [`crate::testing::mocks`] for hand-written mock implementations.

pub mod audio_controller;
pub mod device_notifier;
pub mod device_registry;
pub mod tray_notifier;

pub use audio_controller::AudioController;
pub use device_notifier::DeviceNotifier;
pub use device_registry::{DeviceRegistry, RegistrationOutcome};
pub use tray_notifier::TrayNotifier;
