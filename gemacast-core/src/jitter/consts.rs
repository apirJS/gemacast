//! Shared timing constants and small numeric helpers used across the jitter
//! pipeline actors (stats, target controller, timescaler, flow, orchestrator).

use crate::audio::{OPUS_FRAME_SIZE, OPUS_SAMPLE_RATE};

/// Milliseconds of audio represented by a single Opus frame / packet slot.
pub(super) const MILLIS_PER_FRAME: u32 = (OPUS_FRAME_SIZE as u32 * 1000) / OPUS_SAMPLE_RATE;

/// Linear interpolation between `a` and `b` by fraction `t`.
pub(super) fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}
