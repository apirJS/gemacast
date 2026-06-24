//! Re-exports from [`crate::adapters::capture`] for backward compatibility.
//!
//! The capture backends and factory have moved to the adapters layer
//! (`src/adapters/capture/`). This module re-exports them so existing
//! import paths continue to work.

pub use crate::adapters::capture::*;
