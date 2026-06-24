//! Port: Audio capture abstractions.
//!
//! Defines the [`CaptureBackend`], [`CaptureHandle`], and [`CaptureFactory`] traits
//! that decouple the capture pipeline from platform-specific audio APIs (WASAPI, CPAL).
//!
//! The [`CaptureFactory`] trait uses an **associated type** `Backend` so that
//! `CapturePool<F>` and `AudioStreamEngine<F, N>` monomorphize to the concrete backend,
//! eliminating vtable overhead on the audio hot path.
//!
//! # Strategy Pattern
//!
//! `CaptureFactory` is the Strategy interface. Variants:
//! - [`crate::adapters::capture::DefaultCaptureFactory`] — WASAPI (Windows) / CPAL (other)
//! - Mock factories in `#[cfg(test)]` blocks

use crate::error::GemaCastError;
use ringbuf::HeapCons;
use std::sync::Arc;
use tokio::sync::{Notify, mpsc};

/// Controls an active audio capture stream (play/pause lifecycle).
///
/// Implementations wrap platform-specific stream handles (WASAPI `IAudioClient`,
/// CPAL `Stream`, Oboe `AudioStream`).
pub trait CaptureBackend: Send {
    /// Start capturing audio samples into the associated ring buffer.
    fn play(&mut self) -> Result<(), GemaCastError>;

    /// Pause the capture stream. Samples stop flowing to the ring buffer.
    fn pause(&mut self) -> Result<(), GemaCastError>;
}

/// A constructed capture pipeline ready to be driven by [`CapturePool`](crate::stream::sender::capture_pool::CapturePool).
///
/// Generic over `B` so the backend is known at compile time (static dispatch).
/// The `CapturePool` erases `B` at the point of spawning the capture task,
/// so `AudioCaptureInstance` itself remains non-generic.
pub struct CaptureHandle<B: CaptureBackend> {
    /// The platform capture backend (WASAPI, CPAL, mock).
    pub backend: B,

    /// Consumer end of the ring buffer that receives raw f32 PCM samples
    /// from the backend's capture thread/callback.
    pub consumer: HeapCons<f32>,

    /// Notification primitive signaled by the backend when new samples
    /// are available in the ring buffer.
    pub notify: Arc<Notify>,

    /// Receives fatal stream errors from the backend (e.g., device unplugged).
    pub stream_error_rx: mpsc::Receiver<cpal::StreamError>,
}

/// Factory that creates capture backends (Strategy Pattern).
///
/// The associated type `Backend` allows `CapturePool<F>` to monomorphize
/// the entire capture pipeline at compile time.
///
/// # Strategy variants
///
/// | Implementation | Backend | Platform |
/// |---|---|---|
/// | `DefaultCaptureFactory` | `PlatformCaptureBackend` (enum) | Windows / Desktop Linux |
/// | `MockCaptureFactory` | `MockCaptureBackend` | Tests |
pub trait CaptureFactory: Send + Sync {
    /// The concrete capture backend type produced by this factory.
    type Backend: CaptureBackend + 'static;

    /// Create a capture handle for the system-wide desktop audio mix.
    fn create_desktop_capture(&self) -> Result<CaptureHandle<Self::Backend>, GemaCastError>;

    /// Create a capture handle for a specific process's audio output.
    ///
    /// # Platform support
    ///
    /// Only available on Windows (WASAPI process loopback). Other platforms
    /// should return [`AudioError::ProcessCaptureUnavailable`](crate::error::AudioError::ProcessCaptureUnavailable).
    fn create_process_capture(&self, pid: u32) -> Result<CaptureHandle<Self::Backend>, GemaCastError>;
}
