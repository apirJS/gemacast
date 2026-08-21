//! Testing infrastructure — mock adapters for all port traits.
//!
//! Provides zero-I/O mock implementations of every port trait, enabling
//! unit tests for orchestration code (`AudioStreamEngine`, `CapturePool`,
//! control server) without OS audio devices, network access, or WASAPI.
//!
//! # Usage
//!
//! ```rust,ignore
//! use gemacast_core::testing::mocks::*;
//!
//! let factory = MockCaptureFactory::new();
//! let notifier = MockErrorNotifier::new();
//! let engine = AudioStreamEngine::new(factory, true, notifier);
//! ```

#[cfg(test)]
pub mod mocks {
    use std::sync::{Arc, Mutex};

    use crate::domain::error::GemaCastError;
    use crate::domain::types::{DeviceId, ProcessInfo};
    use crate::ports::capture::{CaptureBackend, CaptureCounters, CaptureFactory, CaptureHandle};
    use crate::ports::error_notifier::ErrorNotifier;
    use crate::ports::process_lister::ProcessLister;
    use crate::ports::transport::AudioPacketTransport;
    use ringbuf::traits::Split;

    // -----------------------------------------------------------------------
    // Call recording
    // -----------------------------------------------------------------------

    /// Records all calls for assertion in tests.
    #[derive(Debug, Clone)]
    pub enum Call {
        Play,
        Pause,
        CreateDesktopCapture,
        CreateProcessCapture { pid: u32 },
        NotifyError { device_id: String, message: String },
        ListProcesses,
        ReceiveAudioPacket,
    }

    /// Shared call log for assertion.
    pub type CallLog = Arc<Mutex<Vec<Call>>>;

    pub fn new_call_log() -> CallLog {
        Arc::new(Mutex::new(Vec::new()))
    }

    // -----------------------------------------------------------------------
    // MockCaptureBackend
    // -----------------------------------------------------------------------

    pub struct MockCaptureBackend {
        pub calls: CallLog,
    }

    impl CaptureBackend for MockCaptureBackend {
        fn play(&mut self) -> Result<(), GemaCastError> {
            self.calls.lock().unwrap().push(Call::Play);
            Ok(())
        }

        fn pause(&mut self) -> Result<(), GemaCastError> {
            self.calls.lock().unwrap().push(Call::Pause);
            Ok(())
        }
    }

    // -----------------------------------------------------------------------
    // MockCaptureFactory
    // -----------------------------------------------------------------------

    #[derive(Clone)]
    pub struct MockCaptureFactory {
        pub calls: CallLog,
    }

    impl Default for MockCaptureFactory {
        fn default() -> Self {
            Self::new()
        }
    }

    impl MockCaptureFactory {
        pub fn new() -> Self {
            Self {
                calls: new_call_log(),
            }
        }
    }

    impl CaptureFactory for MockCaptureFactory {
        type Backend = MockCaptureBackend;

        fn create_desktop_capture(&self) -> Result<CaptureHandle<Self::Backend>, GemaCastError> {
            self.calls.lock().unwrap().push(Call::CreateDesktopCapture);

            let rb = ringbuf::HeapRb::<f32>::new(960 * 4);
            let (_, consumer) = rb.split();
            let notify = Arc::new(tokio::sync::Notify::new());
            let (_, stream_error_rx) = tokio::sync::mpsc::channel(1);

            Ok(CaptureHandle {
                backend: MockCaptureBackend {
                    calls: self.calls.clone(),
                },
                consumer,
                notify,
                stream_error_rx,
                counters: Arc::new(CaptureCounters::default()),
            })
        }

        fn create_process_capture(
            &self,
            pid: u32,
        ) -> Result<CaptureHandle<Self::Backend>, GemaCastError> {
            self.calls
                .lock()
                .unwrap()
                .push(Call::CreateProcessCapture { pid });

            let rb = ringbuf::HeapRb::<f32>::new(960 * 4);
            let (_, consumer) = rb.split();
            let notify = Arc::new(tokio::sync::Notify::new());
            let (_, stream_error_rx) = tokio::sync::mpsc::channel(1);

            Ok(CaptureHandle {
                backend: MockCaptureBackend {
                    calls: self.calls.clone(),
                },
                consumer,
                notify,
                stream_error_rx,
                counters: Arc::new(CaptureCounters::default()),
            })
        }
    }

    // -----------------------------------------------------------------------
    // MockErrorNotifier
    // -----------------------------------------------------------------------

    #[derive(Clone)]
    pub struct MockErrorNotifier {
        pub calls: CallLog,
    }

    impl Default for MockErrorNotifier {
        fn default() -> Self {
            Self::new()
        }
    }

    impl MockErrorNotifier {
        pub fn new() -> Self {
            Self {
                calls: new_call_log(),
            }
        }
    }

    impl ErrorNotifier for MockErrorNotifier {
        fn notify_error(&self, device_id: &DeviceId, message: String) {
            self.calls.lock().unwrap().push(Call::NotifyError {
                device_id: device_id.0.clone(),
                message,
            });
        }
    }

