//! Time-scaling actor: WSOLA (Waveform Similarity Overlap-Add) primitives that
//! stretch or shrink decoded PCM without changing pitch. Used to slow playback
//! before an imminent starvation (expand), drain an overfull buffer
//! (accelerate), and splice across a flush boundary (overlap-add).
//!
//! Owns the pre-computed Hann window and the `wsola_buf` scratch used to hold
//! the pre-flush frame while the post-flush frame is decoded. Reads decoded PCM
//! and writes stretched output through borrowed slices — it holds no decoder,
//! jitter buffer, or playback buffer of its own.

use super::consts::{OLA_LEN, SEARCH_RANGE};
use crate::audio::{OPUS_CHANNELS, OPUS_FRAME_SAMPLES};
use std::collections::VecDeque;

/// WSOLA time-scaler: Hann window + scratch buffer for allocation-free splicing.
pub(super) struct TimeScaler {
    /// Pre-computed Hann window for OLA crossfading (OLA_LEN entries).
    hann_window: Vec<f32>,
    /// Pre-allocated buffer for WSOLA: holds the first frame's PCM while decoding the second.
    wsola_buf: Vec<f32>,
    /// Count of successful accelerate/expand splices. Test-only observability: lets
    /// artifact-regression tests assert that loud audio is NOT time-stretched.
    /// `Cell` because the stretch methods take `&self`.
    #[cfg(test)]
    op_count: std::cell::Cell<usize>,
}

impl TimeScaler {
    pub fn new() -> Self {
        Self {
            hann_window: Self::make_hann_window(),
            wsola_buf: vec![0.0f32; OPUS_FRAME_SAMPLES],
            #[cfg(test)]
            op_count: std::cell::Cell::new(0),
        }
    }

    /// Number of successful time-stretch splices performed so far (test-only).
    #[cfg(test)]
    pub fn op_count(&self) -> usize {
        self.op_count.get()
    }

    /// Record a successful splice (test-only, no-op in release).
    #[inline]
    fn note_op(&self) {
        #[cfg(test)]
        self.op_count.set(self.op_count.get() + 1);
    }

    fn make_hann_window() -> Vec<f32> {
        (0..OLA_LEN)
            .map(|i| 0.5 * (1.0 - (2.0 * std::f32::consts::PI * i as f32 / OLA_LEN as f32).cos()))
            .collect()
    }

    /// Snapshot the current decoded PCM into `wsola_buf` (the pre-flush frame),
    /// so it survives while the post-flush frame is decoded in place.
    pub fn snapshot(&mut self, pcm: &[f32]) {
        let len = pcm.len();
        if len > 0 {
            self.wsola_buf[..len].copy_from_slice(&pcm[..len]);
        }
    }

    /// The pre-flush snapshot, for the fallback path when overlap-add bails.
    pub fn snapshotted(&self, len: usize) -> &[f32] {
        &self.wsola_buf[..len]
    }

    /// Hann Overlap-Add WSOLA splice (allocation-free).
    ///
    /// Reads pcm1 from `self.wsola_buf[..pcm1_len]` and pcm2 from `pcm2`.
    /// Finds the best phase-aligned splice point via **mono-downmixed** normalized
    /// cross-correlation (halves FMA count vs full-stereo, enables NEON auto-vectorization),
    /// then applies a Hann-windowed crossfade on full stereo. Writes output to `playback_buf`.
    pub fn overlap_add(
        &self,
        pcm1_len: usize,
        pcm2: &[f32],
        force_crossfade: bool,
        playback_buf: &mut VecDeque<f32>,
    ) -> bool {
        let ch = OPUS_CHANNELS as usize;
        let pcm2_len = pcm2.len();
        let n1 = pcm1_len / ch;
        let n2 = pcm2_len / ch;

        // Guard: if packets are too small for OLA, just pass through pcm1
        if n1 < OLA_LEN + 16 || n2 < OLA_LEN + 16 {
            return false;
        }

        let anchor = n1 - OLA_LEN;
        let search_limit = SEARCH_RANGE.min(n2.saturating_sub(OLA_LEN));
        // Mono-downmix optimization: pre-compute a contiguous mono reference
        // segment from the stereo tail of pcm1. Contiguous f32 layout enables
        // LLVM to auto-vectorize the inner correlation loop with NEON on ARM.
        let mut mono_ref = [0.0f32; OLA_LEN];
        let mut ref_energy = 0.0f32;
        for (i, m) in mono_ref.iter_mut().enumerate() {
            let base = (anchor + i) * ch;
            let mono = if ch == 2 {
                (self.wsola_buf[base] + self.wsola_buf[base + 1]) * 0.5
            } else {
                self.wsola_buf[base]
            };
            *m = mono;
            ref_energy += mono * mono;
        }

        let mut best_d = 0usize;
        let mut best_corr = f32::NEG_INFINITY;
        for d in 0..search_limit {
            let mut cross = 0.0f32;
            let mut cand_energy = 0.0f32;
            // Inner loop is now stride-1 on contiguous mono data — SIMD-friendly.
            for (i, &m) in mono_ref.iter().enumerate() {
                let base = (d + i) * ch;
                let mono_cand = if ch == 2 {
                    (pcm2[base] + pcm2[base + 1]) * 0.5
                } else {
                    pcm2[base]
                };
                cross += m * mono_cand;
                cand_energy += mono_cand * mono_cand;
            }
            let denom = (ref_energy * cand_energy).sqrt();
            let ncc = if denom > 1e-10 { cross / denom } else { 0.0 };
            if ncc > best_corr {
                best_corr = ncc;
                best_d = d;
            }
        }

        // --- NetEQ VAD & Gentle Acceleration (Method 2 & 4) ---
        // If active speech (not forced) and correlation is weak (< 0.9), abort!
        if !force_crossfade && best_corr < 0.9 {
            return false;
        }

        // 1. pcm1[0..anchor] verbatim (bulk extend, no per-sample push)
        playback_buf.extend(&self.wsola_buf[..anchor * ch]);
        // 2. Hann OLA crossfade (full stereo for transparent output)
        for i in 0..OLA_LEN {
            let hann_in = self.hann_window[i];
            let hann_out = 1.0 - hann_in;
            for c in 0..ch {
                let r = self.wsola_buf[(anchor + i) * ch + c];
                let s = pcm2[(best_d + i) * ch + c];
                playback_buf.push_back(r * hann_out + s * hann_in);
            }
        }

        // 3. pcm2[best_d+OLA_LEN..] verbatim (bulk extend)
        let tail_start = (best_d + OLA_LEN) * ch;
        if tail_start < pcm2_len {
            playback_buf.extend(&pcm2[tail_start..pcm2_len]);
        }

        true
    }

