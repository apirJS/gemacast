use crate::domain::error::{AudioError, GemaCastError, StreamDirection};
use crate::ports::capture::CaptureBackend;

/// Describe a requested cpal buffer size for the format log.
///
/// The millisecond figure is the reason this is a function rather than a format
/// argument: cpal's [`FrameCount`](cpal::FrameCount) is a **per-channel** frame count,
/// so it goes through [`frames_to_ms`](crate::audio::frames_to_ms) with the bare rate.
/// Printing the period is what makes the request readable in a field log instead of
/// inferable from it — 960 per-channel frames is 20 ms, not the 10 ms the
/// `OPUS_FRAME_SAMPLES` name suggests.
fn describe_buffer_size(size: &cpal::BufferSize, rate: u32) -> String {
    match size {
        cpal::BufferSize::Default => "host default".to_owned(),
        cpal::BufferSize::Fixed(frames) => match crate::audio::frames_to_ms(*frames, rate) {
            Some(ms) => format!("{frames} frames ({ms:.1} ms)"),
            // `rate` is `OPUS_SAMPLE_RATE` at the only call site, so this is a guard
            // rather than a case: a zero would otherwise print `inf ms` and read as a
            // broken device.
            None => format!("{frames} frames, rate unknown"),
        },
    }
}

/// Whether the host has to convert to satisfy the config we asked for.
///
/// `create_cpal_loopback` does not negotiate — it *forces* 48 kHz stereo `f32` and every
/// desktop host silently obliges: the WASAPI host sets
/// `AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | SRC_DEFAULT_QUALITY`, ALSA uses
/// `set_rate(..., ValueOr::Nearest)`, and CoreAudio mutates the device's nominal rate.
/// So the requested figures alone say nothing about what the hardware ran at, and a
/// capture that sounds resampled cannot be distinguished from one that was not.
/// Comparing against the device's own default config is what settles it.
///
/// `f32` is checked against a literal because the sample type is fixed by the callback
/// signature in `build_input_stream`, not by anything in the config — there is no field
/// to read it back from.
fn needs_host_conversion(
    requested: &cpal::StreamConfig,
    device: &cpal::SupportedStreamConfig,
) -> bool {
    device.sample_rate() != requested.sample_rate
        || device.channels() != requested.channels
        || device.sample_format() != cpal::SampleFormat::F32
}

/// Record the capture format unconditionally, at info level, before the stream is built.
///
/// Unconditional is the whole point and it must stay that way. This is the only record
/// of what a capture ran at, and without it a field log cannot establish the rate, the
/// channel count or the callback period — the same gap that once forced the wire format
/// to be inferred from an unrelated heartbeat's saturation behaviour.
fn log_capture_format(
    device_name: &str,
    requested: &cpal::StreamConfig,
    device: Option<&cpal::SupportedStreamConfig>,
) {
    let buffer = describe_buffer_size(&requested.buffer_size, requested.sample_rate);

    match device {
        Some(config) => tracing::info!(
            capture_kind = "cpal-loopback",
            host_converting = needs_host_conversion(requested, config),
            "[cpal] Loopback capture on {:?}: requested {} Hz / {} ch / f32, buffer {}; \
             device default {} Hz / {} ch / {}",
            device_name,
            requested.sample_rate,
            requested.channels,
            buffer,
            config.sample_rate(),
            config.channels(),
            config.sample_format(),
        ),
        // The device would not report a default config. Log what was asked for anyway —
        // a partial line still pins the rate and the period, and the absent half is
        // itself worth seeing.
        None => tracing::info!(
            capture_kind = "cpal-loopback",
            "[cpal] Loopback capture on {:?}: requested {} Hz / {} ch / f32, buffer {}; \
             device default unavailable",
            device_name,
            requested.sample_rate,
            requested.channels,
            buffer,
        ),
    }
}

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

