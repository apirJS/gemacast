//! Sender-side audio capture, encoding, and streaming.

pub mod capture_pool;
pub mod engine;
pub mod encode;
pub mod capture;

pub use engine::{AudioStreamCommand, AudioStreamEngine};
