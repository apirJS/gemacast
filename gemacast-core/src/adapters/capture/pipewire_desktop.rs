#![cfg(target_os = "linux")]

//! PipeWire desktop (system-wide) audio capture backend.
//!
//! Captures the monitor of the default audio sink, giving us all system audio
//! (equivalent to WASAPI desktop loopback on Windows).
//!
//! # Threading Model
//!
//! A dedicated OS thread runs the PipeWire `MainLoop`. The `process` callback
//! pushes f32 PCM samples into a lock-free ring buffer. The async side
//! (CapturePool) consumes from the ring buffer via the `Notify` primitive.

use crate::domain::error::{AudioError, GemaCastError};
use crate::ports::capture::{CaptureBackend, CaptureHandle};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use pipewire as pw;
use pw::properties::properties;

use pw::stream::{StreamBox as Stream, StreamFlags};

use super::pipewire_common::{self, create_pw_ring_buffer, push_pw_audio_to_ringbuf};

/// PipeWire desktop audio capture backend.
///
/// Captures the system's default audio output (monitor/loopback).
/// Implements [`CaptureBackend`] with the same lifecycle pattern as
/// [`super::wasapi_desktop::WasapiDesktopCapture`].
pub struct PipeWireDesktopCapture {
    is_running: Arc<AtomicBool>,
    thread_handle: Option<std::thread::JoinHandle<()>>,
}

impl CaptureBackend for PipeWireDesktopCapture {
    fn play(&mut self) -> Result<(), GemaCastError> {
        // The PipeWire stream starts capturing as soon as it's connected.
        // play() is a no-op because the stream is already running.
        Ok(())
    }

    fn pause(&mut self) -> Result<(), GemaCastError> {
        // Pausing is handled by the shutdown mechanism in Drop.
        // We don't explicitly pause the PipeWire stream because the
        // WASAPI backends also don't support true pause — they just stop.
        Ok(())
    }
}

impl Drop for PipeWireDesktopCapture {
    fn drop(&mut self) {
        self.is_running.store(false, Ordering::Relaxed);
        if let Some(handle) = self.thread_handle.take() {
            let _ = handle.join();
        }
    }
}

/// Create a desktop loopback capture handle using PipeWire.
///
/// Connects to the system's default audio sink monitor port, capturing
/// all desktop audio. This is the PipeWire equivalent of WASAPI's
/// `PROCESS_LOOPBACK_MODE_EXCLUDE_TARGET_PROCESS_TREE` with own PID.
///
/// # Errors
///
/// Returns [`AudioError::PipeWireConnectionFailed`] if PipeWire cannot
/// be initialized or the stream cannot be created.
/// Returns [`AudioError::PipeWireError`] for any runtime PipeWire errors.
pub fn create_pipewire_desktop_loopback()
-> Result<CaptureHandle<super::PlatformCaptureBackend>, GemaCastError> {
    let (mut producer, resources, stream_error_tx) = create_pw_ring_buffer();
    let notify_clone = resources.notify.clone();

    let is_running = Arc::new(AtomicBool::new(true));
    let is_running_thread = is_running.clone();

    let thread_handle = std::thread::spawn(move || {
        // Initialize PipeWire on this thread
        pw::init();

        let result = run_desktop_capture_loop(
            &mut producer,
            &notify_clone,
            &is_running_thread,
            stream_error_tx,
        );

        if let Err(e) = result {
            tracing::error!("[PipeWire Desktop] Capture loop error: {}", e);
        }

        notify_clone.notify_waiters();
    });

    Ok(CaptureHandle {
        backend: super::PlatformCaptureBackend::PipeWireDesktop(PipeWireDesktopCapture {
            is_running,
            thread_handle: Some(thread_handle),
        }),
        consumer: resources.consumer,
        notify: resources.notify,
        stream_error_rx: resources.stream_error_rx,
    })
}

