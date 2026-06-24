use crate::error::{AudioError, GemaCastError, StreamDirection};
use crate::ports::capture::CaptureBackend;

pub struct CpalLoopbackCapture {
    stream: cpal::Stream,
}

impl CaptureBackend for CpalLoopbackCapture {
    fn play(&mut self) -> Result<(), GemaCastError> {
        use cpal::traits::StreamTrait;
        self.stream
            .play()
            .map_err(|e| AudioError::PlayStreamFailed {
                direction: StreamDirection::Input,
                source: e,
            })?;
        Ok(())
    }

    fn pause(&mut self) -> Result<(), GemaCastError> {
        use cpal::traits::StreamTrait;
        let _ = self.stream.pause();
        Ok(())
    }
}

#[cfg(not(target_os = "windows"))]
pub fn create_cpal_loopback() -> Result<crate::ports::capture::CaptureHandle<super::PlatformCaptureBackend>, GemaCastError> {
    use crate::audio::{OPUS_CHANNELS, OPUS_FRAME_SAMPLES, OPUS_SAMPLE_RATE};
    use crate::ports::capture::CaptureHandle;
    use cpal::traits::{DeviceTrait, HostTrait};
    use ringbuf::{HeapRb, traits::*};
    use std::sync::Arc;
    use tokio::sync::{Notify, mpsc};

    let rb = HeapRb::<f32>::new(OPUS_FRAME_SAMPLES * 64);
    let (mut rb_producer, rb_consumer) = rb.split();
    let (stream_error_tx, stream_error_rx) = mpsc::channel::<cpal::StreamError>(1);

    let notify = Arc::new(Notify::new());
    let notify_clone = notify.clone();

    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or(AudioError::NoOutputDevice)?;

    let mut buffer_size = cpal::BufferSize::Default;
    let rate = OPUS_SAMPLE_RATE;
    if let Ok(mut supported_configs) = device.supported_output_configs()
        && let Some(config) = supported_configs.find(|c| {
            c.channels() == OPUS_CHANNELS
                && c.min_sample_rate() <= rate
                && c.max_sample_rate() >= rate
        })
        && let cpal::SupportedBufferSize::Range { min, max } = config.buffer_size()
    {
        let desired = OPUS_FRAME_SAMPLES as u32;
        buffer_size = cpal::BufferSize::Fixed(desired.clamp(*min, *max));
    }

    let stream_config = cpal::StreamConfig {
        channels: OPUS_CHANNELS,
        sample_rate: OPUS_SAMPLE_RATE,
        buffer_size,
    };

    let audio_stream = device
        .build_input_stream(
            &stream_config,
            move |data: &[f32], _: &_| {
                if rb_producer.vacant_len() >= data.len() {
                    let _ = rb_producer.push_slice(data);
                }

                notify_clone.notify_one();
            },
            move |e| {
                let _ = stream_error_tx.blocking_send(e);
            },
            None,
        )
        .map_err(|e| AudioError::BuildStreamFailed {
            direction: StreamDirection::Input,
            source: e,
        })?;

    Ok(CaptureHandle {
        backend: super::PlatformCaptureBackend::Cpal(CpalLoopbackCapture {
            stream: audio_stream,
        }),
        consumer: rb_consumer,
        notify,
        stream_error_rx,
    })
}
