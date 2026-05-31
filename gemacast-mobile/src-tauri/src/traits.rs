//! Trait abstractions for all I/O boundaries in the mobile receiver.
//!
//! These traits decouple domain logic from concrete dependencies
//! (`tauri::AppHandle`, `HttpControlClient`, `AudioStreamReceiver`, `netdev`),
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
pub mod sender_control;
pub mod session_manager;
pub mod types;

pub use frontend_notifier::FrontendNotifier;
pub use network_info::NetworkInfoProvider;
pub use platform_service::PlatformService;
pub use sender_control::{SenderControlClient, SenderControlClientFactory};
pub use session_manager::SessionManager;
pub use types::{ConnectParams, InterfaceInfo, ResumeParams, SessionInfo, SessionParams};