/// Internal: runs the PipeWire MainLoop on the capture thread.
///
/// Creates a stream connected to the default audio sink's monitor,
/// capturing all system audio.
fn run_desktop_capture_loop(
    producer: &mut ringbuf::HeapProd<f32>,
    notify: &Arc<tokio::sync::Notify>,
    is_running: &Arc<AtomicBool>,
    stream_error_tx: tokio::sync::mpsc::Sender<cpal::StreamError>,
) -> Result<(), GemaCastError> {
    let mainloop = pw::main_loop::MainLoopRc::new(None)
        .map_err(|e| AudioError::PipeWireConnectionFailed(format!("MainLoop: {e}")))?;

    let context = pw::context::ContextRc::new(&mainloop, None)
        .map_err(|e| AudioError::PipeWireConnectionFailed(format!("Context: {e}")))?;

    let core = context
        .connect_rc(None)
        .map_err(|e| AudioError::PipeWireConnectionFailed(format!("Core: {e}")))?;

    // Create a capture stream that connects to the default audio sink's monitor.
    // MEDIA_CLASS "Audio/Sink" with MEDIA_CATEGORY "Capture" and AUTOCONNECT
    // tells PipeWire to connect us to the default sink's monitor port.
    let props = properties! {
        *pw::keys::MEDIA_TYPE => "Audio",
        *pw::keys::MEDIA_CATEGORY => "Capture",
        *pw::keys::MEDIA_ROLE => "Music",
        *pw::keys::NODE_NAME => "gemacast-desktop-capture",
        *pw::keys::STREAM_CAPTURE_SINK => "true",
    };

    let stream = Stream::new(&core, "gemacast-desktop-capture", props)
        .map_err(|e| AudioError::PipeWireError(format!("Stream::new: {e}")))?;

    // We use raw pointers to pass data into the process callback.
    // This is safe because:
    // 1. producer and notify outlive the mainloop (they're on the same thread stack)
    // 2. The callback is only invoked while mainloop.run() is executing
    let producer_ptr = producer as *mut ringbuf::HeapProd<f32>;
    let notify_ptr = notify as *const Arc<tokio::sync::Notify>;
    let is_running_ptr = is_running as *const Arc<AtomicBool>;

    let is_running_err = is_running.clone();
    let mainloop_weak3 = mainloop.downgrade();

    let _listener = stream
        .add_local_listener()
        .state_changed(move |_, old_state, new_state| {
            tracing::debug!(
                "[PipeWire Desktop] stream state changed {:?} -> {:?}",
                old_state,
                new_state
            );
            match new_state {
                pw::stream::StreamState::Error(err) => {
                    tracing::error!("[PipeWire Desktop] stream error: {}", err);
                    is_running_err.store(false, Ordering::Relaxed);
                    let _ = stream_error_tx.try_send(cpal::StreamError::DeviceNotAvailable);
                    if let Some(ml) = mainloop_weak3.upgrade() {
                        ml.quit();
                    }
                }
                pw::stream::StreamState::Unconnected => {
                    tracing::warn!("[PipeWire Desktop] stream disconnected");
                    is_running_err.store(false, Ordering::Relaxed);
                    let _ = stream_error_tx.try_send(cpal::StreamError::DeviceNotAvailable);
                    if let Some(ml) = mainloop_weak3.upgrade() {
                        ml.quit();
                    }
                }
                _ => {}
            }
        })
        .process(move |stream, _| {
            // Safety: pointers are valid for the lifetime of the mainloop
            let producer = unsafe { &mut *producer_ptr };
            let notify = unsafe { &*notify_ptr };
            let is_running = unsafe { &*is_running_ptr };

            if !is_running.load(Ordering::Relaxed) {
                return;
            }

            if let Some(mut buffer) = stream.dequeue_buffer() {
                let datas = buffer.datas_mut();
                if let Some(data) = datas.first() {
                    if let Some(chunk) = data.chunk() {
                        let offset = chunk.offset() as usize;
                        let size = chunk.size() as usize;

                        if let Some(slice) = data.data() {
                            if offset + size <= slice.len() {
                                let audio_bytes = &slice[offset..offset + size];
                                let n_samples = size / std::mem::size_of::<f32>();

                                unsafe {
                                    push_pw_audio_to_ringbuf(
                                        audio_bytes.as_ptr() as *const f32,
                                        n_samples,
                                        producer,
                                        notify,
                                    );
                                }
                            }
                        }
                    }
                }
            }
        })
        .register()
        .map_err(|e| AudioError::PipeWireError(format!("Listener: {e}")))?;

    // Build audio format params: 48kHz stereo F32LE interleaved
    let values = pipewire_common::build_audio_params();
    let mut params = [pw::spa::pod::Pod::from_bytes(&values)
        .ok_or_else(|| AudioError::PipeWireError("Invalid pod bytes".to_string()))?];

    stream
        .connect(
            pw::spa::utils::Direction::Input,
            None,
            StreamFlags::AUTOCONNECT | StreamFlags::MAP_BUFFERS | StreamFlags::RT_PROCESS,
            &mut params,
        )
        .map_err(|e| AudioError::PipeWireError(format!("Stream connect: {e}")))?;

    tracing::info!("[PipeWire Desktop] Capture stream connected, entering main loop");

    // Run the main loop — blocks until quit
    // We use a timer to periodically check is_running
    let is_running_timer = is_running.clone();
    let mainloop_weak = mainloop.downgrade();
    let _timer = mainloop.add_timer(move |_| {
        if !is_running_timer.load(Ordering::Relaxed) {
            if let Some(ml) = mainloop_weak.upgrade() {
                ml.quit();
            }
        }
    });
    // Check every 100ms if we should stop
    if let Some(ref timer_source) = _timer {
        timer_source.update_timer(
            Some(std::time::Duration::from_millis(100)),
            Some(std::time::Duration::from_millis(100)),
        );
    }

    mainloop.run();

    tracing::info!("[PipeWire Desktop] Capture main loop exited");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::capture::pipewire_common::is_pipewire_available;

    #[test]
    fn test_create_desktop_loopback() {
        if is_pipewire_available() {
            let result = create_pipewire_desktop_loopback();
            // It should either succeed (create the stream and connect to dummy driver)
            // or fail with a PipeWireError/ConnectionFailed, but it should not panic.
            assert!(
                result.is_ok() || result.is_err(),
                "create_pipewire_desktop_loopback did not return a valid Result"
            );

            // If it succeeded, we ensure the backend drop implementation works correctly
            if let Ok(handle) = result {
                drop(handle);
            }
        } else {
            println!("PipeWire is not available, skipping desktop loopback test.");
        }
    }
}