    /// NetEQ Preemptive Expand (Method 1).
    /// Stretches the current decode buffer by exactly one pitch period (up to 15ms)
    /// to slow down playback and prevent an imminent starvation gap.
    ///
    /// Returns `Some(n)` where `n` is the number of **interleaved samples inserted**
    /// (so the orchestrator can immediately correct the filtered buffer level, as
    /// NetEQ's `BufferLevelFilter` does), or `None` if no stretch was performed.
    pub fn expand(&self, pcm: &[f32], playback_buf: &mut VecDeque<f32>) -> Option<usize> {
        let ch = OPUS_CHANNELS as usize;
        let n = pcm.len() / ch;
        if n < OLA_LEN + 16 {
            return None;
        }

        let anchor = n - OLA_LEN;
        let search_limit = SEARCH_RANGE.min(anchor.saturating_sub(16));
        if search_limit == 0 {
            return None;
        }

        let mut mono_ref = [0.0f32; OLA_LEN];
        let mut ref_energy = 0.0f32;
        for (i, m) in mono_ref.iter_mut().enumerate() {
            let base = (anchor + i) * ch;
            let mono = if ch == 2 {
                (pcm[base] + pcm[base + 1]) * 0.5
            } else {
                pcm[base]
            };
            *m = mono;
            ref_energy += mono * mono;
        }

        let mut best_d = 0usize;
        let mut best_corr = f32::NEG_INFINITY;
        for d in 0..search_limit {
            let mut cross = 0.0f32;
            let mut cand_energy = 0.0f32;
            for (i, &m) in mono_ref.iter().enumerate() {
                let base = (d + i) * ch;
                let mono_cand = if ch == 2 {
                    (pcm[base] + pcm[base + 1]) * 0.5
                } else {
                    pcm[base]
                };
                cross += m * mono_cand;
                cand_energy += mono_cand * mono_cand;
            }
            let denom = (ref_energy * cand_energy).sqrt();
            let ncc = if denom > 1e-10 { cross / denom } else { 0.0 };
            if ncc > best_corr {
                best_corr = ncc;
                best_d = d;
            }
        }

        // NetEQ requires strong correlation (>0.9) to stretch, otherwise it causes robotic artifacts
        if best_corr < 0.9 {
            return None;
        }

        // 1. pcm[0..anchor] verbatim
        playback_buf.extend(&pcm[..anchor * ch]);
        // 2. Hann OLA crossfade
        for i in 0..OLA_LEN {
            let hann_in = self.hann_window[i];
            let hann_out = 1.0 - hann_in;
            for c in 0..ch {
                let r = pcm[(anchor + i) * ch + c];
                let s = pcm[(best_d + i) * ch + c];
                playback_buf.push_back(r * hann_out + s * hann_in);
            }
        }
        // 3. pcm[best_d+OLA_LEN..end] verbatim
        let tail_start = (best_d + OLA_LEN) * ch;
        if tail_start < pcm.len() {
            playback_buf.extend(&pcm[tail_start..]);
        }

        // Inserted audio = one pitch period (anchor - best_d) of sample-frames.
        // The output is longer than the input by exactly this many sample-frames.
        self.note_op();
        Some((anchor - best_d) * ch)
    }

