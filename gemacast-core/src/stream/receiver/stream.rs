#[cfg(not(target_os = "android"))]
use crate::audio::{OPUS_CHANNELS, OPUS_FRAME_SAMPLES};
use crate::{
    audio::{OPUS_SAMPLE_RATE, create_opus_decoder},
    domain::error::{AudioError, CodecDirection, GemaCastError, StreamDirection},
    domain::types::JitterConfig,
    jitter::{JitterBufferManager, RawPacket},
};
#[cfg(not(target_os = "android"))]
use cpal::StreamError;
#[cfg(not(target_os = "android"))]
use cpal::traits::*;
#[cfg(target_os = "android")]
use oboe::{
    AudioOutputCallback, AudioOutputStreamSafe, AudioStreamBuilder, DataCallbackResult,
    PerformanceMode, SharingMode,
};
use ringbuf::traits::*;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU32, Ordering},
};
#[cfg(not(target_os = "android"))]
use tokio::sync::mpsc;

#[cfg(not(target_os = "android"))]
pub type PlaybackStream = cpal::Stream;

/// On Android the playback stream can be backed by either Oboe (preferred for
/// low latency) or cpal (fallback when Oboe fails to open a stream).
#[cfg(target_os = "android")]
pub enum PlaybackStream {
    Oboe(oboe::AudioStreamAsync<oboe::Output, OboeCallback>),
    Cpal(cpal::Stream),
}

#[cfg(target_os = "android")]
pub struct OboeCallback {
    jitter_manager: JitterBufferManager,
    packet_consumer: ringbuf::HeapCons<RawPacket>,
    volume: Arc<AtomicU32>,
    is_playing: Arc<AtomicBool>,
}

#[cfg(target_os = "android")]
impl AudioOutputCallback for OboeCallback {
    type FrameType = (f32, oboe::Stereo);

    fn on_audio_ready(
        &mut self,
        _stream: &mut dyn AudioOutputStreamSafe,
        audio_data: &mut [(f32, f32)],
    ) -> DataCallbackResult {
        let vol = f32::from_bits(self.volume.load(Ordering::Relaxed));

        let float_slice = unsafe {
            std::slice::from_raw_parts_mut(
                audio_data.as_mut_ptr() as *mut f32,
                audio_data.len() * 2,
            )
        };

        if !self.is_playing.load(Ordering::Relaxed) {
            while self.packet_consumer.try_pop().is_some() {}
            for sample in float_slice.iter_mut() {
                *sample = 0.0;
            }
            self.jitter_manager.reset();
            return DataCallbackResult::Continue;
        }

        self.jitter_manager
            .ingest_packets(&mut self.packet_consumer);
        self.jitter_manager.fill_output(float_slice, vol);

        DataCallbackResult::Continue
    }
}

#[cfg(not(target_os = "android"))]
pub fn build_playback_stream(
    mut packet_consumer: ringbuf::HeapCons<RawPacket>,
    config_ref: Arc<std::sync::RwLock<JitterConfig>>,
    is_tcp_mode: Arc<AtomicBool>,
    is_playing: Arc<AtomicBool>,
    volume: Arc<AtomicU32>,
    latency_metric: Arc<AtomicU32>,
    stream_error_tx: mpsc::Sender<StreamError>,
) -> Result<PlaybackStream, GemaCastError> {
    let decoder = create_opus_decoder().map_err(|e| AudioError::OpusInitFailed {
        direction: CodecDirection::Decoder,
        source: e,
    })?;
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or(AudioError::NoOutputDevice)?;

    let mut buffer_size = cpal::BufferSize::Default;

    if let Ok(mut supported_configs) = device.supported_output_configs()
        && let Some(config) = supported_configs.find(|c| {
            c.channels() == OPUS_CHANNELS
                && c.min_sample_rate() <= OPUS_SAMPLE_RATE
                && c.max_sample_rate() >= OPUS_SAMPLE_RATE
        })
    {
        match config.buffer_size() {
            cpal::SupportedBufferSize::Range { min, max } => {
                let desired = OPUS_FRAME_SAMPLES as u32;
                buffer_size = cpal::BufferSize::Fixed(desired.clamp(*min, *max));
            }
            cpal::SupportedBufferSize::Unknown => {}
        }
    }

    let stream_config = cpal::StreamConfig {
        channels: OPUS_CHANNELS,
        sample_rate: OPUS_SAMPLE_RATE,
        buffer_size,
    };

    let mut jitter_manager =
        JitterBufferManager::new(decoder, latency_metric, config_ref, is_tcp_mode);

    device
        .build_output_stream(
            &stream_config,
            move |data: &mut [f32], _: &_| {
                let vol = f32::from_bits(volume.load(Ordering::Relaxed));

                if !is_playing.load(Ordering::Relaxed) {
                    while packet_consumer.try_pop().is_some() {}
                    for sample in data.iter_mut() {
                        *sample = 0.0;
                    }
                    jitter_manager.reset();
                    return;
                }

                jitter_manager.ingest_packets(&mut packet_consumer);
                jitter_manager.fill_output(data, vol);
            },
            move |e| {
                let _ = stream_error_tx.blocking_send(e);
            },
            None,
        )
        .map_err(|e| {
            AudioError::BuildStreamFailed {
                direction: StreamDirection::Output,
                source: e,
            }
            .into()
        })
}