    // -----------------------------------------------------------------------
    // MockProcessLister
    // -----------------------------------------------------------------------

    #[derive(Clone)]
    pub struct MockProcessLister {
        pub calls: CallLog,
        pub processes: Vec<ProcessInfo>,
    }

    impl Default for MockProcessLister {
        fn default() -> Self {
            Self::new()
        }
    }

    impl MockProcessLister {
        pub fn new() -> Self {
            Self {
                calls: new_call_log(),
                processes: Vec::new(),
            }
        }

        pub fn with_processes(processes: Vec<ProcessInfo>) -> Self {
            Self {
                calls: new_call_log(),
                processes,
            }
        }
    }

    impl ProcessLister for MockProcessLister {
        fn list_processes(&self) -> Vec<ProcessInfo> {
            self.calls.lock().unwrap().push(Call::ListProcesses);
            self.processes.clone()
        }
    }

    // -----------------------------------------------------------------------
    // MockTransport
    // -----------------------------------------------------------------------

    pub struct MockTransport {
        pub calls: CallLog,
        /// Packets to return, one per call. When empty, returns EOF.
        pub packets: Vec<Vec<u8>>,
        index: usize,
    }

    impl MockTransport {
        pub fn new(packets: Vec<Vec<u8>>) -> Self {
            Self {
                calls: new_call_log(),
                packets,
                index: 0,
            }
        }
    }

