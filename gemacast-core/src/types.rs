//! Re-exports from [`crate::domain::types`] for backward compatibility.
//!
//! The canonical location is now `src/domain/types.rs`.

pub use crate::domain::types::*;

// Keep the ControlMessage re-export here (not in domain — control layer).
pub use crate::control::messages::ControlMessage;