    /// NetEQ-style single-frame acceleration.
    ///
    /// Finds the pitch period via **autocorrelation** within the current
    /// `decode_buf`, then removes one pitch period via Hann overlap-add.
    ///
    /// Key difference from the old cross-packet WSOLA: this correlates the
    /// signal **with itself** (autocorrelation), not two different packets.
    /// Autocorrelation on tonal audio (speech, music) almost always succeeds
    /// because periodic signals repeat themselves within a single frame.
    ///
    /// Returns `Some(n)` where `n` is the number of **interleaved samples removed**
    /// (so the orchestrator can immediately correct the filtered buffer level, as
    /// NetEQ's `BufferLevelFilter` does), or `None` if no stretch was performed.
    pub fn accelerate(
        &self,
        pcm: &[f32],
        fast_mode: bool,
        playback_buf: &mut VecDeque<f32>,
    ) -> Option<usize> {
        let ch = OPUS_CHANNELS as usize;
        let n = pcm.len() / ch;
        // Need at least 2*OLA_LEN to have non-overlapping search + reference
        if n < 2 * OLA_LEN + 16 {
            return None;
        }

        // Reference: the TAIL of the frame (last OLA_LEN sample-frames)
        let anchor = n - OLA_LEN;
        // Search limit: search window must not overlap the reference window
        let search_limit = anchor.saturating_sub(OLA_LEN);
        if search_limit == 0 {
            return None;
        }

        // --- Step 1: Autocorrelation to find pitch period ---
        // Mono-downmix the tail for fast NCC (same technique as expand)
        let mut mono_ref = [0.0f32; OLA_LEN];
        let mut ref_energy = 0.0f32;
        for (i, m) in mono_ref.iter_mut().enumerate() {
            let base = (anchor + i) * ch;
            let mono = if ch == 2 {
                (pcm[base] + pcm[base + 1]) * 0.5
            } else {
                pcm[base]
            };
            *m = mono;
            ref_energy += mono * mono;
        }

        // Search WITHIN the same frame for the best matching segment
        let mut best_d = 0usize;
        let mut best_corr = f32::NEG_INFINITY;
        for d in 0..search_limit {
            let mut cross = 0.0f32;
            let mut cand_energy = 0.0f32;
            for (i, &m) in mono_ref.iter().enumerate() {
                let base = (d + i) * ch;
                let mono_cand = if ch == 2 {
                    (pcm[base] + pcm[base + 1]) * 0.5
                } else {
                    pcm[base]
                };
                cross += m * mono_cand;
                cand_energy += mono_cand * mono_cand;
            }
            let denom = (ref_energy * cand_energy).sqrt();
            let ncc = if denom > 1e-10 { cross / denom } else { 0.0 };
            if ncc > best_corr {
                best_corr = ncc;
                best_d = d;
            }
        }

        // NetEQ thresholds: 0.9 for normal, 0.5 for fast mode (kFastAccelerate).
        // Fast mode activates when buffer is extremely overfull — trades
        // slightly lower quality for much faster drain.
        let threshold = if fast_mode { 0.5 } else { 0.9 };
        if best_corr < threshold {
            return None;
        }

        // Pitch period = distance between matching section and reference
        let pitch_period = anchor - best_d;
        if pitch_period < OLA_LEN {
            return None;
        }

        // --- Step 2: Remove pitch period(s) via overlap-add ---
        // In fast mode, remove MULTIPLE pitch periods per operation.
        // This matches NetEQ accelerate.cc:62-67:
        //   peak_index = (fs_mult_120 / peak_index) * peak_index;
        // Removing more per-op means fewer OLA crossfades needed,
        // which eliminates the 'clicking' artifacts from too-frequent
        // phase discontinuities.
        let remove_len = if fast_mode {
            // Half the frame length in sample-frames, rounded to
            // multiple of pitch period. Cap at (anchor - best_d) to
            // stay within the frame.
            let half_frame = n / 2;
            let multiples = half_frame / pitch_period;
            let multi_remove = multiples.max(1) * pitch_period;
            multi_remove.min(anchor - best_d)
        } else {
            pitch_period
        };

        // Splice point after removing `remove_len` samples:
        // Output: [0..best_d] + crossfade([best_d..], [best_d+remove_len..]) + tail
        let splice_start = best_d + remove_len;
        if splice_start + OLA_LEN > n {
            return None; // Not enough room for crossfade
        }

        // 1. [0..best_d] verbatim
        playback_buf.extend(&pcm[..best_d * ch]);

        // 2. Hann OLA crossfade between the two pitch-aligned sections
        for i in 0..OLA_LEN {
            let hann_in = self.hann_window[i];
            let hann_out = 1.0 - hann_in;
            for c in 0..ch {
                let early = pcm[(best_d + i) * ch + c];
                let late = pcm[(splice_start + i) * ch + c];
                playback_buf.push_back(early * hann_out + late * hann_in);
            }
        }

        // 3. Tail after the crossfade
        let tail_start = (splice_start + OLA_LEN) * ch;
        if tail_start < pcm.len() {
            playback_buf.extend(&pcm[tail_start..]);
        }

        // Removed audio = `remove_len` sample-frames. The output is shorter than
        // the input by exactly this many sample-frames (one or more pitch periods).
        self.note_op();
        Some(remove_len * ch)
    }
}
