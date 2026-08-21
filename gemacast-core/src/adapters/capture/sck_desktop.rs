#![cfg(target_os = "macos")]

//! ScreenCaptureKit desktop (system-wide) audio capture backend.
//!
//! Captures all system audio by creating an `SCContentFilter` for the
//! entire primary display. This is the macOS equivalent of WASAPI desktop
//! loopback on Windows.
//!
//! # Permissions
//!
//! Requires the "Screen & System Audio Recording" permission in
//! System Settings → Privacy & Security. If denied, returns
//! [`AudioError::ScreenCapturePermissionDenied`].

use crate::domain::error::{AudioError, GemaCastError};
use crate::ports::capture::{CaptureBackend, CaptureHandle};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use screencapturekit::prelude::*;

use super::sck_common::{SckAudioHandler, create_sck_audio_config, create_sck_ring_buffer};

/// ScreenCaptureKit desktop audio capture backend.
///
/// Captures all system audio from the primary display.
/// Implements [`CaptureBackend`] with the same lifecycle pattern as
/// [`super::wasapi_desktop::WasapiDesktopCapture`].
pub struct SckDesktopCapture {
    stream: SCStream,
    is_running: Arc<AtomicBool>,
}

impl CaptureBackend for SckDesktopCapture {
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

impl Drop for SckDesktopCapture {
    fn drop(&mut self) {
        if self.is_running.load(Ordering::Relaxed) {
            let _ = self.stream.stop_capture();
            self.is_running.store(false, Ordering::Relaxed);
        }
    }
}

/// Create a desktop loopback capture handle using ScreenCaptureKit.
///
/// Captures all system audio by filtering the primary display.
/// The audio handler pushes 48kHz stereo f32 PCM into the ring buffer.
///
/// # Errors
///
/// * [`AudioError::ScreenCapturePermissionDenied`] if screen recording
///   permission has not been granted.
/// * [`AudioError::ScreenCaptureKitError`] for SCK runtime errors.
pub fn create_sck_desktop_loopback()
-> Result<CaptureHandle<super::PlatformCaptureBackend>, GemaCastError> {
    // Enumerate shareable content — this will fail if permission is denied.
    let content = SCShareableContent::get().map_err(|e| {
        let msg = format!("{e}");
        if msg.contains("permission") || msg.contains("denied") || msg.contains("not authorized") {
            GemaCastError::Audio(AudioError::ScreenCapturePermissionDenied)
        } else {
            GemaCastError::Audio(AudioError::ScreenCaptureKitError(msg))
        }
    })?;

    let displays = content.displays();
    if displays.is_empty() {
        return Err(AudioError::ScreenCaptureKitError("No displays found".to_string()).into());
    }
    let display = &displays[0];

    // Build a content filter capturing the entire primary display.
    // We exclude no windows — we want all system audio.
    let filter = SCContentFilter::create()
        .with_display(display)
        .with_excluding_windows(&[])
        .build();

    let config = create_sck_audio_config();

    let (producer, stream_error_tx, resources) = create_sck_ring_buffer();
    let handler = SckAudioHandler::new(producer, resources.notify.clone());

    let delegate =
        screencapturekit::stream::delegate_trait::StreamCallbacks::new().on_error(move |e| {
            tracing::warn!("[SCK Desktop] Stream stopped with error: {e}");
            let _ = stream_error_tx.try_send(cpal::StreamError::DeviceNotAvailable);
        });

    let mut stream = SCStream::new_with_delegate(&filter, &config, delegate);
    stream.add_output_handler(handler, SCStreamOutputType::Audio);

    stream
        .start_capture()
        .map_err(|e| AudioError::ScreenCaptureKitError(format!("start: {e}")))?;

    tracing::info!("[SCK Desktop] Capture stream started on primary display");

    Ok(CaptureHandle {
        backend: super::PlatformCaptureBackend::SckDesktop(SckDesktopCapture {
            stream,
            is_running: Arc::new(AtomicBool::new(true)),
        }),
        consumer: resources.consumer,
        notify: resources.notify,
        stream_error_rx: resources.stream_error_rx,
        counters: resources.counters,
    })
}
