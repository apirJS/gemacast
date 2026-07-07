//! Audio capture backends and factory (sender-side).
//!
//! Re-exports port traits from [`crate::ports::capture`] and provides the
//! production [`DefaultCaptureFactory`] that selects platform-specific backends.

use crate::domain::error::GemaCastError;

#[cfg(not(target_os = "android"))]
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

// DEAD CODE: ScreenCaptureKit disabled — untested, macOS falls back to CPAL
#[cfg(false)]
pub mod sck_common;
#[cfg(false)]
pub mod sck_desktop;
#[cfg(false)]
pub mod sck_process;

// Re-export port traits for backward compatibility.
// Consumers that previously imported from `stream::sender::capture::CaptureBackend`
// will continue to work.
pub use crate::ports::capture::{CaptureBackend, CaptureFactory, CaptureHandle};

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
    // ScreenCaptureKit variants disabled — untested
    #[cfg(not(target_os = "android"))]
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
            #[cfg(not(target_os = "android"))]
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
            #[cfg(not(target_os = "android"))]
            Self::Cpal(b) => b.pause(),
        }
    }
}

/// Production capture factory (WASAPI on Windows, PipeWire on Linux,
/// CPAL as universal fallback).
///
/// Implements [`CaptureFactory`] with `Backend = PlatformCaptureBackend`,
/// so the entire pipeline monomorphizes at compile time.
///
/// On Windows, the factory first attempts the modern WASAPI Application Loopback
/// API (which bypasses OEM Audio Processing Objects for clean audio). If WASAPI
/// fails (e.g., on older Windows builds < 20348), it falls back to CPAL with
/// a warning log. Per-process WASAPI capture returns
/// [`ProcessCaptureUnavailable`](crate::domain::error::AudioError::ProcessCaptureUnavailable)
/// on failure since CPAL cannot do per-process capture.
///
/// On Linux, the factory first attempts PipeWire. If PipeWire is unavailable
/// (e.g., PulseAudio-only systems), it falls back to CPAL. Per-process capture
/// requires PipeWire and returns
/// [`ProcessCaptureUnavailable`](crate::domain::error::AudioError::ProcessCaptureUnavailable)
/// if PipeWire is missing or if the capture fails at runtime.
///
/// On macOS, ScreenCaptureKit is disabled (untested). Desktop capture uses
/// CPAL loopback. Per-process capture is unavailable.
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

        // macOS: ScreenCaptureKit disabled (untested), use CPAL directly
        #[cfg(target_os = "macos")]
        return cpal_loopback::create_cpal_loopback();

        #[cfg(not(any(
            target_os = "windows",
            target_os = "linux",
            target_os = "macos",
            target_os = "android"
        )))]
        return cpal_loopback::create_cpal_loopback();

        #[cfg(target_os = "android")]
        return Err(crate::domain::error::AudioError::ProcessCaptureUnavailable.into());
    }

    #[allow(unused_variables)]
    fn create_process_capture(
        &self,
        pid: u32,
    ) -> Result<CaptureHandle<Self::Backend>, GemaCastError> {
        #[cfg(target_os = "windows")]
        return wasapi_loopback::create_wasapi_process_loopback(pid).map_err(|e| {
            tracing::warn!(
                "WASAPI per-process capture failed ({e}), per-process capture unavailable"
            );
            crate::domain::error::AudioError::ProcessCaptureUnavailable.into()
        });

        #[cfg(target_os = "linux")]
        return if pipewire_common::is_pipewire_available() {
            pipewire_process::create_pipewire_process_loopback(pid).map_err(|e| {
                tracing::warn!(
                    "PipeWire per-process capture failed ({e}), per-process capture unavailable"
                );
                crate::domain::error::AudioError::ProcessCaptureUnavailable.into()
            })
        } else {
            tracing::warn!("PipeWire not available — per-process audio capture requires PipeWire");
            Err(crate::domain::error::AudioError::ProcessCaptureUnavailable.into())
        };

        // macOS: ScreenCaptureKit disabled (untested), per-process capture unavailable
        #[cfg(target_os = "macos")]
        return Err(crate::domain::error::AudioError::ProcessCaptureUnavailable.into());

        #[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
        return Err(crate::domain::error::AudioError::ProcessCaptureUnavailable.into());
    }
}
