//! Re-exports from [`crate::adapters::transport`] for backward compatibility.
//!
//! The transport structs have moved to the adapters layer (`src/adapters/transport.rs`).
//! This module re-exports them so existing import paths continue to work.

pub use crate::adapters::transport::*;

// Also re-export the port trait from its canonical location.
pub use crate::ports::transport::AudioPacketTransport;
