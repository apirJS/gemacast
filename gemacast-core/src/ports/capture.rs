//! Port: Audio capture abstractions.
//!
//! Defines the [`CaptureBackend`], [`CaptureHandle`], and [`CaptureFactory`] traits
//! that decouple the capture pipeline from platform-specific audio APIs (WASAPI, CPAL).
//!
//! The [`CaptureFactory`] trait uses an **associated type** `Backend` so that
//! `CapturePool<F>` and `AudioStreamEngine<F, N>` monomorphize to the concrete backend,
//! eliminating vtable overhead on the audio hot path.
//!
//! # Strategy Pattern
//!
//! `CaptureFactory` is the Strategy interface. Variants:
//! - [`crate::adapters::capture::DefaultCaptureFactory`] — WASAPI (Windows) / CPAL (other)
//! - Mock factories in `#[cfg(test)]` blocks
//!
//! # The capture format contract
//!
//! Every implementation in this module delivers samples in exactly one shape, and
//! **nothing downstream re-checks it** — the encoder, the jitter buffer and the wire
//! format all assume it holds:
//!
//! | property | value | constant |
//! | --- | --- | --- |
//! | sample rate | 48 000 Hz | [`OPUS_SAMPLE_RATE`](crate::audio::OPUS_SAMPLE_RATE) |
//! | channels | 2 (stereo) | [`OPUS_CHANNELS`](crate::audio::OPUS_CHANNELS) |
//! | sample type | `f32`, native endian, nominally −1.0..=1.0 | — |
//! | layout | **interleaved** `[L, R, L, R, …]` — never planar | — |
//! | frame | 480 sample-pairs = 960 `f32` values = 10 ms | [`OPUS_FRAME_SIZE`](crate::audio::OPUS_FRAME_SIZE) / [`OPUS_FRAME_SAMPLES`](crate::audio::OPUS_FRAME_SAMPLES) |
//!
//! Note which constant is which: `OPUS_FRAME_SIZE` (480) counts sample-pairs **per
//! channel** and is what platform APIs taking a "frame count" want;
//! `OPUS_FRAME_SAMPLES` (960) counts interleaved `f32` values and is what a buffer
//! length is measured in. Passing one where the other belongs asks for double or
//! half the intended period, which is silent and sounds like a latency regression.
//!
//! ## Producer obligations
//!
//! An adapter that pushes into [`CaptureHandle::consumer`]'s ring buffer must:
//!
//! 1. **Push an even number of samples on every single push.** The ring carries no
//!    channel phase of its own — it is a flat `f32` stream whose stereo pairing is
//!    implied by position. One odd-length push shifts every later sample by one
//!    slot, which swaps left and right **for the remaining lifetime of the stream**
//!    and cannot be detected downstream. A platform buffer that ends mid-pair must
//!    hold the trailing sample over and prepend it to the next push, not drop it and
//!    not push it.
//! 2. **Convert before pushing, not after.** Resampling to 48 kHz and downmixing to
//!    stereo both belong on the capture side of the ring. Use
//!    [`CaptureResampler`](crate::audio::CaptureResampler) and
//!    [`downmix_to_stereo`](crate::audio::mixdown::downmix_to_stereo) rather than
//!    reimplementing either.
//! 3. **Validate the negotiated format once, at construction**, via
//!    [`validate_capture_format`](crate::audio::mixdown::validate_capture_format),
//!    and log it unconditionally. A capture that cannot report the rate and channel
//!    count it actually ran at cannot be diagnosed from a field log.
//! 4. **Never allocate, block, or log per-sample on the capture callback.** It is a
//!    real-time thread on every platform. Counters
//!    ([`CaptureCounters`](crate::ports::capture::CaptureCounters)) are the way to
//!    report from here; something off the hot path formats them later.

use crate::domain::error::GemaCastError;
use ringbuf::HeapCons;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::{Notify, mpsc};

