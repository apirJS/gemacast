//! Domain layer — pure domain logic with **zero** dependencies on I/O.
//!
//! The innermost hexagonal layer. Everything here is pure logic:
//! value objects, domain errors, audio constants, and algorithmic services.
//!
//! # Contents
//!
//! - **Value objects**: `DeviceId`, `AudioSource`, `JitterConfig`, `RawPacket`, etc.
//! - **Domain errors**: `GemaCastError`, `AudioError`, `NetworkError`, etc.
//! - **Domain services**: `encode_frame()`, `CaptureResampler`, `JitterBufferManager`
//! - **Audio constants**: `OPUS_SAMPLE_RATE`, codec factories

/// Core domain types (value objects, enums, newtypes).
/// Canonical location — moved from `src/types.rs`.
pub mod types;

/// Domain error types.
/// Canonical location — moved from `src/error.rs`.
pub mod error;

/// Audio constants, codec factories, and resampler.
/// Re-exports from `src/audio/`.
pub use crate::audio;

/// Jitter buffer management (already pure algorithmic domain code).
/// Re-exports from `src/jitter/`.
pub use crate::jitter;

/// Audio frame encoding (pure function, no I/O).
/// Re-exports from `src/stream/sender/encode`.
pub use crate::stream::sender::encode as encoding;
