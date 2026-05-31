use ringbuf::HeapCons;
use std::sync::Arc;
use tokio::sync::{Notify, mpsc};

use crate::error::GemaCastError;

pub mod cpal_loopback;
#[cfg(target_os = "windows")]
pub mod wasapi_common;
#[cfg(target_os = "windows")]
pub mod wasapi_desktop;
pub mod wasapi_loopback;

pub trait CaptureBackend: Send {
    fn play(&mut self) -> Result<(), GemaCastError>;
    fn pause(&mut self) -> Result<(), GemaCastError>;
}
pub struct CaptureHandle {
    pub backend: Box<dyn CaptureBackend>,
    pub consumer: HeapCons<f32>,
    pub notify: Arc<Notify>,
    pub stream_error_rx: mpsc::Receiver<cpal::StreamError>,
}

pub trait CaptureFactory: Send + Sync {
    fn create_desktop_capture(&self) -> Result<CaptureHandle, GemaCastError>;
    fn create_process_capture(&self, pid: u32) -> Result<CaptureHandle, GemaCastError>;
}

pub struct DefaultCaptureFactory;

impl CaptureFactory for DefaultCaptureFactory {
    fn create_desktop_capture(&self) -> Result<CaptureHandle, GemaCastError> {
        #[cfg(windows)]
        {
            wasapi_desktop::create_wasapi_desktop_loopback()
        }
        #[cfg(not(windows))]
        {
            cpal_loopback::create_cpal_loopback()
        }
    }

    #[allow(unused_variables)]
    fn create_process_capture(&self, pid: u32) -> Result<CaptureHandle, GemaCastError> {
        #[cfg(windows)]
        {
            wasapi_loopback::create_wasapi_process_loopback(pid)
        }
        #[cfg(not(windows))]
        {
            Err(crate::error::AudioError::ProcessCaptureUnavailable.into())
        }
    }
}