/// Counters shared between a capture backend and whatever reports on it.
///
/// Every field is written from the capture callback and read from elsewhere, so all
/// of them are relaxed atomics: an increment must not synchronize the audio thread
/// against anything, and a reader that observes a slightly stale total is fine —
/// these are diagnostics, not control signals.
///
/// Their whole purpose is to make silent failures visible. Before this existed, a
/// full ring buffer discarded a whole capture buffer, an unknown sample format
/// emitted zeros, and a corrupted chunk was played as audio — all three with no log
/// line and no way to tell from a field capture that anything had happened.
///
/// A **non-zero reading on any of these is the signal**, not the magnitude. Several
/// are tripwires for cases believed unreachable; the count exists so that belief is
/// falsifiable.
#[derive(Debug, Default)]
pub struct CaptureCounters {
    /// Samples discarded because the ring buffer had no room. Non-zero means the
    /// consumer is not keeping up, or a burst exceeded the ring's 640 ms.
    pub dropped_samples: AtomicU64,

    /// Bytes skipped at the head of a platform buffer to reach `f32` alignment.
    /// Tripwire: non-zero proves unaligned chunks occur in the field, which is the
    /// case the `align_to` handling exists for.
    pub unaligned_prefix_bytes: AtomicU64,

    /// Samples held over because a platform buffer ended mid-stereo-pair. Non-zero
    /// means obligation 1 above is load-bearing on this platform.
    pub truncated_samples: AtomicU64,

    /// Reads from the ring buffer that returned an odd number of samples, i.e. a
    /// producer that broke obligation 1.
    ///
    /// This is the consumer-side counterpart to `truncated_samples`, and the two say
    /// different things: `truncated_samples` counts a producer *honouring* the
    /// obligation by holding a sample back, while this counts one *breaking* it. A
    /// non-zero reading here means left and right are swapped from that point on for
    /// the rest of the stream, which is inaudible as a defect — it sounds like a
    /// stereo image, just the wrong one — and is undetectable anywhere downstream.
    pub odd_ring_reads: AtomicU64,

    /// Platform buffers flagged corrupted by the driver and skipped.
    pub corrupted_chunks: AtomicU64,

    /// Platform buffers flagged as silent, for which decode was skipped.
    pub silent_buffers: AtomicU64,

    /// Platform buffers whose sample format was not recognised, emitted as silence.
    /// Tripwire: non-zero means format negotiation produced something the decoder
    /// does not handle, and the audio is silence rather than sound.
    pub unknown_format_buffers: AtomicU64,

    /// Fatal stream errors that could not be delivered because the error channel was
    /// already occupied. The first error is the actionable one; this counts the rest.
    pub dropped_stream_errors: AtomicU64,
}

impl CaptureCounters {
    /// Add to a counter from the capture callback.
    ///
    /// Relaxed because these are diagnostics: no downstream decision reads them, so
    /// there is nothing to order against, and the audio thread must not pay for a
    /// fence here.
    #[inline]
    pub fn add(counter: &AtomicU64, n: u64) {
        counter.fetch_add(n, Ordering::Relaxed);
    }

    /// Snapshot every counter as `(name, value)` pairs for logging.
    ///
    /// Allocates, so call this from the reporting path — never from the capture
    /// callback.
    pub fn snapshot(&self) -> Vec<(&'static str, u64)> {
        vec![
            (
                "dropped_samples",
                self.dropped_samples.load(Ordering::Relaxed),
            ),
            (
                "unaligned_prefix_bytes",
                self.unaligned_prefix_bytes.load(Ordering::Relaxed),
            ),
            (
                "truncated_samples",
                self.truncated_samples.load(Ordering::Relaxed),
            ),
            (
                "odd_ring_reads",
                self.odd_ring_reads.load(Ordering::Relaxed),
            ),
            (
                "corrupted_chunks",
                self.corrupted_chunks.load(Ordering::Relaxed),
            ),
            (
                "silent_buffers",
                self.silent_buffers.load(Ordering::Relaxed),
            ),
            (
                "unknown_format_buffers",
                self.unknown_format_buffers.load(Ordering::Relaxed),
            ),
            (
                "dropped_stream_errors",
                self.dropped_stream_errors.load(Ordering::Relaxed),
            ),
        ]
    }

