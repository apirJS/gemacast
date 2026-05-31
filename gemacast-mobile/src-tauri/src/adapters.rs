//! Production implementations of the trait abstractions.
//!
//! These adapters wrap the concrete I/O types (`tauri::AppHandle`,
//! `HttpControlClient`, `AudioStreamReceiver`, `netdev`) behind the traits
//! defined in [`crate::traits`]. The composition root in [`crate::run`]
//! creates these once at startup and passes `Arc<dyn Trait>` to services.

pub mod frontend_notifier;
pub mod network_info;
pub mod platform_service;
pub mod sender_control;
pub mod session_manager;

pub use frontend_notifier::TauriFrontendNotifier;
pub use network_info::NativeNetworkInfoProvider;
pub use platform_service::NativePlatformService;
pub use sender_control::HttpSenderControlClientFactory;
pub use session_manager::TokioSessionManager;
