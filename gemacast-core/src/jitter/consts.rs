//! Shared timing constants and small numeric helpers used across the jitter
//! pipeline actors (stats, target controller, timescaler, flow, orchestrator).

use crate::audio::{OPUS_FRAME_SIZE, OPUS_SAMPLE_RATE};

/// Milliseconds of audio represented by a single Opus frame / packet slot.
pub(super) const MILLIS_PER_FRAME: u32 = (OPUS_FRAME_SIZE as u32 * 1000) / OPUS_SAMPLE_RATE;

/// OLA window length in sample-frames for WSOLA crossfading.
/// 128 frames = 2.67ms at 48kHz — long enough for perceptual transparency.
pub(super) const OLA_LEN: usize = 128;

/// Search range in sample-frames for cross-correlation alignment.
/// 720 frames = 15ms at 48kHz, covering the full human pitch period range.
pub(super) const SEARCH_RANGE: usize = 720;

/// Convert milliseconds to frames using ceiling division.
/// Prevents truncation to 0 for sub-frame values (e.g. 2ms / 5ms = 1 frame, not 0).
pub(super) fn ms_to_frames_ceil(ms: u32) -> u32 {
    ms.div_ceil(MILLIS_PER_FRAME)
}

/// Linear interpolation between `a` and `b` by fraction `t`.
pub(super) fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}
