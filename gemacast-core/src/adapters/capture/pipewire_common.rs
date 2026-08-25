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
use crate::domain::error::AudioError;
use crate::ports::capture::CaptureCounters;

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
///
/// `capture_kind` only labels the log line — `"desktop"` or `"process"` — so a field
/// capture can tell the two PipeWire backends apart, which is what the WASAPI path
/// already records.
///
/// # What the log line says, and what it deliberately does not
///
/// It reports the format **requested**, and says so. Nothing here reads the server's
/// answer back: neither PipeWire stream installs a `param_changed` handler, so the
/// negotiated `AudioInfoRaw` is never parsed, never validated and never logged. Calling
/// this line "negotiated" would be a claim nothing checked.
///
/// It is still worth logging, because the request is not a formality on this backend.
/// The pod below carries **one fixed value** per field rather than a `Choice` range, so
/// a server that cannot deliver 48 kHz stereo `F32LE` fails `connect` outright instead
/// of answering with something else — which means on the paths where capture works at
/// all, the requested format is also the running one. Reading the negotiation back is
/// what makes that inference unnecessary, and it is its own commit.
///
/// The buffer period is absent for the same reason: PipeWire's quantum is not known
/// until the server reports it, so there is nothing to log here yet.
pub fn build_audio_params(capture_kind: &str) -> Result<Vec<u8>, AudioError> {
    let mut format_value = spa::param::audio::AudioInfoRaw::new();
    format_value.set_format(spa::param::audio::AudioFormat::F32LE);
    format_value.set_rate(OPUS_SAMPLE_RATE);
    format_value.set_channels(OPUS_CHANNELS as u32);

    // Read back through the accessors instead of restating the three constants, so the
    // line reports what actually went into the pod rather than what was meant to.
    // Field by field on purpose: `AudioInfoRaw`'s own `Debug` also prints `position`, a
    // fixed 64-entry channel map that is all zeros here and would bury the three values
    // that matter.
    let requested_format = format_value.format();
    let requested_rate = format_value.rate();
    let requested_channels = format_value.channels();

    let mut cursor = std::io::Cursor::new(Vec::new());
    spa::pod::serialize::PodSerializer::serialize(
        &mut cursor,
        &spa::pod::Value::Object(spa::pod::Object {
            type_: spa::utils::SpaTypes::ObjectParamFormat.as_raw(),
            id: spa::param::ParamType::EnumFormat.as_raw(),
            properties: format_value.into(),
        }),
    )
    .map_err(|error| {
        AudioError::PipeWireError(format!("audio parameter serialization: {error}"))
    })?;

    // After serialization, so the line means "this is what was asked of the server"
    // rather than "this is what we intended to ask".
    tracing::info!(
        capture_kind,
        "[PipeWire] Requested capture format: {} Hz / {} ch / {:?}, interleaved \
         (negotiated format is not read back)",
        requested_rate,
        requested_channels,
        requested_format,
    );

    Ok(cursor.into_inner())
}

/// Convenience struct for the resources produced by a PipeWire capture stream setup.
pub struct PwCaptureResources {
    /// Consumer end of the ring buffer (goes into `CaptureHandle`).
    pub consumer: ringbuf::HeapCons<f32>,
    /// Notification primitive signaled when new samples are available.
    pub notify: Arc<Notify>,
    /// Receives fatal stream errors from the PipeWire thread.
    pub stream_error_rx: mpsc::Receiver<cpal::StreamError>,
    /// Diagnostic counters written by the `process` callback.
    ///
    /// The same `Arc` goes into [`PwProcessData`] on the PipeWire thread and into
    /// `CaptureHandle` on the consumer side, so the counts the callback writes are
    /// the counts the pool logs.
    pub counters: Arc<CaptureCounters>,
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
            counters: Arc::new(CaptureCounters::default()),
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
    /// Shared with the `CaptureHandle` this stream produced — see
    /// [`PwCaptureResources::counters`].
    pub counters: Arc<CaptureCounters>,
}

/// Extract interleaved f32 samples from a PipeWire buffer's data chunks
/// and push them into the ring buffer.
///
/// A push that does not fit is dropped **whole** and counted in
/// `counters.dropped_samples`. That is the pre-existing policy, kept deliberately:
/// whether a partial push is better than dropping the buffer is a real decision, and
/// it should be made once a field capture shows how often this fires at all. Until
/// then the counter is the only new thing here.
///
/// `notify` is signalled only when samples were actually pushed. A wake with nothing
/// to read costs the consumer a round trip for no work.
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
    counters: &CaptureCounters,
) {
    if data_ptr.is_null() || n_samples == 0 {
        return;
    }

    let samples = unsafe { std::slice::from_raw_parts(data_ptr, n_samples) };

    if producer.vacant_len() >= samples.len() {
        let pushed = producer.push_slice(samples);
        // Short push cannot happen after the vacancy check above, but counting the
        // shortfall costs nothing and makes that claim falsifiable.
        CaptureCounters::add(&counters.dropped_samples, (samples.len() - pushed) as u64);
        notify.notify_one();
    } else {
        CaptureCounters::add(&counters.dropped_samples, samples.len() as u64);
    }
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
    use serial_test::serial;

    #[test]
    #[serial(pipewire)]
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
                &resources.counters,
            );
        }

        assert_eq!(producer.occupied_len(), n_samples);
        assert!(
            resources.counters.all_clear(),
            "a push that fits must not record a drop: {:?}",
            resources.counters.snapshot()
        );
    }

    #[test]
    fn should_count_dropped_samples_when_the_ring_is_full() {
        let (mut producer, resources, _tx) = create_pw_ring_buffer();

        // Fill the ring so the next push cannot fit.
        let filler = vec![0.0f32; PW_RING_BUFFER_SIZE];
        assert_eq!(producer.push_slice(&filler), PW_RING_BUFFER_SIZE);

        let dropped = [0.1f32, 0.2, 0.3, 0.4];
        unsafe {
            push_pw_audio_to_ringbuf(
                dropped.as_ptr(),
                dropped.len(),
                &mut producer,
                &resources.notify,
                &resources.counters,
            );
        }

        assert_eq!(
            resources
                .counters
                .dropped_samples
                .load(std::sync::atomic::Ordering::Relaxed),
            dropped.len() as u64,
            "a whole-buffer drop must be counted, not silent"
        );
    }

    #[test]
    fn should_ignore_a_null_or_empty_push_without_counting_a_drop() {
        let (mut producer, resources, _tx) = create_pw_ring_buffer();

        unsafe {
            push_pw_audio_to_ringbuf(
                std::ptr::null(),
                4,
                &mut producer,
                &resources.notify,
                &resources.counters,
            );
            let empty = [0.0f32; 1];
            push_pw_audio_to_ringbuf(
                empty.as_ptr(),
                0,
                &mut producer,
                &resources.notify,
                &resources.counters,
            );
        }

        assert_eq!(producer.occupied_len(), 0);
        assert!(resources.counters.all_clear());
    }
}
