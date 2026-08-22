#![cfg(target_os = "macos")]

//! Shared ScreenCaptureKit utilities for audio capture.
//!
//! Provides the audio output handler, configuration builder, error mapping, and
//! dispatch queue shared by the desktop and per-process capture backends.
//!
//! # Audio Data Flow
//!
//! ScreenCaptureKit delivers audio as `CMSampleBuffer` objects via the
//! `SCStreamOutputTrait::did_output_sample_buffer` callback. Each buffer carries an
//! `AudioBufferList` whose **layout is not interleaved**: SCK emits
//! `AUDIO_FORMAT_FLOAT_PLANAR`, one `AudioBuffer` per channel (buffer 0 = left,
//! buffer 1 = right), exactly as OBS's shipping code documents
//! (`TEMP/obs-studio/plugins/mac-capture/mac-sck-common.m:318`). The pipeline wants
//! **interleaved** `[L, R, L, R, …]`, so the callback interleaves the planar halves.
//! A single-buffer list is treated as already-interleaved stereo and passed through.
//!
//! The byte-level extraction (planar interleave, the odd-sample carry, alignment
//! counting) lives in the platform-neutral
//! [`sck_buffers_to_interleaved_stereo`](crate::audio::mixdown::sck_buffers_to_interleaved_stereo)
//! so it is unit-tested on every CI leg; this module is the SCK glue around it.

use crate::audio::mixdown::sck_buffers_to_interleaved_stereo;
use crate::audio::{OPUS_CHANNELS, OPUS_FRAME_SAMPLES, OPUS_SAMPLE_RATE};
use crate::domain::error::{AudioError, GemaCastError};
use crate::ports::capture::CaptureCounters;
use ringbuf::{HeapRb, traits::*};
use std::sync::{Arc, Mutex, Once};
use tokio::sync::{Notify, mpsc};

use screencapturekit::dispatch_queue::{DispatchQoS, DispatchQueue};
use screencapturekit::prelude::*;
use screencapturekit::utils::error::SCError;

/// Size of the ring buffer in samples (shared between all SCK backends).
/// Same sizing as WASAPI/PipeWire: 64 Opus frames worth of stereo f32 samples.
pub const SCK_RING_BUFFER_SIZE: usize = OPUS_FRAME_SAMPLES * 64;

/// Map a ScreenCaptureKit error to the crate's error type.
///
/// Prefers the typed [`SCError`] variant over string matching: `PermissionDenied`
/// becomes the dedicated [`AudioError::ScreenCapturePermissionDenied`] so the UI can
/// prompt for Screen Recording access; every other variant carries its display text.
/// This replaces three copies of a fragile `msg.contains("permission")` scan that
/// depended on the wording of an error string nobody controls.
pub fn map_sck_error(error: SCError) -> GemaCastError {
    match error {
        SCError::PermissionDenied(_) => {
            GemaCastError::Audio(AudioError::ScreenCapturePermissionDenied)
        }
        // `SCError` is `#[non_exhaustive]`; the wildcard is required and also covers
        // every non-permission variant with its `Display` text.
        other => GemaCastError::Audio(AudioError::ScreenCaptureKitError(other.to_string())),
    }
}

/// Create the serial dispatch queue SCK delivers audio callbacks on.
///
/// SCK invokes `did_output_sample_buffer` on a serial queue; naming it explicitly at
/// `UserInteractive` QoS documents the real-time intent and keeps the callback off a
/// low-priority default rather than leaving the queue implicit (`None`). The returned
/// queue **must outlive the stream** — the caller stores it in the capture backend
/// struct, and because [`DispatchQueue`] is reference-counted (`Clone` bumps, `Drop`
/// releases) holding one there cannot dangle.
pub fn create_sck_capture_queue() -> DispatchQueue {
    DispatchQueue::new("com.apir.gemacast.capture", DispatchQoS::UserInteractive)
}