pub fn create_cpal_loopback()
-> Result<crate::ports::capture::CaptureHandle<super::PlatformCaptureBackend>, GemaCastError> {
    use crate::audio::{OPUS_CHANNELS, OPUS_FRAME_SAMPLES, OPUS_SAMPLE_RATE};
    use crate::ports::capture::{CaptureCounters, CaptureHandle};
    use cpal::traits::{DeviceTrait, HostTrait};
    use ringbuf::{HeapRb, traits::*};
    use std::sync::Arc;
    use tokio::sync::{Notify, mpsc};

    let rb = HeapRb::<f32>::new(OPUS_FRAME_SAMPLES * 64);
    let (mut rb_producer, rb_consumer) = rb.split();
    let (stream_error_tx, stream_error_rx) = mpsc::channel::<cpal::StreamError>(1);

    let notify = Arc::new(Notify::new());
    let notify_clone = notify.clone();
    let counters = Arc::new(CaptureCounters::default());
    let counters_cb = counters.clone();

    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or(AudioError::NoOutputDevice)?;

    // Read purely so the log below can say whether the host is converting. Neither call
    // is fatal: a device that will not report its description or default config can
    // still build a stream, and losing a diagnostic is not worth losing capture over.
    //
    // `description()` rather than `name()`, which cpal 0.17 deprecated — it carries the
    // manufacturer, driver and interface type too, but only the name is logged. The rest
    // is worth adding the day a field report turns out to hinge on which driver was in
    // play; it is noise until then.
    let device_name = device
        .description()
        .map(|d| d.name().to_owned())
        .unwrap_or_else(|_| "<unnamed>".to_owned());
    let device_config = device.default_output_config().ok();

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

    log_capture_format(&device_name, &stream_config, device_config.as_ref());

    let audio_stream = device
        .build_input_stream(
            &stream_config,
            move |data: &[f32], _: &_| {
                if rb_producer.vacant_len() >= data.len() {
                    let _ = rb_producer.push_slice(data);
                } else {
                    // Same whole-buffer drop policy as the WASAPI and PipeWire paths.
                    CaptureCounters::add(&counters_cb.dropped_samples, data.len() as u64);
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
        counters,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The buffer-size description.
    ///
    /// These pin the per-channel reading of cpal's `FrameCount`. Dividing by the
    /// interleaved total instead would report every period at half its true length,
    /// and a log that is wrong by exactly 2× is worse than no log — it looks
    /// authoritative.
    mod buffer_size_description {
        use super::*;

        #[test]
        fn should_report_a_fixed_size_in_frames_and_milliseconds() {
            // 480 per-channel frames at 48 kHz is 10 ms. This is the value the request
            // should be using; see `describe_buffer_size`.
            assert_eq!(
                describe_buffer_size(&cpal::BufferSize::Fixed(480), 48_000),
                "480 frames (10.0 ms)"
            );
        }

        #[test]
        fn should_read_a_frame_count_as_per_channel_not_interleaved() {
            // The value actually requested today. 20 ms, not 10 — the assertion exists
            // so that stays visible rather than being quietly halved by a divisor that
            // also divides by the channel count.
            assert_eq!(
                describe_buffer_size(
                    &cpal::BufferSize::Fixed(crate::audio::OPUS_FRAME_SAMPLES as u32),
                    crate::audio::OPUS_SAMPLE_RATE
                ),
                "960 frames (20.0 ms)"
            );
        }

        #[test]
        fn should_name_the_host_default_rather_than_printing_a_period_for_it() {
            assert_eq!(
                describe_buffer_size(&cpal::BufferSize::Default, 48_000),
                "host default"
            );
        }

        #[test]
        fn should_not_print_an_infinite_period_when_the_rate_is_zero() {
            let described = describe_buffer_size(&cpal::BufferSize::Fixed(480), 0);

            assert_eq!(described, "480 frames, rate unknown");
            assert!(
                !described.contains("inf"),
                "a zero rate must not read as a broken device: {described}"
            );
        }
    }

    /// The host-conversion verdict.
    ///
    /// Worth testing because it is the only field in the log line that is a conclusion
    /// rather than a transcription, and because each of the three ways a device can
    /// differ has to count — a check that only compared sample rates would report
    /// `host_converting=false` on a mono or 16-bit device, which is precisely a case
    /// where the host is converting hardest.
    mod host_conversion {
        use super::*;

        fn requested() -> cpal::StreamConfig {
            cpal::StreamConfig {
                channels: crate::audio::OPUS_CHANNELS,
                sample_rate: crate::audio::OPUS_SAMPLE_RATE,
                buffer_size: cpal::BufferSize::Default,
            }
        }

        fn device(
            channels: u16,
            rate: u32,
            format: cpal::SampleFormat,
        ) -> cpal::SupportedStreamConfig {
            cpal::SupportedStreamConfig::new(
                channels,
                rate,
                cpal::SupportedBufferSize::Range { min: 64, max: 4096 },
                format,
            )
        }

        #[test]
        fn should_report_no_conversion_when_the_device_already_matches() {
            assert!(!needs_host_conversion(
                &requested(),
                &device(2, 48_000, cpal::SampleFormat::F32)
            ));
        }

        #[test]
        fn should_detect_a_rate_mismatch() {
            assert!(needs_host_conversion(
                &requested(),
                &device(2, 44_100, cpal::SampleFormat::F32)
            ));
        }

        #[test]
        fn should_detect_a_channel_count_mismatch() {
            assert!(needs_host_conversion(
                &requested(),
                &device(1, 48_000, cpal::SampleFormat::F32)
            ));
        }

        #[test]
        fn should_detect_a_sample_format_mismatch() {
            // The case with no field to read it back from: we always take `&[f32]` in
            // the callback, so a 16-bit device is converted by the host with nothing in
            // the requested config to show it.
            assert!(needs_host_conversion(
                &requested(),
                &device(2, 48_000, cpal::SampleFormat::I16)
            ));
        }
    }
}
