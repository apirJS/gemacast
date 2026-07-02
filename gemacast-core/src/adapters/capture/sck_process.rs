#![cfg(target_os = "macos")]

//! ScreenCaptureKit per-process audio capture backend.
//!
//! Captures audio from a specific application by creating an `SCContentFilter`
//! that targets only that application's windows. This is the macOS equivalent
//! of WASAPI process loopback on Windows.
//!
//! # Process Discovery
//!
//! ScreenCaptureKit provides `SCShareableContent::applications()` which lists
//! all capturable applications with their PIDs. We match the target PID and
//! create a filter scoped to that application only.
//!
//! # Permissions
//!
//! Requires the "Screen & System Audio Recording" permission.

use crate::domain::error::{AudioError, GemaCastError};
use crate::ports::capture::{CaptureBackend, CaptureHandle};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use screencapturekit::prelude::*;

use super::sck_common::{SckAudioHandler, create_sck_audio_config, create_sck_ring_buffer};

/// ScreenCaptureKit per-process audio capture backend.
///
/// Captures audio from a specific application identified by PID.
/// Implements [`CaptureBackend`] with the same lifecycle as
/// [`super::wasapi_loopback::WasapiLoopbackCapture`].
pub struct SckProcessCapture {
    stream: SCStream,
    is_running: Arc<AtomicBool>,
}

impl CaptureBackend for SckProcessCapture {
    fn play(&mut self) -> Result<(), GemaCastError> {
        if !self.is_running.load(Ordering::Relaxed) {
            self.stream
                .start_capture()
                .map_err(|e| AudioError::ScreenCaptureKitError(format!("start: {e}")))?;
            self.is_running.store(true, Ordering::Relaxed);
        }
        Ok(())
    }

    fn pause(&mut self) -> Result<(), GemaCastError> {
        if self.is_running.load(Ordering::Relaxed) {
            self.stream
                .stop_capture()
                .map_err(|e| AudioError::ScreenCaptureKitError(format!("stop: {e}")))?;
            self.is_running.store(false, Ordering::Relaxed);
        }
        Ok(())
    }
}

impl Drop for SckProcessCapture {
    fn drop(&mut self) {
        if self.is_running.load(Ordering::Relaxed) {
            let _ = self.stream.stop_capture();
            self.is_running.store(false, Ordering::Relaxed);
        }
    }
}

/// Create a per-process audio capture handle using ScreenCaptureKit.
///
/// Discovers the target application by PID via `SCShareableContent`,
/// then creates an `SCContentFilter` scoped to that application's windows.
///
/// # Arguments
///
/// * `pid` - The process ID of the application to capture audio from.
///
/// # Errors
///
/// * [`AudioError::ProcessNotFound`] if no application with the given PID
///   is found in `SCShareableContent`.
/// * [`AudioError::ScreenCapturePermissionDenied`] if screen recording
///   permission has not been granted.
/// * [`AudioError::ScreenCaptureKitError`] for SCK runtime errors.
pub fn create_sck_process_loopback(
    pid: u32,
) -> Result<CaptureHandle<super::PlatformCaptureBackend>, GemaCastError> {
    // Enumerate shareable content — this will fail if permission is denied.
    let content = SCShareableContent::get().map_err(|e| {
        let msg = format!("{e}");
        if msg.contains("permission") || msg.contains("denied") || msg.contains("not authorized") {
            GemaCastError::Audio(AudioError::ScreenCapturePermissionDenied)
        } else {
            GemaCastError::Audio(AudioError::ScreenCaptureKitError(msg))
        }
    })?;

    // Find the target application by PID.
    let target_app = content
        .applications()
        .into_iter()
        .find(|app| app.process_id() == pid as i32)
        .ok_or_else(|| GemaCastError::Audio(AudioError::ProcessNotFound(pid)))?;

    let app_name = target_app
        .application_name()
        .unwrap_or_else(|| format!("PID {pid}"));

    // Get the application's windows for the content filter.
    // If the app has windows, we use them. If not (e.g., background audio app),
    // we fall back to a display filter that includes only this app.
    let app_windows = target_app.windows();

    let filter = if !app_windows.is_empty() {
        // Use the first window to create a window-scoped filter
        SCContentFilter::create()
            .with_window(&app_windows[0])
            .build()
    } else {
        // Fallback: use display filter including only this app
        let displays = content.displays();
        if displays.is_empty() {
            return Err(AudioError::ScreenCaptureKitError("No displays found".to_string()).into());
        }
        SCContentFilter::create()
            .with_display(&displays[0])
            .with_including_applications(&[&target_app], &[])
            .build()
    };

    let config = create_sck_audio_config();

    let (producer, stream_error_tx, resources) = create_sck_ring_buffer();
    let handler = SckAudioHandler::new(producer, resources.notify.clone());

    let delegate =
        screencapturekit::stream::delegate_trait::StreamCallbacks::new().on_error(move |e| {
            tracing::warn!("[SCK Process] Stream stopped with error: {e}");
            let _ = stream_error_tx.try_send(cpal::StreamError::DeviceNotAvailable);
        });

    let mut stream = SCStream::new_with_delegate(&filter, &config, delegate);
    stream.add_output_handler(handler, SCStreamOutputType::Audio);

    stream
        .start_capture()
        .map_err(|e| AudioError::ScreenCaptureKitError(format!("start: {e}")))?;

    tracing::info!(
        "[SCK Process] Capture stream started for '{}' (PID {})",
        app_name,
        pid
    );

    Ok(CaptureHandle {
        backend: super::PlatformCaptureBackend::SckProcess(SckProcessCapture {
            stream,
            is_running: Arc::new(AtomicBool::new(true)),
        }),
        consumer: resources.consumer,
        notify: resources.notify,
        stream_error_rx: resources.stream_error_rx,
    })
}