/// Convenience struct for the resources produced by a SCK capture stream setup.
pub struct SckCaptureResources {
    /// Consumer end of the ring buffer (goes into `CaptureHandle`).
    pub consumer: ringbuf::HeapCons<f32>,
    /// Notification primitive signaled when new samples are available.
    pub notify: Arc<Notify>,
    /// Receives fatal stream errors from the capture thread.
    pub stream_error_rx: mpsc::Receiver<cpal::StreamError>,
    /// Diagnostic counters written by the capture callback.
    ///
    /// The same `Arc` is cloned into [`SckAudioHandler`] on the callback side and kept
    /// here for the `CaptureHandle`, so the counts the callback writes are the counts
    /// the pool logs — the shape [`crate::adapters::capture::pipewire_common`] uses.
    pub counters: Arc<CaptureCounters>,
}

/// Create the ring buffer and associated synchronization primitives
/// for a SCK capture stream.
pub fn create_sck_ring_buffer() -> (
    ringbuf::HeapProd<f32>,
    mpsc::Sender<cpal::StreamError>,
    SckCaptureResources,
) {
    let rb = HeapRb::<f32>::new(SCK_RING_BUFFER_SIZE);
    let (producer, consumer) = rb.split();
    let notify = Arc::new(Notify::new());
    let (stream_error_tx, stream_error_rx) = mpsc::channel::<cpal::StreamError>(1);

    (
        producer,
        stream_error_tx,
        SckCaptureResources {
            consumer,
            notify,
            stream_error_rx,
            counters: Arc::new(CaptureCounters::default()),
        },
    )
}

/// Build a standard `SCStreamConfiguration` optimized for audio-only capture.
///
/// Configures:
/// - Audio capture enabled at 48kHz stereo (matching the Opus pipeline)
/// - Current process audio excluded (prevents feedback loops)
/// - Minimal video dimensions (1x1) since we only want audio
///
/// SCK is free to *ignore* the sample-rate/channel request and fall back to its own
/// default (48 kHz / stereo for unsupported values), so nothing downstream trusts
/// these numbers — the callback derives the channel layout from the buffer count it
/// actually receives, and logs it once (see [`SckAudioHandler`]).
pub fn create_sck_audio_config() -> SCStreamConfiguration {
    SCStreamConfiguration::new()
        .with_captures_audio(true)
        .with_excludes_current_process_audio(true)
        .with_sample_rate(OPUS_SAMPLE_RATE as i32)
        .with_channel_count(OPUS_CHANNELS as i32)
        .with_width(1) // Minimal — we only capture audio
        .with_height(1)
}

/// Producer-side state the capture callback mutates, guarded by a single lock.
///
/// `carry` holds a trailing sample when a platform buffer ends mid-stereo-pair, so the
/// next callback prepends it — obligation 1 of the [capture format
/// contract](crate::ports::capture). `scratch` is reused across callbacks so the
/// interleave allocates nothing on the hot path.
struct HandlerState {
    producer: ringbuf::HeapProd<f32>,
    /// Odd trailing sample held over to the next callback; see [`HandlerState`].
    carry: Option<f32>,
    /// Reused interleave buffer — cleared and refilled each callback.
    scratch: Vec<f32>,
}

/// Audio output handler that receives `CMSampleBuffer` from ScreenCaptureKit
/// and pushes interleaved f32 PCM samples into the ring buffer.
///
/// Implements `SCStreamOutputTrait`, so it must be `Send + Sync` — Apple's dispatch
/// queue invokes the callback from a thread we do not own.
///
/// # Real-time discipline
///
/// The callback runs on an audio dispatch queue and must never block. The producer,
/// carry and scratch buffer live behind a [`Mutex`] accessed with `try_lock`: the
/// queue is serial so the lock is effectively uncontended, and `try_lock` guarantees
/// that even a pathological race (a teardown on another thread) drops the buffer and
/// counts it rather than parking the audio thread.
pub struct SckAudioHandler {
    state: Mutex<HandlerState>,
    notify: Arc<Notify>,
    counters: Arc<CaptureCounters>,
    /// `"sck-desktop"` / `"sck-process"` — labels the one-shot format log so a field
    /// capture can tell the two backends apart, matching the PipeWire `capture_kind`.
    label: &'static str,
    /// Logs the observed buffer layout on the first callback only. Once-gated so it
    /// never fires per-sample on the hot path.
    format_logged: Once,
}

