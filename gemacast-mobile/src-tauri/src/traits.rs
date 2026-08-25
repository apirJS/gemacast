//! Trait abstractions for all I/O boundaries in the mobile player.
//!
//! These traits decouple domain logic from concrete dependencies
//! (`tauri::AppHandle`, `HttpControlClient`, `AudioStreamPlayer`, `netdev`),
//! making every service function unit-testable with mock implementations.
//!
//! # Production implementations
//!
//! See [`crate::adapters`] for the concrete adapters used at runtime.
//!
//! # Testing
//!
//! See [`crate::testing::mocks`] for hand-written mock implementations.

pub mod frontend_notifier;
pub mod network_info;
pub mod platform_service;
pub mod session_manager;
pub mod streamer_control;
pub mod types;

pub use frontend_notifier::FrontendNotifier;
pub use network_info::NetworkInfoProvider;
pub use platform_service::{NotificationPermission, PlatformService, PlaybackState};
pub use session_manager::SessionManager;
pub use streamer_control::{StreamerControlClient, StreamerControlClientFactory};
pub use types::{ConnectParams, InterfaceInfo, ResumeParams, SessionInfo, SessionParams};
