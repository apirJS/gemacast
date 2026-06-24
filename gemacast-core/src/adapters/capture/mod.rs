//! Audio capture backends and factory (sender-side).
//!
//! Re-exports port traits from [`crate::ports::capture`] and provides the
//! production [`DefaultCaptureFactory`] that selects platform-specific backends.

use crate::domain::error::GemaCastError;

pub mod cpal_loopback;
#[cfg(target_os = "windows")]
pub mod wasapi_common;
#[cfg(target_os = "windows")]
pub mod wasapi_desktop;
pub mod wasapi_loopback;

// Re-export port traits for backward compatibility.
// Consumers that previously imported from `stream::sender::capture::CaptureBackend`
// will continue to work.
pub use crate::ports::capture::{CaptureBackend, CaptureFactory, CaptureHandle};

// ---------------------------------------------------------------------------
// Platform capture backend (enum dispatch for static dispatch within factory)
// ---------------------------------------------------------------------------

/// Enum-dispatched capture backend that wraps all platform-specific backends.
///
/// This is the associated type `Backend` for [`DefaultCaptureFactory`].
/// Using an enum instead of `Box<dyn CaptureBackend>` gives us:
/// - No vtable pointer indirection
/// - Compiler can inline `play()`/`pause()` through the match arms
/// - Stack-allocated (no heap allocation per capture handle)
pub enum PlatformCaptureBackend {
    #[cfg(target_os = "windows")]
    WasapiDesktop(wasapi_desktop::WasapiDesktopCapture),
    #[cfg(target_os = "windows")]
    WasapiProcess(wasapi_loopback::WasapiLoopbackCapture),
    #[cfg(not(target_os = "windows"))]
    Cpal(cpal_loopback::CpalLoopbackCapture),
}

impl CaptureBackend for PlatformCaptureBackend {
    fn play(&mut self) -> Result<(), GemaCastError> {
        match self {
            #[cfg(target_os = "windows")]
            Self::WasapiDesktop(b) => b.play(),
            #[cfg(target_os = "windows")]
            Self::WasapiProcess(b) => b.play(),
            #[cfg(not(target_os = "windows"))]
            Self::Cpal(b) => b.play(),
        }
    }

    fn pause(&mut self) -> Result<(), GemaCastError> {
        match self {
            #[cfg(target_os = "windows")]
            Self::WasapiDesktop(b) => b.pause(),
            #[cfg(target_os = "windows")]
            Self::WasapiProcess(b) => b.pause(),
            #[cfg(not(target_os = "windows"))]
            Self::Cpal(b) => b.pause(),
        }
    }
}

// ---------------------------------------------------------------------------
// Production capture factory
// ---------------------------------------------------------------------------

/// Production capture factory (Strategy: WASAPI on Windows, CPAL elsewhere).
///
/// Implements [`CaptureFactory`] with `Backend = PlatformCaptureBackend`,
/// so the entire pipeline monomorphizes at compile time.
pub struct DefaultCaptureFactory;

impl CaptureFactory for DefaultCaptureFactory {
    type Backend = PlatformCaptureBackend;

    fn create_desktop_capture(&self) -> Result<CaptureHandle<Self::Backend>, GemaCastError> {
        #[cfg(windows)]
        {
            wasapi_desktop::create_wasapi_desktop_loopback()
        }
        #[cfg(not(windows))]
        {
            cpal_loopback::create_cpal_loopback()
        }
    }

    #[allow(unused_variables)]
    fn create_process_capture(
        &self,
        pid: u32,
    ) -> Result<CaptureHandle<Self::Backend>, GemaCastError> {
        #[cfg(windows)]
        {
            wasapi_loopback::create_wasapi_process_loopback(pid)
        }
        #[cfg(not(windows))]
        {
            Err(crate::domain::error::AudioError::ProcessCaptureUnavailable.into())
        }
    }
}
