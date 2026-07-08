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

/// Internal: runs the PipeWire ThreadLoop on the capture thread.
///
/// Creates a stream connected to the default audio sink's monitor,
/// capturing all system audio.
///
/// Uses [`ThreadLoopBox`] so PipeWire's event loop runs on a background
/// thread, and all proxy operations happen under the thread loop's lock
/// (required by PipeWire's context-safety model).
fn run_desktop_capture_loop(
    producer: &mut ringbuf::HeapProd<f32>,
    notify: &Arc<tokio::sync::Notify>,
    is_running: &Arc<AtomicBool>,
    stream_error_tx: tokio::sync::mpsc::Sender<cpal::StreamError>,
) -> Result<(), GemaCastError> {
    pw::init();

    let mainloop =
        unsafe { pw::thread_loop::ThreadLoopBox::new(Some("gemacast-desktop-capture"), None) }
            .map_err(|e| AudioError::PipeWireError(format!("Failed to create thread loop: {e}")))?;

    let context = pw::context::ContextBox::new(mainloop.loop_(), None)
        .map_err(|e| AudioError::PipeWireError(format!("Context: {e}")))?;

    // Start the thread loop so PipeWire processes events in the background.
    // We must hold the lock while creating proxies and connecting the stream.
    mainloop.start();
    let loop_guard = mainloop.lock();

    let core = context
        .connect(None)
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
    // 2. The callback is only invoked while the thread loop is running
    let producer_ptr = producer as *mut ringbuf::HeapProd<f32>;
    let notify_ptr = notify as *const Arc<tokio::sync::Notify>;
    let is_running_ptr = is_running as *const Arc<AtomicBool>;

    let is_running_err = is_running.clone();

    let _listener = stream
        .add_local_listener::<()>()
        .state_changed(move |_, _, old_state, new_state| {
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
                }
                pw::stream::StreamState::Unconnected => {
                    tracing::warn!("[PipeWire Desktop] stream disconnected");
                    is_running_err.store(false, Ordering::Relaxed);
                    let _ = stream_error_tx.try_send(cpal::StreamError::DeviceNotAvailable);
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
                if let Some(data) = datas.first_mut() {
                    let chunk = data.chunk();
                    let offset = chunk.offset() as usize;
                    let size = chunk.size() as usize;

                    if let Some(slice) = data.data()
                        && offset + size <= slice.len()
                    {
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

    // Release the lock so the PipeWire thread can process events
    drop(loop_guard);

    // Block the current thread until is_running goes false
    while is_running.load(Ordering::Relaxed) {
        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    tracing::info!("[PipeWire Desktop] Capture main loop exited");

    // 1. Lock the loop to safely destroy proxies and context
    let loop_guard = mainloop.lock();
    drop(_listener);
    drop(stream);
    drop(core);
    drop(context);
    drop(loop_guard);

    // 2. Stop the background thread (joins it)
    mainloop.stop();

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::capture::pipewire_common::is_pipewire_available;
    use serial_test::serial;

    /// Generate a silent WAV file (48 kHz, stereo, s16) and return its path.
    fn create_silent_wav(duration_secs: u32) -> String {
        let sample_rate: u32 = 48000;
        let channels: u16 = 2;
        let bits_per_sample: u16 = 16;
        let byte_rate = sample_rate * channels as u32 * bits_per_sample as u32 / 8;
        let block_align = channels * bits_per_sample / 8;
        let data_size = byte_rate * duration_secs;
        let file_size = 36 + data_size;

        let path = std::env::temp_dir().join(format!(
            "gemacast_ci_silence_{}.wav",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));

        let mut buf: Vec<u8> = Vec::with_capacity(44);
        buf.extend_from_slice(b"RIFF");
        buf.extend_from_slice(&file_size.to_le_bytes());
        buf.extend_from_slice(b"WAVE");
        buf.extend_from_slice(b"fmt ");
        buf.extend_from_slice(&16u32.to_le_bytes());
        buf.extend_from_slice(&1u16.to_le_bytes());
        buf.extend_from_slice(&channels.to_le_bytes());
        buf.extend_from_slice(&sample_rate.to_le_bytes());
        buf.extend_from_slice(&byte_rate.to_le_bytes());
        buf.extend_from_slice(&block_align.to_le_bytes());
        buf.extend_from_slice(&bits_per_sample.to_le_bytes());
        buf.extend_from_slice(b"data");
        buf.extend_from_slice(&data_size.to_le_bytes());

        use std::io::Write;
        let mut file = std::fs::File::create(&path).expect("failed to create WAV file");
        file.write_all(&buf).expect("failed to write WAV header");
        let chunk = vec![0u8; 65536];
        let mut remaining = data_size as usize;
        while remaining > 0 {
            let n = remaining.min(chunk.len());
            file.write_all(&chunk[..n])
                .expect("failed to write WAV data");
            remaining -= n;
        }
        path.to_string_lossy().into_owned()
    }

    #[test]
    #[serial(pipewire)]
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

    /// End-to-end test: verifies that desktop capture actually receives audio samples.
    ///
    /// 1. Creates a null audio sink so PipeWire has somewhere to route audio
    /// 2. Spawns pw-cat playing infinite silence through that sink
    /// 3. Creates a desktop capture stream (which monitors the sink)
    /// 4. Waits for samples to appear in the ring buffer consumer
    /// 5. Asserts that we received > 0 samples
    #[test]
    #[serial(pipewire)]
    fn test_desktop_capture_receives_audio() {
        if !is_pipewire_available() {
            println!("PipeWire is not available, skipping desktop capture receives audio test.");
            return;
        }

        // Create a dummy null sink for the headless CI environment
        let _ = std::process::Command::new("pw-cli")
            .args([
                "create-node",
                "adapter",
                "{ factory.name=support.null-audio-sink node.name=\"ci-desktop-sink\" media.class=Audio/Sink object.linger=true }",
            ])
            .status();

        std::thread::sleep(std::time::Duration::from_millis(300));

        // Spawn pw-cat to play silence through the sink, generating audio traffic.
        // Use a proper WAV file because CI's pw-cat uses libsndfile which needs a container header.
        let wav_path = create_silent_wav(30);
        let mut child = match std::process::Command::new("pw-cat")
            .arg("-p")
            .arg(&wav_path)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
        {
            Ok(child) => child,
            Err(e) => {
                let _ = std::fs::remove_file(&wav_path);
                println!(
                    "Failed to spawn pw-cat ({}), skipping desktop capture receives audio test.",
                    e
                );
                return;
            }
        };

        // Give WirePlumber time to set up the node and routing
        std::thread::sleep(std::time::Duration::from_millis(1000));

        // Verify pw-cat is still running
        if let Ok(Some(status)) = child.try_wait() {
            let mut stderr_str = String::new();
            if let Some(mut stderr) = child.stderr.take() {
                use std::io::Read;
                let _ = stderr.read_to_string(&mut stderr_str);
            }
            let _ = std::fs::remove_file(&wav_path);
            panic!(
                "pw-cat exited prematurely with status {:?}. Stderr: {}",
                status, stderr_str
            );
        }

        // Create the desktop capture
        let result = create_pipewire_desktop_loopback();
        assert!(
            result.is_ok(),
            "Expected desktop capture to succeed, got {:?}",
            result.err()
        );

        let CaptureHandle {
            backend,
            mut consumer,
            notify: _notify,
            stream_error_rx: _stream_error_rx,
        } = result.unwrap();

        // Wait for audio samples to arrive in the consumer ring buffer.
        // The capture runs on a dedicated PipeWire thread, so we poll the
        // consumer side with a timeout.
        let mut total_samples = 0usize;
        let mut scratch = vec![0.0f32; 4096];
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);

        use ringbuf::traits::*;
        while std::time::Instant::now() < deadline {
            let n = consumer.pop_slice(&mut scratch);
            total_samples += n;
            if total_samples > 0 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }

        assert!(
            total_samples > 0,
            "Desktop capture did not receive any audio samples within the timeout"
        );

        // Clean up: drop the capture backend first, then kill pw-cat
        drop(backend);
        let _ = child.kill();
        let _ = child.wait();
        let _ = std::fs::remove_file(&wav_path);
    }
}