    impl AudioPacketTransport for MockTransport {
        fn receive_audio_packet(
            &mut self,
            buffer: &mut [u8],
        ) -> std::io::Result<(usize, std::net::SocketAddr)> {
            self.calls.lock().unwrap().push(Call::ReceiveAudioPacket);
            if self.index < self.packets.len() {
                let data = &self.packets[self.index];
                self.index += 1;
                let len = data.len();
                buffer[..len].copy_from_slice(data);
                Ok((len, "127.0.0.1:1234".parse().unwrap()))
            } else {
                Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "No more mock packets",
                ))
            }
        }
    }

    // -----------------------------------------------------------------------
    // FakeAudioProducer
    // -----------------------------------------------------------------------

    /// Injects synthetic audio data into a `CaptureHandle`'s ring buffer.
    ///
    /// This enables full pipeline testing (capture → resample → encode → transport)
    /// without requiring OS audio hardware or platform-specific backends.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let (handle, mut producer) = create_fake_capture();
    /// producer.push_tone(440.0, 100); // 100ms of 440Hz sine wave
    /// // ... assert encoded packets arrive via the pipeline
    /// ```
    pub struct FakeAudioProducer {
        pub producer: ringbuf::HeapProd<f32>,
        pub notify: Arc<tokio::sync::Notify>,
    }

    impl FakeAudioProducer {
        /// Push a sine wave tone into the ring buffer.
        ///
        /// Generates `duration_ms` milliseconds of a sine wave at `frequency_hz`,
        /// using 48kHz stereo format (matching the Opus pipeline).
        ///
        /// Returns the number of samples that fitted, as [`Self::push_samples`] does.
        pub fn push_tone(&mut self, frequency_hz: f32, duration_ms: u32) -> usize {
            let sample_rate = crate::audio::OPUS_SAMPLE_RATE as f32;
            let channels = crate::audio::OPUS_CHANNELS as usize;
            let num_frames = (sample_rate * duration_ms as f32 / 1000.0) as usize;

            let mut samples = Vec::with_capacity(num_frames * channels);
            for i in 0..num_frames {
                let t = i as f32 / sample_rate;
                let sample = (2.0 * std::f32::consts::PI * frequency_hz * t).sin() * 0.5;
                for _ in 0..channels {
                    samples.push(sample);
                }
            }
            self.push_samples(&samples)
        }

        /// Push silence (zero samples) into the ring buffer.
        ///
        /// Returns the number of samples that fitted, as [`Self::push_samples`] does.
        pub fn push_silence(&mut self, duration_ms: u32) -> usize {
            let sample_rate = crate::audio::OPUS_SAMPLE_RATE as f32;
            let channels = crate::audio::OPUS_CHANNELS as usize;
            let num_samples = (sample_rate * duration_ms as f32 / 1000.0) as usize * channels;

            self.push_samples(&vec![0.0f32; num_samples])
        }

        /// Push arbitrary sample data into the ring buffer, returning how many samples
        /// fitted.
        ///
        /// Overflow is dropped silently, as it always was, but only in whole stereo
        /// pairs. This is the single choke point for all three helpers, and the reason
        /// it exists is [producer obligation
        /// 1](crate::ports::capture#producer-obligations): the previous shape pushed
        /// sample by sample with `try_push`, so a ring that filled mid-pair accepted the
        /// left channel and dropped the right, shifting every later sample by one slot
        /// and swapping the channels for the rest of the run. A mock is the worst
        /// possible place for that — it would present as a real backend defect with
        /// nothing in the backend to find.
        pub fn push_samples(&mut self, samples: &[f32]) -> usize {
            use ringbuf::traits::{Observer, Producer};

            debug_assert_eq!(
                samples.len() % 2,
                0,
                "a mock push must carry whole stereo pairs; {} samples is half a pair",
                samples.len()
            );

            // Clamped to whole pairs *before* the push rather than checked after,
            // because `push_slice` is bounded by `vacant_len()` and that can be odd
            // whenever a test pops an odd count from the consumer side.
            let room = self.producer.vacant_len().min(samples.len()) & !1;
            let pushed = self.producer.push_slice(&samples[..room]);

            self.notify.notify_one();
            pushed
        }
    }

    /// Create a `CaptureHandle` with a `FakeAudioProducer` for testing.
    ///
    /// Returns the handle (consumer side) and the producer for injecting
    /// synthetic audio. The handle uses a `MockCaptureBackend` so
    /// `play()`/`pause()` are recorded in the call log.
    pub fn create_fake_capture() -> (CaptureHandle<MockCaptureBackend>, FakeAudioProducer) {
        let calls = new_call_log();
        let rb = ringbuf::HeapRb::<f32>::new(crate::audio::OPUS_FRAME_SAMPLES * 64);
        let (producer, consumer) = rb.split();
        let notify = Arc::new(tokio::sync::Notify::new());
        let (_, stream_error_rx) = tokio::sync::mpsc::channel(1);

        let handle = CaptureHandle {
            backend: MockCaptureBackend {
                calls: calls.clone(),
            },
            consumer,
            notify: notify.clone(),
            stream_error_rx,
            counters: Arc::new(CaptureCounters::default()),
        };

        let fake_producer = FakeAudioProducer { producer, notify };

        (handle, fake_producer)
    }

    /// Tests for the harness itself.
    ///
    /// Worth having because a parity bug in a mock producer presents as a backend
    /// defect: the frames stay 960 samples long and every downstream assertion still
    /// passes, so the search would start in the adapter, where there would be nothing
    /// to find.
    #[cfg(test)]
    mod tests {
        use super::*;
        use ringbuf::traits::{Consumer, Observer};

        const RING_CAPACITY: usize = crate::audio::OPUS_FRAME_SAMPLES * 64;

        #[test]
        fn push_samples_should_report_what_fitted() {
            let (_handle, mut producer) = create_fake_capture();

            assert_eq!(producer.push_samples(&[0.1, 0.2, 0.3, 0.4]), 4);
        }

        #[test]
        fn push_samples_should_drop_whole_pairs_when_the_ring_is_nearly_full() {
            let (mut handle, mut producer) = create_fake_capture();

            // Fill it, then take an odd number back out. Popping is the only way to
            // leave an odd vacancy, because every push is even.
            assert_eq!(
                producer.push_samples(&vec![0.0f32; RING_CAPACITY]),
                RING_CAPACITY
            );
            let mut scratch = [0.0f32; 3];
            assert_eq!(handle.consumer.pop_slice(&mut scratch), 3);
            assert_eq!(producer.producer.vacant_len(), 3);

            // Three slots free, two samples' worth of room. The per-sample `try_push`
            // loop this replaces would have taken all three and swapped the channels
            // for the rest of the run.
            let occupied_before = handle.consumer.occupied_len();
            let pushed = producer.push_samples(&[0.1, 0.2, 0.3, 0.4]);

            assert_eq!(pushed, 2, "a half pair must not be pushed to fill a gap");
            // The delta, not the absolute parity: this test made occupancy odd itself
            // by popping 3, and an odd pop is a thing only a test can do. What the
            // producer owes is that it never *changes* the alignment.
            assert_eq!(
                (handle.consumer.occupied_len() - occupied_before) % 2,
                0,
                "a push must not shift the ring's stereo alignment"
            );
        }

        #[test]
        fn push_tone_should_push_whole_stereo_pairs() {
            let (_handle, mut producer) = create_fake_capture();

            // 10 ms at 48 kHz stereo is exactly one frame.
            let pushed = producer.push_tone(440.0, 10);

            assert_eq!(pushed, crate::audio::OPUS_FRAME_SAMPLES);
        }

        #[test]
        fn push_tone_should_write_the_same_sample_to_both_channels() {
            let (mut handle, mut producer) = create_fake_capture();

            producer.push_tone(1_000.0, 10);

            let mut frame = vec![0.0f32; crate::audio::OPUS_FRAME_SAMPLES];
            assert_eq!(
                handle.consumer.pop_slice(&mut frame),
                crate::audio::OPUS_FRAME_SAMPLES
            );

            // Not a formality: this is the assertion that fails if the ring ever
            // shifts by one slot, and it is the only place in the harness that can
            // detect it. The tone is mono-sourced, so L and R are bit-identical.
            for pair in frame.chunks_exact(2) {
                assert_eq!(pair[0], pair[1], "left and right diverged");
            }
            // A non-trivial signal, so the check above is not passing on all zeros.
            assert!(frame.iter().any(|s| s.abs() > 0.1));
        }

        #[test]
        fn push_silence_should_push_whole_stereo_pairs() {
            let (_handle, mut producer) = create_fake_capture();

            assert_eq!(
                producer.push_silence(10),
                crate::audio::OPUS_FRAME_SAMPLES,
                "10 ms of 48 kHz stereo is one frame"
            );
        }
    }
}
