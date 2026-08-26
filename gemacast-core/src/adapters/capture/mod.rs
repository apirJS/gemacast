//! Audio capture backends and factory (streamer-side).
//!
//! Re-exports port traits from [`crate::ports::capture`] and provides the
//! production [`DefaultCaptureFactory`] that selects platform-specific backends.

#[cfg(not(target_os = "android"))]
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

#[cfg(target_os = "macos")]
pub mod sck_common;
#[cfg(target_os = "macos")]
pub mod sck_desktop;
#[cfg(target_os = "macos")]
pub mod sck_process;

// Re-export port traits for backward compatibility.
// Consumers that previously imported from `stream::streamer::capture::CaptureBackend`
// will continue to work.
pub use crate::ports::capture::{CaptureBackend, CaptureFactory, CaptureHandle};

/// Enum-dispatched capture backend that wraps all platform-specific backends.
///
/// This is the associated type `Backend` for [`DefaultCaptureFactory`].
/// Using an enum instead of `Box<dyn CaptureBackend>` gives us:
/// - No vtable pointer indirection
/// - Compiler can inline `play()`/`pause()` through the match arms
/// - Stack-allocated (no heap allocation per capture handle)
///
/// Not available on Android — Android is a player-only platform.
#[cfg(not(target_os = "android"))]
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
    #[cfg(not(target_os = "android"))]
    Cpal(cpal_loopback::CpalLoopbackCapture),
}

#[cfg(not(target_os = "android"))]
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
            #[cfg(target_os = "macos")]
            Self::SckDesktop(b) => b.pause(),
            #[cfg(target_os = "macos")]
            Self::SckProcess(b) => b.pause(),
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
/// On macOS 13+, the factory first attempts ScreenCaptureKit (the OS-native path,
/// like WASAPI/PipeWire). If SCK fails, or on macOS < 13 where the audio APIs are
/// unavailable, it falls back to CPAL loopback with a log line. CPAL loopback on
/// macOS needs a virtual output device (BlackHole/Soundflower) and captures nothing
/// on a stock Mac, so the `< 13` branch logs how to get audio working.
///
/// Not available on Android — Android is a player-only platform.
#[cfg(not(target_os = "android"))]
pub struct DefaultCaptureFactory;

#[cfg(not(target_os = "android"))]
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

        // macOS: ScreenCaptureKit on 13+, CPAL loopback below or on any SCK failure.
        #[cfg(target_os = "macos")]
        return if macos_supports_sck() {
            sck_desktop::create_sck_desktop_loopback().or_else(|e| {
                tracing::warn!("SCK desktop capture failed ({e}), falling back to CPAL loopback");
                cpal_loopback::create_cpal_loopback()
            })
        } else {
            cpal_loopback::create_cpal_loopback()
        };

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

        // macOS: per-process capture is SCK-only (CPAL has no per-process path), so
        // unlike desktop there is nothing to fall back to — mirror Windows and report
        // it unavailable. On macOS < 13 SCK audio does not exist, so skip it entirely.
        #[cfg(target_os = "macos")]
        return if macos_supports_sck() {
            sck_process::create_sck_process_loopback(pid).map_err(|e| {
                tracing::warn!(
                    "SCK per-process capture failed ({e}), per-process capture unavailable"
                );
                crate::domain::error::AudioError::ProcessCaptureUnavailable.into()
            })
        } else {
            tracing::warn!(
                "macOS < 13 — per-process audio capture requires ScreenCaptureKit (macOS 13+)"
            );
            Err(crate::domain::error::AudioError::ProcessCaptureUnavailable.into())
        };

        #[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
        return Err(crate::domain::error::AudioError::ProcessCaptureUnavailable.into());
    }
}

/// True when the host is macOS 13.0 (Ventura) or newer, where ScreenCaptureKit's
/// audio-capture APIs exist.
///
/// This is a gate, not a try-and-fall-back. The `screencapturekit` crate links a bundled
/// Swift bridge whose package declares `platforms: [.macOS(.v13)]`, and the audio config
/// setters (`capturesAudio`, `sampleRate`, `channelCount`, `excludesCurrentProcessAudio`)
/// carry **no** `if #available` guard behind it — verified in the published 1.5.4 source.
/// So below macOS 13 those calls are not a catchable error, they are undefined behaviour
/// at the framework boundary. Gate first, and never ask.
///
/// The result is cached, and the "unsupported → CPAL" notice is logged exactly
/// once per session.
///
/// Detection shells out to `sw_vers -productVersion`, a stable binary present on every
/// macOS install. If it cannot be run or parsed we assume **unsupported** and use CPAL:
/// the only cost is forgoing SCK on a Mac that might have supported it, never a crash
/// from calling into a framework that predates the API.
#[cfg(target_os = "macos")]
fn macos_supports_sck() -> bool {
    use std::sync::OnceLock;
    static SUPPORTED: OnceLock<bool> = OnceLock::new();
    *SUPPORTED.get_or_init(|| {
        let supported = macos_product_version_major().is_some_and(|major| major >= 13);
        if !supported {
            // Once-per-session, because CPAL loopback captures nothing on a stock Mac —
            // without this line that silent capture looks like a bug rather than a
            // missing dependency.
            tracing::info!(
                "macOS < 13 (or version undetectable): built-in audio capture needs macOS 13+ \
                 (ScreenCaptureKit). Falling back to CPAL loopback, which captures nothing without \
                 a virtual audio device such as BlackHole — install one, or upgrade to macOS 13+."
            );
        }
        supported
    })
}

/// Major component of `sw_vers -productVersion` (`"13.5.2"` → `13`), or `None` if the
/// command cannot be run or its output parsed. See [`macos_supports_sck`].
#[cfg(target_os = "macos")]
fn macos_product_version_major() -> Option<u32> {
    let output = crate::process::quiet_command("sw_vers")
        .arg("-productVersion")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let version = String::from_utf8(output.stdout).ok()?;
    version.trim().split('.').next()?.parse::<u32>().ok()
}
