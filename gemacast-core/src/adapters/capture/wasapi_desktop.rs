#![cfg(target_os = "windows")]

use crate::{domain::error::GemaCastError, ports::capture::CaptureHandle};

use windows::Win32::Media::Audio::PROCESS_LOOPBACK_MODE_EXCLUDE_TARGET_PROCESS_TREE;

pub type WasapiDesktopCapture = super::wasapi_loopback::WasapiLoopbackCapture;

/// Create a desktop loopback capture handle using the modern Application Loopback API.
///
/// Uses `PROCESS_LOOPBACK_MODE_EXCLUDE_TARGET_PROCESS_TREE` with Gemacast's own PID
/// as the target. This captures all system audio except Gemacast's own process tree,
/// bypassing OEM audio processing and preventing feedback from Gemacast itself.
///
/// Requires Windows 10 Build 20348+. The factory falls back to CPAL on older systems.
pub fn create_wasapi_desktop_loopback()
-> Result<CaptureHandle<super::PlatformCaptureBackend>, GemaCastError> {
    let CaptureHandle {
        backend,
        consumer,
        notify,
        stream_error_rx,
        counters,
    } = super::wasapi_loopback::create_wasapi_application_loopback(
        std::process::id(),
        PROCESS_LOOPBACK_MODE_EXCLUDE_TARGET_PROCESS_TREE,
        "desktop",
    )?;

    Ok(CaptureHandle {
        backend: super::PlatformCaptureBackend::WasapiDesktop(backend),
        consumer,
        notify,
        stream_error_rx,
        counters,
    })
}