    /// True when every counter is still zero — the expected steady state.
    pub fn all_clear(&self) -> bool {
        self.snapshot().iter().all(|(_, v)| *v == 0)
    }
}

/// Controls an active audio capture stream (play/pause lifecycle).
///
/// Implementations wrap platform-specific stream handles (WASAPI `IAudioClient`,
/// CPAL `Stream`, Oboe `AudioStream`).
///
/// Implementors must honour the producer obligations documented at
/// [module level](self#producer-obligations) — in particular that every push into the
/// ring buffer carries an even number of samples.
pub trait CaptureBackend: Send {
    /// Start capturing audio samples into the associated ring buffer.
    fn play(&mut self) -> Result<(), GemaCastError>;

    /// Pause the capture stream. Samples stop flowing to the ring buffer.
    fn pause(&mut self) -> Result<(), GemaCastError>;
}

/// A constructed capture pipeline ready to be driven by [`CapturePool`](crate::adapters::capture_pool::CapturePool).
///
/// Generic over `B` so the backend is known at compile time (static dispatch).
/// The `CapturePool` erases `B` at the point of spawning the capture task,
/// so `AudioCaptureInstance` itself remains non-generic.
pub struct CaptureHandle<B: CaptureBackend> {
    /// The platform capture backend (WASAPI, CPAL, mock).
    pub backend: B,

    /// Consumer end of the ring buffer that receives raw f32 PCM samples
    /// from the backend's capture thread/callback.
    ///
    /// **48 kHz, stereo, interleaved `f32`** — see the [format
    /// contract](self#the-capture-format-contract). The stream is flat: stereo
    /// pairing is implied by position, so a producer that pushes an odd number of
    /// samples swaps the channels for the rest of the session.
    pub consumer: HeapCons<f32>,

    /// Notification primitive signaled by the backend when new samples
    /// are available in the ring buffer.
    ///
    /// Signal it only when samples were actually pushed. A wake with nothing to read
    /// costs the consumer a round trip for no work.
    pub notify: Arc<Notify>,

    /// Receives fatal stream errors from the backend (e.g., device unplugged).
    pub stream_error_rx: mpsc::Receiver<cpal::StreamError>,

    /// Diagnostic counters written by the capture callback.
    ///
    /// Read and logged off the hot path — see [`CaptureCounters`].
    pub counters: Arc<CaptureCounters>,
}

/// Factory that creates capture backends (Strategy Pattern).
///
/// The associated type `Backend` allows `CapturePool<F>` to monomorphize
/// the entire capture pipeline at compile time.
///
/// # Strategy variants
///
/// | Implementation | Backend | Platform |
/// |---|---|---|
/// | `DefaultCaptureFactory` | `PlatformCaptureBackend` (enum) | Windows / Desktop Linux |
/// | `MockCaptureFactory` | `MockCaptureBackend` | Tests |
pub trait CaptureFactory: Send + Sync {
    /// The concrete capture backend type produced by this factory.
    type Backend: CaptureBackend + 'static;

    /// Create a capture handle for the system-wide desktop audio mix.
    fn create_desktop_capture(&self) -> Result<CaptureHandle<Self::Backend>, GemaCastError>;

    /// Create a capture handle for a specific process's audio output.
    ///
    /// # Platform support
    ///
    /// Only available on Windows (WASAPI process loopback). Other platforms
    /// should return [`AudioError::ProcessCaptureUnavailable`](crate::domain::error::AudioError::ProcessCaptureUnavailable).
    fn create_process_capture(
        &self,
        pid: u32,
    ) -> Result<CaptureHandle<Self::Backend>, GemaCastError>;
}
