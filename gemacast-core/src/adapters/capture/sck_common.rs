#![cfg(target_os = "macos")]

//! Shared ScreenCaptureKit utilities for audio capture.
//!
//! Provides the audio output handler, configuration builder, and PCM data
//! extraction logic shared by the desktop and per-process capture backends.
//!
//! # Audio Data Flow
//!
//! ScreenCaptureKit delivers audio as `CMSampleBuffer` objects via the
//! `SCStreamOutputTrait::did_output_sample_buffer` callback. Each buffer
//! contains interleaved f32 PCM samples at the configured sample rate.
//! We extract these samples and push them into the lock-free ring buffer
//! for consumption by the CapturePool.

use crate::audio::{OPUS_CHANNELS, OPUS_FRAME_SAMPLES, OPUS_SAMPLE_RATE};
use ringbuf::{HeapRb, traits::*};
use std::sync::Arc;
use tokio::sync::{Notify, mpsc};

use screencapturekit::prelude::*;

/// Size of the ring buffer in samples (shared between all SCK backends).
/// Same sizing as WASAPI/PipeWire: 64 Opus frames worth of stereo f32 samples.
pub const SCK_RING_BUFFER_SIZE: usize = OPUS_FRAME_SAMPLES * 64;

/// Convenience struct for the resources produced by a SCK capture stream setup.
pub struct SckCaptureResources {
    /// Consumer end of the ring buffer (goes into `CaptureHandle`).
    pub consumer: ringbuf::HeapCons<f32>,
    /// Notification primitive signaled when new samples are available.
    pub notify: Arc<Notify>,
    /// Receives fatal stream errors from the capture thread.
    pub stream_error_rx: mpsc::Receiver<cpal::StreamError>,
    /// Diagnostic counters for the capture callback.
    ///
    /// Nothing writes these yet — the SCK handler is not wired to them, and this
    /// backend is disabled pending the 14.2+ process-tap work. The field exists so the
    /// port surface is the same shape on every platform.
    pub counters: Arc<crate::ports::capture::CaptureCounters>,
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
            counters: Arc::new(crate::ports::capture::CaptureCounters::default()),
        },
    )
}

/// Build a standard `SCStreamConfiguration` optimized for audio-only capture.
///
/// Configures:
/// - Audio capture enabled at 48kHz stereo (matching the Opus pipeline)
/// - Current process audio excluded (prevents feedback loops)
/// - Minimal video dimensions (1x1) since we only want audio
pub fn create_sck_audio_config() -> SCStreamConfiguration {
    SCStreamConfiguration::new()
        .with_captures_audio(true)
        .with_excludes_current_process_audio(true)
        .with_sample_rate(OPUS_SAMPLE_RATE as i32)
        .with_channel_count(OPUS_CHANNELS as i32)
        .with_width(1) // Minimal — we only capture audio
        .with_height(1)
}

/// Audio output handler that receives `CMSampleBuffer` from ScreenCaptureKit
/// and pushes f32 PCM samples into the ring buffer.
///
/// This struct implements `SCStreamOutputTrait` and must be `Send + Sync`
/// because Apple's dispatch queues invoke the callback from arbitrary threads.
pub struct SckAudioHandler {
    producer: std::sync::Mutex<ringbuf::HeapProd<f32>>,
    notify: Arc<Notify>,
}

impl SckAudioHandler {
    pub fn new(producer: ringbuf::HeapProd<f32>, notify: Arc<Notify>) -> Self {
        Self {
            producer: std::sync::Mutex::new(producer),
            notify,
        }
    }
}

impl SCStreamOutputTrait for SckAudioHandler {
    fn did_output_sample_buffer(&self, sample: CMSampleBuffer, of_type: SCStreamOutputType) {
        // Only process audio samples, ignore video frames
        if of_type != SCStreamOutputType::Audio {
            return;
        }

        // Extract audio data from the CMSampleBuffer.
        // ScreenCaptureKit delivers audio as interleaved f32 PCM
        // at the sample rate we configured (48kHz stereo).
        if let Some(audio_data) = extract_audio_f32_from_sample_buffer(&sample) {
            if let Ok(mut producer) = self.producer.lock()
                && producer.vacant_len() >= audio_data.len()
            {
                let _ = producer.push_slice(&audio_data);
            }
            self.notify.notify_one();
        }
    }
}

/// Extract interleaved f32 PCM samples from a `CMSampleBuffer`.
///
/// ScreenCaptureKit delivers audio buffers containing `AudioBufferList` data.
/// This function:
/// 1. Gets the audio buffer list from the sample buffer
/// 2. Interprets the raw bytes from the first buffer as f32 samples
/// 3. Returns the samples as a Vec (already interleaved by SCK config)
///
/// Returns `None` if the sample buffer contains no valid audio data.
fn extract_audio_f32_from_sample_buffer(sample: &CMSampleBuffer) -> Option<Vec<f32>> {
    // Get the audio buffer list data from the sample buffer.
    // The CMSampleBuffer provides access to the underlying AudioBufferList
    // which contains the PCM audio samples.
    let audio_list = sample.audio_buffer_list()?;
    let buffer = audio_list.iter().next()?;
    let data_bytes = buffer.data();

    if data_bytes.is_empty() {
        return None;
    }

    // The audio data is f32 PCM samples (we configured F32 format via SCK).
    // Interpret the raw bytes as f32 samples.
    let n_samples = data_bytes.len() / std::mem::size_of::<f32>();
    if n_samples == 0 {
        return None;
    }

    let mut samples = Vec::with_capacity(n_samples);

    // Safety: We're reading f32 values from a byte slice that was provided
    // by ScreenCaptureKit. The data is guaranteed to be valid audio data
    // at the format we configured (F32, 48kHz, 2ch interleaved).
    for i in 0..n_samples {
        let offset = i * std::mem::size_of::<f32>();
        if offset + std::mem::size_of::<f32>() <= data_bytes.len() {
            let bytes = [
                data_bytes[offset],
                data_bytes[offset + 1],
                data_bytes[offset + 2],
                data_bytes[offset + 3],
            ];
            samples.push(f32::from_le_bytes(bytes));
        }
    }

    Some(samples)
}
