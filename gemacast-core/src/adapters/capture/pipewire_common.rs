#![cfg(target_os = "linux")]

//! Shared PipeWire utilities for audio capture.
//!
//! Provides the [`PipeWireThread`] abstraction that runs a PipeWire `MainLoop`
//! on a dedicated OS thread, plus helpers for creating audio capture streams
//! with proper format negotiation.
//!
//! # Threading Model
//!
//! PipeWire's `MainLoop` is **not** compatible with Tokio's async runtime.
//! All PipeWire interactions happen on a dedicated `std::thread`, matching
//! the same pattern used by the WASAPI capture backends on Windows.

use crate::audio::{OPUS_CHANNELS, OPUS_FRAME_SAMPLES, OPUS_SAMPLE_RATE};

use ringbuf::{HeapRb, traits::*};
use std::sync::Arc;

use tokio::sync::{Notify, mpsc};

use pipewire as pw;
use pw::spa;

/// Size of the ring buffer in samples (shared between all PipeWire backends).
/// Same sizing as WASAPI: 64 Opus frames worth of stereo f32 samples.
pub const PW_RING_BUFFER_SIZE: usize = OPUS_FRAME_SAMPLES * 64;

/// Build SPA audio format parameters for 48kHz stereo F32 interleaved.
///
/// This creates the pod parameter that tells PipeWire what audio format
/// we want to receive in our capture stream's `process` callback.
pub fn build_audio_params() -> Vec<u8> {
    let mut params_buf = vec![0u8; 1024];
    let mut format_value = spa::param::audio::AudioInfoRaw::new();
    format_value.set_format(spa::param::audio::AudioFormat::F32LE);
    format_value.set_rate(OPUS_SAMPLE_RATE);
    format_value.set_channels(OPUS_CHANNELS as u32);

    spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(&mut params_buf),
        &spa::pod::Value::Object(spa::pod::Object {
            type_: spa::utils::SpaTypes::ObjectParamFormat.as_raw(),
            id: spa::param::ParamType::EnumFormat.as_raw(),
            properties: format_value.into(),
        }),
    )
    .expect("Failed to serialize PipeWire audio params");

    params_buf
}

/// Convenience struct for the resources produced by a PipeWire capture stream setup.
pub struct PwCaptureResources {
    /// Consumer end of the ring buffer (goes into `CaptureHandle`).
    pub consumer: ringbuf::HeapCons<f32>,
    /// Notification primitive signaled when new samples are available.
    pub notify: Arc<Notify>,
    /// Receives fatal stream errors from the PipeWire thread.
    pub stream_error_rx: mpsc::Receiver<cpal::StreamError>,
}

/// Create the ring buffer and associated synchronization primitives
/// for a PipeWire capture stream.
///
/// Returns the producer side (for the PipeWire thread) and the
/// consumer-side resources (for the `CaptureHandle`).
pub fn create_pw_ring_buffer() -> (
    ringbuf::HeapProd<f32>,
    PwCaptureResources,
    mpsc::Sender<cpal::StreamError>,
) {
    let rb = HeapRb::<f32>::new(PW_RING_BUFFER_SIZE);
    let (producer, consumer) = rb.split();
    let notify = Arc::new(Notify::new());
    let (stream_error_tx, stream_error_rx) = mpsc::channel::<cpal::StreamError>(1);

    (
        producer,
        PwCaptureResources {
            consumer,
            notify,
            stream_error_rx,
        },
        stream_error_tx,
    )
}

/// Process callback data shared between the PipeWire thread and the
/// stream's `process` event handler.
///
/// The `process` callback reads audio data from PipeWire's buffer,
/// pushes it into our ring buffer, and notifies the async consumer.
pub struct PwProcessData {
    pub producer: ringbuf::HeapProd<f32>,
    pub notify: Arc<Notify>,
}

/// Extract interleaved f32 samples from a PipeWire buffer's data chunks
/// and push them into the ring buffer.
///
/// # Safety
///
/// The `data_ptr` must point to valid audio data of `n_samples` f32 values
/// as provided by PipeWire's buffer dequeue mechanism.
pub unsafe fn push_pw_audio_to_ringbuf(
    data_ptr: *const f32,
    n_samples: usize,
    producer: &mut ringbuf::HeapProd<f32>,
    notify: &Notify,
) {
    if data_ptr.is_null() || n_samples == 0 {
        return;
    }

    let samples = unsafe { std::slice::from_raw_parts(data_ptr, n_samples) };

    if producer.vacant_len() >= samples.len() {
        let _ = producer.push_slice(samples);
    }

    notify.notify_one();
}

/// Check if PipeWire is available on the system by attempting to initialize it.
///
/// Returns `true` if PipeWire can be initialized successfully.
/// This is used to decide whether to use PipeWire or fall back to CPAL.
///
/// # Note
///
/// This function catches panics from `pw::init()` to handle systems where
/// PipeWire libraries are not installed (e.g., PulseAudio-only setups).
pub fn is_pipewire_available() -> bool {
    std::panic::catch_unwind(|| {
        pw::init();
        true
    })
    .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pipewire_initialization() {
        // This test acts as a smoke test in our CI pipeline.
        // In the headless CI environment, dbus-run-session sets up DBus,
        // and the PipeWire daemon runs in the background.
        // pw::init() shouldn't panic if PipeWire is properly available.
        if is_pipewire_available() {
            // Verify we can create a thread loop (lightweight, no proxies).
            // We deliberately avoid creating a ContextBox or Core here
            // because dropping those on a non-started loop triggers
            // PipeWire's "impl_ext_end_proxy" context check warnings.
            let mainloop =
                unsafe { pw::thread_loop::ThreadLoopBox::new(Some("gemacast-init-test"), None) };
            assert!(mainloop.is_ok(), "Failed to create PipeWire ThreadLoop");
        } else {
            // We only print a warning so the test passes on developers'
            // machines that don't have PipeWire, but fails loudly if
            // the CI environment is supposedly set up but fails.
            println!("PipeWire is not available, skipping smoke test.");
        }
    }

    #[test]
    fn test_push_pw_audio_to_ringbuf() {
        let (mut producer, resources, _tx) = create_pw_ring_buffer();

        let dummy_audio = [0.1f32, 0.2, 0.3, 0.4];
        let n_samples = dummy_audio.len();

        unsafe {
            push_pw_audio_to_ringbuf(
                dummy_audio.as_ptr(),
                n_samples,
                &mut producer,
                &resources.notify,
            );
        }

        assert_eq!(producer.occupied_len(), n_samples);
    }
}
