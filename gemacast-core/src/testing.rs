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
    use crate::ports::capture::{CaptureBackend, CaptureFactory, CaptureHandle};
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
}
