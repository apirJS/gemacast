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

#[cfg(target_os = "linux")]
pub mod pipewire_common;
#[cfg(target_os = "linux")]
pub mod pipewire_desktop;
#[cfg(target_os = "linux")]
pub mod pipewire_process;

#[cfg(target_os = "macos")]
pub mod sck_common;
#[cfg(target_os = "macos")]
pub mod sck_desktop;
#[cfg(target_os = "macos")]
pub mod sck_process;

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
    #[cfg(target_os = "linux")]
    PipeWireDesktop(pipewire_desktop::PipeWireDesktopCapture),
    #[cfg(target_os = "linux")]
    PipeWireProcess(pipewire_process::PipeWireProcessCapture),
    #[cfg(target_os = "macos")]
    SckDesktop(sck_desktop::SckDesktopCapture),
    #[cfg(target_os = "macos")]
    SckProcess(sck_process::SckProcessCapture),
    Cpal(cpal_loopback::CpalLoopbackCapture),
}

impl CaptureBackend for PlatformCaptureBackend {
    fn play(&mut self) -> Result<(), GemaCastError> {
        match self {
            #[cfg(target_os = "windows")]
            Self::WasapiDesktop(b) => b.play(),
            #[cfg(target_os = "windows")]
            Self::WasapiProcess(b) => b.play(),
            #[cfg(target_os = "linux")]
            Self::PipeWireDesktop(b) => b.play(),
            #[cfg(target_os = "linux")]
            Self::PipeWireProcess(b) => b.play(),
            #[cfg(target_os = "macos")]
            Self::SckDesktop(b) => b.play(),
            #[cfg(target_os = "macos")]
            Self::SckProcess(b) => b.play(),
            Self::Cpal(b) => b.play(),
        }
    }

    fn pause(&mut self) -> Result<(), GemaCastError> {
        match self {
            #[cfg(target_os = "windows")]
            Self::WasapiDesktop(b) => b.pause(),
            #[cfg(target_os = "windows")]
            Self::WasapiProcess(b) => b.pause(),
            #[cfg(target_os = "linux")]
            Self::PipeWireDesktop(b) => b.pause(),
            #[cfg(target_os = "linux")]
            Self::PipeWireProcess(b) => b.pause(),
            #[cfg(target_os = "macos")]
            Self::SckDesktop(b) => b.pause(),
            #[cfg(target_os = "macos")]
            Self::SckProcess(b) => b.pause(),
            Self::Cpal(b) => b.pause(),
        }
    }
}

// ---------------------------------------------------------------------------
// Production capture factory
// ---------------------------------------------------------------------------

/// Production capture factory (WASAPI on Windows, PipeWire on Linux,
/// ScreenCaptureKit on macOS, CPAL as universal fallback).
///
/// Implements [`CaptureFactory`] with `Backend = PlatformCaptureBackend`,
/// so the entire pipeline monomorphizes at compile time.
///
/// On Windows, the factory first attempts the modern WASAPI Application Loopback
/// API (which bypasses OEM Audio Processing Objects for clean audio). If WASAPI
/// fails (e.g., on older Windows builds < 20348), it falls back to CPAL with
/// a warning log.
///
/// On Linux, the factory first attempts PipeWire. If PipeWire is unavailable
/// (e.g., PulseAudio-only systems), it falls back to CPAL. Per-process capture
/// requires PipeWire — it is not available via CPAL.
///
/// On macOS, the factory uses ScreenCaptureKit for both desktop and per-process
/// capture. Falls back to CPAL for desktop capture if SCK permission is denied.
pub struct DefaultCaptureFactory;

impl CaptureFactory for DefaultCaptureFactory {
    type Backend = PlatformCaptureBackend;

    fn create_desktop_capture(&self) -> Result<CaptureHandle<Self::Backend>, GemaCastError> {
        #[cfg(target_os = "windows")]
        return wasapi_desktop::create_wasapi_desktop_loopback().or_else(|e| {
            tracing::warn!("WASAPI desktop capture failed ({e}), falling back to CPAL loopback");
            cpal_loopback::create_cpal_loopback()
        });

        #[cfg(target_os = "linux")]
        return if pipewire_common::is_pipewire_available() {
            pipewire_desktop::create_pipewire_desktop_loopback().or_else(|e| {
                tracing::warn!(
                    "PipeWire desktop capture failed ({e}), falling back to CPAL loopback"
                );
                cpal_loopback::create_cpal_loopback()
            })
        } else {
            tracing::info!("PipeWire not available, using CPAL loopback for desktop capture");
            cpal_loopback::create_cpal_loopback()
        };

        #[cfg(target_os = "macos")]
        return sck_desktop::create_sck_desktop_loopback().or_else(|e| {
            tracing::warn!(
                "ScreenCaptureKit desktop capture failed ({e}), falling back to CPAL loopback"
            );
            cpal_loopback::create_cpal_loopback()
        });

        #[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
        return cpal_loopback::create_cpal_loopback();
    }

    #[allow(unused_variables)]
    fn create_process_capture(
        &self,
        pid: u32,
    ) -> Result<CaptureHandle<Self::Backend>, GemaCastError> {
        #[cfg(target_os = "windows")]
        return wasapi_loopback::create_wasapi_process_loopback(pid);

        #[cfg(target_os = "linux")]
        return if pipewire_common::is_pipewire_available() {
            pipewire_process::create_pipewire_process_loopback(pid)
        } else {
            tracing::warn!("PipeWire not available — per-process audio capture requires PipeWire");
            Err(crate::domain::error::AudioError::ProcessCaptureUnavailable.into())
        };

        #[cfg(target_os = "macos")]
        return sck_process::create_sck_process_loopback(pid);

        #[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
        return Err(crate::domain::error::AudioError::ProcessCaptureUnavailable.into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_factory_creation_does_not_panic() {
        // Skip in CI to avoid macOS ScreenCaptureKit hanging on permissions dialog
        if std::env::var("CI").is_ok() {
            return;
        }
        let factory = DefaultCaptureFactory;

        // This test ensures that the factory methods don't panic upon invocation
        // regardless of the platform. We don't assert Ok() because we might be
        // running in a CI environment without audio hardware or permissions.
        let _desktop_result = factory.create_desktop_capture();

        // The process capture is expected to fail on non-Windows/macOS platforms
        // if PipeWire isn't available, or succeed if it is. Either way, no panic.
        let _process_result = factory.create_process_capture(999999);
    }
}