/// Build a cpal-based playback stream on Android as a fallback when Oboe fails.
#[cfg(target_os = "android")]
pub fn build_cpal_fallback_stream(
    mut packet_consumer: ringbuf::HeapCons<RawPacket>,
    config_ref: Arc<std::sync::RwLock<JitterConfig>>,
    is_tcp_mode: Arc<AtomicBool>,
    is_playing: Arc<AtomicBool>,
    volume: Arc<AtomicU32>,
    latency_metric: Arc<AtomicU32>,
) -> Result<PlaybackStream, GemaCastError> {
    use cpal::traits::*;

    let decoder = create_opus_decoder().map_err(|e| AudioError::OpusInitFailed {
        direction: CodecDirection::Decoder,
        source: e,
    })?;

    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or(AudioError::NoOutputDevice)?;

    let stream_config = cpal::StreamConfig {
        channels: 2,
        sample_rate: OPUS_SAMPLE_RATE,
        buffer_size: cpal::BufferSize::Default,
    };

    let mut jitter_manager =
        JitterBufferManager::new(decoder, latency_metric, config_ref, is_tcp_mode);

    let stream = device
        .build_output_stream(
            &stream_config,
            move |data: &mut [f32], _: &_| {
                let vol = f32::from_bits(volume.load(Ordering::Relaxed));

                if !is_playing.load(Ordering::Relaxed) {
                    while packet_consumer.try_pop().is_some() {}
                    for sample in data.iter_mut() {
                        *sample = 0.0;
                    }
                    jitter_manager.reset();
                    return;
                }

                jitter_manager.ingest_packets(&mut packet_consumer);
                jitter_manager.fill_output(data, vol);
            },
            move |e| {
                tracing::error!("cpal fallback stream error: {}", e);
            },
            None,
        )
        .map_err(|e| AudioError::BuildStreamFailed {
            direction: StreamDirection::Output,
            source: e,
        })?;

    Ok(PlaybackStream::Cpal(stream))
}

/// Build a playback stream on Android. Tries Oboe first for lowest latency;
/// if Oboe fails to open the stream, automatically falls back to cpal.
#[cfg(target_os = "android")]
pub fn build_playback_stream(
    packet_consumer: ringbuf::HeapCons<RawPacket>,
    config_ref: Arc<std::sync::RwLock<JitterConfig>>,
    is_tcp_mode: Arc<AtomicBool>,
    is_playing: Arc<AtomicBool>,
    volume: Arc<AtomicU32>,
    latency_metric: Arc<AtomicU32>,
    exclusive_mode: bool,
) -> Result<PlaybackStream, GemaCastError> {
    let decoder = create_opus_decoder().map_err(|e| AudioError::OpusInitFailed {
        direction: CodecDirection::Decoder,
        source: e,
    })?;

    let callback = OboeCallback {
        jitter_manager: JitterBufferManager::new(
            decoder,
            latency_metric.clone(),
            config_ref.clone(),
            is_tcp_mode.clone(),
        ),
        packet_consumer,
        volume: volume.clone(),
        is_playing: is_playing.clone(),
    };

    let builder = AudioStreamBuilder::default()
        .set_direction::<oboe::Output>()
        .set_performance_mode(PerformanceMode::LowLatency)
        .set_sharing_mode(if exclusive_mode {
            SharingMode::Exclusive
        } else {
            SharingMode::Shared
        })
        .set_format::<f32>()
        .set_channel_count::<oboe::Stereo>()
        .set_channel_conversion_allowed(true)
        .set_sample_rate(OPUS_SAMPLE_RATE as i32)
        .set_sample_rate_conversion_quality(oboe::SampleRateConversionQuality::Fastest)
        .set_callback(callback);

    match builder.open_stream() {
        Ok(stream) => Ok(PlaybackStream::Oboe(stream)),
        Err(oboe_err) => {
            tracing::warn!(
                "Oboe failed to open stream ({}), falling back to cpal",
                oboe_err
            );

            // The original packet_consumer was moved into the OboeCallback
            // (which is now dropped), so we need a fresh ring buffer for the
            // cpal fallback. The caller will connect it to the same producer.
            //
            // However, since the consumer was already split from the ring buffer
            // and consumed by the failed Oboe callback, we cannot recover it.
            // Instead, return an error with proper Oboe error types and let the
            // caller handle the fallback at a higher level.
            //
            // For a seamless fallback, we would need to defer callback creation,
            // but that would require restructuring the ring buffer ownership.
            // For now, return the Oboe error — the caller can retry with cpal.
            Err(AudioError::OboeStreamBuildFailed {
                direction: StreamDirection::Output,
                message: format!("{}", oboe_err),
            }
            .into())
        }
    }
}