impl SckAudioHandler {
    pub fn new(
        producer: ringbuf::HeapProd<f32>,
        notify: Arc<Notify>,
        counters: Arc<CaptureCounters>,
        label: &'static str,
    ) -> Self {
        Self {
            state: Mutex::new(HandlerState {
                producer,
                carry: None,
                scratch: Vec::with_capacity(OPUS_FRAME_SAMPLES),
            }),
            notify,
            counters,
            label,
            format_logged: Once::new(),
        }
    }
}

impl SCStreamOutputTrait for SckAudioHandler {
    fn did_output_sample_buffer(&self, sample: CMSampleBuffer, of_type: SCStreamOutputType) {
        // Only audio; a dummy Screen output (if one is ever added to silence SCK) lands
        // here too and is discarded.
        if of_type != SCStreamOutputType::Audio {
            return;
        }

        let Some(list) = sample.audio_buffer_list() else {
            return;
        };

        // One `&[u8]` per channel buffer. The iterator item borrows from `list` (not the
        // loop temporary), so this collect is lifetime-safe; it is a 1–2 element bounded
        // allocation — the *lock*, not this, is the real-time hazard the design guards.
        let buffers: Vec<&[u8]> = list.iter().map(|b| b.data()).collect();

        // Record the layout SCK actually delivered, exactly once. `AudioBuffer` does
        // expose a public `number_channels` field, but it describes the *buffer*, not the
        // list, so it cannot distinguish planar from interleaved on its own — the buffer
        // count is the layout signal: 2 → planar (interleaved here), 1 → already
        // interleaved stereo. Both are logged so a field capture can cross-check them.
        self.format_logged.call_once(|| {
            let byte_sizes: Vec<usize> = buffers.iter().map(|b| b.len()).collect();
            let channels_per_buffer: Vec<u32> = list.iter().map(|b| b.number_channels).collect();
            let layout = match buffers.len() {
                1 => "interleaved stereo (1 buffer)",
                2 => "planar L/R (2 buffers → interleaved)",
                _ => "UNRECOGNIZED (counted as unknown_format_buffers)",
            };
            tracing::info!(
                capture_kind = self.label,
                "[SCK] first audio callback: {} buffer(s), byte sizes {:?}, \
                 number_channels {:?}, treated as {}",
                buffers.len(),
                byte_sizes,
                channels_per_buffer,
                layout,
            );
        });

        let Ok(mut guard) = self.state.try_lock() else {
            // Unreachable on a serial queue (nothing else locks `state`), but if it ever
            // happens the only non-blocking choice is to drop — counted, never silent.
            let dropped: usize = buffers
                .iter()
                .map(|b| b.len() / std::mem::size_of::<f32>())
                .sum();
            CaptureCounters::add(&self.counters.dropped_samples, dropped as u64);
            return;
        };
        let state = &mut *guard;

        // Planar-or-interleaved interleave, carry handling, alignment + orphan counting
        // all live in the tested pure function.
        sck_buffers_to_interleaved_stereo(
            &buffers,
            &mut state.carry,
            &mut state.scratch,
            &self.counters,
        );

        if state.scratch.is_empty() {
            return;
        }

        // Whole-buffer drop policy on a full ring, mirroring the WASAPI/PipeWire paths,
        // and counted in `dropped_samples`. Notify only when something was pushed — a
        // wake with nothing to read costs the consumer a round trip for no work.
        if state.producer.vacant_len() >= state.scratch.len() {
            let pushed = state.producer.push_slice(&state.scratch);
            CaptureCounters::add(
                &self.counters.dropped_samples,
                (state.scratch.len() - pushed) as u64,
            );
            self.notify.notify_one();
        } else {
            CaptureCounters::add(&self.counters.dropped_samples, state.scratch.len() as u64);
        }
    }
}
