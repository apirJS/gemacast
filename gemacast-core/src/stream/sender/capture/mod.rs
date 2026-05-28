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
