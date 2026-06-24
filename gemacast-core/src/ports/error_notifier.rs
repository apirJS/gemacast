//! Port: Engine error notification.
//!
//! Decouples the [`AudioStreamEngine`](crate::stream::sender::engine::AudioStreamEngine)
//! from WebSocket connection internals. The engine notifies connected receivers
//! about errors (capture failures, source changes) through this trait instead
//! of directly manipulating `WsConnectionMap`.
//!
//! # Design note
//!
//! This is intentionally **synchronous** (not async). Error notifications are
//! fire-and-forget — if the channel is full, dropping the notification is
//! acceptable. This avoids async trait complexity and keeps the trait
//! fully static-dispatch compatible without `Pin<Box<dyn Future>>`.

use crate::domain::types::DeviceId;

/// Notifies a connected receiver about engine-side errors.
///
/// # Production
///
/// [`crate::adapters::error_notifier::WsErrorNotifier`] — sends `WsEvent::Error`
/// via the WebSocket connection map using `try_send` (non-blocking).
///
/// # Testing
///
/// Mock implementations record calls for assertion.
pub trait ErrorNotifier: Send + Sync {
    /// Notify the device that an error occurred in the engine.
    ///
    /// This is best-effort: implementations should not block or propagate errors.
    fn notify_error(&self, device_id: &DeviceId, message: String);
}
