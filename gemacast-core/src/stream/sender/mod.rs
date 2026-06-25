//! Sender-side audio capture, encoding, and streaming.

pub mod capture_pool;
pub mod encode;
pub mod engine;

pub use engine::{AudioStreamCommand, AudioStreamEngine};
