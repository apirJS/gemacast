//! Time-scaling actor: WSOLA (Waveform Similarity Overlap-Add) primitives that
//! stretch or shrink decoded PCM without changing pitch. Used to slow playback
//! before an imminent starvation (expand), drain an overfull buffer
//! (accelerate), and splice across a flush boundary (overlap-add).
//!
//! Owns the pre-computed crossfade ramp and the `wsola_buf` scratch used to hold
//! the pre-flush frame while the post-flush frame is decoded. Reads decoded PCM
//! and writes stretched output through borrowed slices — it holds no decoder,
//! jitter buffer, or playback buffer of its own.

use super::consts::{
    ACCEL_WINDOW_FRAMES, COARSE_PEAKS, EXPAND_NCC_THRESHOLD, OLA_LEN, PITCH_DECIMATION,
    SEARCH_RANGE, SILENCE_RMS,
};
use crate::audio::{OPUS_CHANNELS, OPUS_FRAME_SAMPLES};
use std::collections::VecDeque;

/// Total interleaved capacity of the accelerate staging window.
const ACCEL_WINDOW_SAMPLES: usize = ACCEL_WINDOW_FRAMES * OPUS_FRAME_SAMPLES;

/// WSOLA time-scaler: crossfade ramp + scratch buffer for allocation-free splicing.
pub(super) struct TimeScaler {
    /// Pre-computed crossfade ramp for OLA splicing (OLA_LEN entries).
    fade_ramp: Vec<f32>,
    /// Pre-allocated buffer for WSOLA: holds the first frame's PCM while decoding the second.
    wsola_buf: Vec<f32>,
    /// Staging window for `accelerate`: up to `ACCEL_WINDOW_FRAMES` decoded frames
    /// laid end to end, so one splice can reach a pitch period longer than a frame.
    accel_window: Vec<f32>,
    /// Valid interleaved samples currently staged in `accel_window`.
    accel_len: usize,
    /// Mono downmix of the staged window (sample-frames), reused by the pitch search.
    mono_buf: Vec<f32>,
    /// Decimated mono window for the coarse pitch search.
    mono_dec: Vec<f32>,
    /// The previously decoded frame, kept so `expand` can correlate over ~30ms
    /// without popping a future packet out of the jitter buffer.
    ///
    /// This is NetEQ's `old_data`: `PreemptiveExpand::Process` takes
    /// `[old_data | new_data]` and requires the pair to be "(almost) 30 ms"
    /// (`preemptive_expand.cc:28-33`), where `old_data` is *already-played*
    /// audio borrowed back from the sync buffer, not a packet taken from the
    /// packet buffer. Staging a future frame instead would disable the growth
    /// actuator at `occupied <= 1` — precisely the imminent-underrun case it
    /// exists for — and emit two frames plus a period per call, parking enough
    /// surplus in the playback buffer to skip the next callback's depth control.
    hist_buf: Vec<f32>,
    /// Valid interleaved samples in `hist_buf`. Zero until the first frame is
    /// remembered, which narrows the reachable period to one frame's worth for
    /// exactly one callback after a reset.
    hist_len: usize,
    /// Terminal-seam quality of the most recent splice, in units of the incoming
    /// signal's own max slope across the crossfade region. `None` until a splice
    /// measures one, and taken (not read) by the orchestrator so a window can
    /// never re-count a stale value. `Cell` because `overlap_add` takes `&self`.
    ///
    /// See [`TimeScaler::note_splice_step`] for what the number means and why it
    /// is defined the way it is.
    last_splice_step: std::cell::Cell<Option<f32>>,
    /// Entry-seam quality of the most recent [`Self::conceal_frame`], in the same
    /// units and taken the same way as [`Self::last_splice_step`].
    ///
    /// **Deliberately a separate field, against the letter of the v21 plan.**
    /// `splice_step` has one property that makes it useful: a monotonic ramp
    /// cannot read above 1.00, so any reading above it means the fade shape
    /// regressed — v19 measured max exactly 1.000 across 204 splices and CLAUDE.md
    /// pins that as the standing tripwire. A concealment entry is a hard join, not
    /// a crossfade, so it legitimately reads *above* 1.00 whenever the material's
    /// level moved across the repeated period. Folding the two together would
    /// leave a future capture unable to tell a real fade regression from
    /// concealment landing on a decaying note.
    last_conceal_step: std::cell::Cell<Option<f32>>,
    /// Count of successful accelerate/expand splices. Test-only observability: lets
    /// artifact-regression tests assert that loud audio is NOT time-stretched.
    /// `Cell` because the stretch methods take `&self`.
    #[cfg(test)]
    op_count: std::cell::Cell<usize>,
}

impl TimeScaler {
    pub fn new() -> Self {
        Self {
            fade_ramp: Self::make_fade_ramp(),
            wsola_buf: vec![0.0f32; OPUS_FRAME_SAMPLES],
            accel_window: vec![0.0f32; ACCEL_WINDOW_SAMPLES],
            mono_buf: vec![0.0f32; ACCEL_WINDOW_SAMPLES / OPUS_CHANNELS as usize],
            mono_dec: vec![
                0.0f32;
                ACCEL_WINDOW_SAMPLES / OPUS_CHANNELS as usize / PITCH_DECIMATION + 1
            ],
            hist_buf: vec![0.0f32; OPUS_FRAME_SAMPLES],
            hist_len: 0,
            accel_len: 0,
            last_splice_step: std::cell::Cell::new(None),
            last_conceal_step: std::cell::Cell::new(None),
            #[cfg(test)]
            op_count: std::cell::Cell::new(0),
        }
    }

    /// Take the terminal-seam measurement of the last splice, clearing it.
    ///
    /// Returns `None` when no splice has happened since the previous take, or
    /// when the last one landed on material with no slope to normalise against.
    pub fn take_splice_step(&self) -> Option<f32> {
        self.last_splice_step.replace(None)
    }

    /// Take the entry-seam measurement of the last concealed frame, clearing it.
    /// See [`Self::last_conceal_step`] for why this is not `take_splice_step`.
    pub fn take_conceal_step(&self) -> Option<f32> {
        self.last_conceal_step.replace(None)
    }

    /// Measure how far the crossfade's final sample lands from the incoming
    /// signal it hands over to, in units of that signal's own steepest step
    /// across the same region.
    ///
    /// **What it is.** With `w = fade_ramp[OLA_LEN-1]`, the last emitted sample is
    /// `out = early*(1-w) + late*w`, and playback continues from the incoming
    /// (`late`) signal. The reported value is
    ///
    /// ```text
    ///   (|out - late_last| + |late_last - late_prev|) / max |Δlate|
    ///    \_______________/   \___________________/     \_________/
    ///      fade residual      the incoming signal's     steepest step
    ///                         own last step             in the region
    /// ```
    ///
    /// The fade residual is **exactly zero for a monotonic ramp** (`w == 1.0`, so
    /// `out == late_last`), which leaves the ratio at the material's own relative
    /// slope — at most 1.00 by construction. A fade that does not end on the
    /// incoming signal pushes the residual up and the ratio past 1.00, so a single
    /// number separates the two shapes without an ear.
    ///
    /// **Why not the raw terminal discontinuity.** That is `|out - late_next|`,
    /// and `late_next` is outside the staged window on the accelerate path —
    /// `splice_start + OLA_LEN == n` there by the pitch geometry, so the tail is
    /// empty and the continuation is the *next callback's* frame. Accelerate is
    /// 66-75% of all splices on 2.4GHz, so a metric that skipped it would measure
    /// the wrong 25%. Substituting the incoming signal's own last in-region step
    /// keeps one definition across all three splice sites and keeps the whole
    /// measurement inside audio the splice already touched.
    ///
    /// **Flat material is excluded, not reported as infinity.** With no slope
    /// there is nothing to normalise against; the ratio would be `inf` or `NaN`
    /// and would poison the window max. A splice there is silence-on-silence, and
    /// the two paths that act on silence (fast-forward shed, free growth) do not
    /// come through here at all.
    ///
    /// Runs only on a successful splice, and costs one pass over the crossfade
    /// region — negligible beside the pitch search that just ran (`SEARCH_RANGE`
    /// lags × `OLA_LEN` taps).
    fn note_splice_step(&self, early: &[f32], early_at: usize, late: &[f32], late_at: usize) {
        if let Some(v) =
            Self::seam_step(early, early_at, late, late_at, self.fade_ramp[OLA_LEN - 1])
        {
            self.last_splice_step.set(Some(v));
        }
    }

    /// Measure the concealment **entry** seam — the one hard join in
    /// [`Self::conceal_frame`], where the last sample actually played is followed
    /// by a sample taken one pitch period earlier.
    ///
    /// Takes the same two indices as [`Self::note_splice_step`] and means the same
    /// thing by them; the mapping is exact rather than convenient. Playback
    /// continues from the emitted period, whose first sample is `win[n - P]`; that
    /// is `late_next` in the parent's definition, so the incoming region ends on
    /// its predecessor `win[n - P - 1] = win[best_d + OLA_LEN - 1]`, i.e. the
    /// incoming region starts at `best_d`. The last sample already emitted is
    /// `win[n - 1] = win[anchor + OLA_LEN - 1]`, so the outgoing region starts at
    /// `anchor`. `residual + last_step` then bounds the true discontinuity
    /// `|win[n-1] - win[n-P]|` exactly as it does for a crossfade.
    ///
    /// The one difference is `handover = 0.0`: a concealed frame's entry is not a
    /// crossfade handover. `note_splice_step` supplies the terminal ramp weight
    /// `fade_ramp[OLA_LEN-1] == 1.0`, which drives the residual to a structural
    /// zero and would report this seam as flawless whatever it is. At `w = 0` the
    /// residual becomes the real quantity — the distance between the last played
    /// sample and where the emitted period begins, bounded but not erased by the
    /// correlation the pitch search maximized.
    ///
    /// Reporting the entry rather than the period wrap is the conservative half:
    /// the wrap is source-adjacent, so its residual is exactly zero and its reading
    /// would be this one's `last_step / slope`, which this value dominates term by
    /// term. It lands in its own field — see [`Self::last_conceal_step`].
    fn note_conceal_step(&self, win: &[f32], anchor: usize, best_d: usize) {
        if let Some(v) = Self::seam_step(win, anchor, win, best_d, 0.0) {
            self.last_conceal_step.set(Some(v));
        }
    }

    /// Shared body of [`Self::note_splice_step`] and [`Self::note_conceal_step`].
    /// `handover` is the weight the emitted signal gives the incoming source at
    /// the final sample of the region: 1.0 for a completed crossfade, 0.0 for a
    /// hard join. `None` on flat material. See [`Self::note_splice_step`] for what
    /// the number means.
    fn seam_step(
        early: &[f32],
        early_at: usize,
        late: &[f32],
        late_at: usize,
        handover: f32,
    ) -> Option<f32> {
        let ch = OPUS_CHANNELS as usize;
        let w = handover;
        let mut slope = 0.0f32;
        let mut residual = 0.0f32;
        let mut last_step = 0.0f32;
        for c in 0..ch {
            let mut prev = late[late_at * ch + c];
            for i in 1..OLA_LEN {
                let s = late[(late_at + i) * ch + c];
                slope = slope.max((s - prev).abs());
                prev = s;
            }
            let late_last = late[(late_at + OLA_LEN - 1) * ch + c];
            let late_prev = late[(late_at + OLA_LEN - 2) * ch + c];
            let early_last = early[(early_at + OLA_LEN - 1) * ch + c];
            let out_last = early_last * (1.0 - w) + late_last * w;
            residual = residual.max((out_last - late_last).abs());
            last_step = last_step.max((late_last - late_prev).abs());
        }
        if slope > f32::EPSILON {
            Some((residual + last_step) / slope)
        } else {
            None
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

    /// v18: the fade must be a *monotonic* ramp, not a full Hann bell.
    ///
    /// The old `0.5 * (1 - cos(2π i / N))` runs 0 → 1 → 0, so the crossfade
    /// ended back on the **outgoing** signal while the tail continues from the
    /// **incoming** one — a terminal discontinuity in multiples of the signal's
    /// own slope, which is what the OLA exists to remove. It was masked only by
    /// the 0.9 NCC gate making the two sources nearly equal.
    ///
    /// NetEQ's crossfade is the same monotonic shape: a linear alpha ramp
    /// (`audio_vector.cc:259-267`). A raised-cosine ramp matches that and gives
    /// zero derivative at both ends, so the seams are slope-continuous as well.
    fn make_fade_ramp() -> Vec<f32> {
        (0..OLA_LEN)
            .map(|i| 0.5 * (1.0 - (std::f32::consts::PI * i as f32 / (OLA_LEN - 1) as f32).cos()))
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

    /// Begin an accelerate window with the frame that is about to be played.
    ///
    /// The window may only ever hold audio that has **not** been emitted yet: a
    /// splice deletes samples, and samples already handed to `fill_output` are
    /// gone. So the window is rebuilt per drain decision from the freshly decoded
    /// frame, then optionally extended with the next contiguous frame.
    pub fn window_begin(&mut self, pcm: &[f32]) {
        let len = pcm.len().min(ACCEL_WINDOW_SAMPLES);
        self.accel_window[..len].copy_from_slice(&pcm[..len]);
        self.accel_len = len;
    }

    /// Append one more decoded frame to the accelerate window.
    /// Returns `false` (and stages nothing) if the frame would not fit.
    pub fn window_extend(&mut self, pcm: &[f32]) -> bool {
        if self.accel_len + pcm.len() > ACCEL_WINDOW_SAMPLES {
            return false;
        }
        self.accel_window[self.accel_len..self.accel_len + pcm.len()].copy_from_slice(pcm);
        self.accel_len += pcm.len();
        true
    }

    /// How many more interleaved samples the accelerate window can still take.
    pub fn window_headroom(&self) -> usize {
        ACCEL_WINDOW_SAMPLES - self.accel_len
    }

    /// The staged accelerate window.
    pub fn window(&self) -> &[f32] {
        &self.accel_window[..self.accel_len]
    }

    /// Emit the staged window verbatim — the no-splice path. Every staged frame
    /// must reach the playback buffer exactly once, whether or not the splice fired.
    pub fn emit_window(&self, playback_buf: &mut VecDeque<f32>) {
        playback_buf.extend(&self.accel_window[..self.accel_len]);
    }

    /// Overlap-Add WSOLA splice (allocation-free).
    ///
    /// Reads pcm1 from `self.wsola_buf[..pcm1_len]` and pcm2 from `pcm2`.
    /// Finds the best phase-aligned splice point via **mono-downmixed** normalized
    /// cross-correlation (halves FMA count vs full-stereo, enables NEON auto-vectorization),
    /// then applies a monotonic crossfade ramp on full stereo. Writes output to
    /// `playback_buf`.
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
        // 2. OLA crossfade (full stereo for transparent output). The ramp ends at
        //    1.0, so the last faded sample is pure `pcm2` and the verbatim tail
        //    below resumes from `best_d + OLA_LEN` with no step.
        for i in 0..OLA_LEN {
            let fade_in = self.fade_ramp[i];
            let fade_out = 1.0 - fade_in;
            for c in 0..ch {
                let r = self.wsola_buf[(anchor + i) * ch + c];
                let s = pcm2[(best_d + i) * ch + c];
                playback_buf.push_back(r * fade_out + s * fade_in);
            }
        }

        // 3. pcm2[best_d+OLA_LEN..] verbatim (bulk extend)
        let tail_start = (best_d + OLA_LEN) * ch;
        if tail_start < pcm2_len {
            playback_buf.extend(&pcm2[tail_start..pcm2_len]);
        }

        self.note_splice_step(&self.wsola_buf, anchor, pcm2, best_d);
        true
    }

    /// Keep `pcm` as the "already played" half of `expand`'s search window.
    ///
    /// Must be called with the frame that is about to leave the decoder — i.e.
    /// *before* the next packet overwrites `decode_buf` — on every callback that
    /// emits contiguous audio. One frame stale is the most this may ever be: the
    /// expand rate limiter is 20 callbacks, so remembering only when a splice
    /// fires would leave the history 200 ms old and destroy the correlation the
    /// window exists to measure.
    pub fn remember(&mut self, pcm: &[f32]) {
        let len = pcm.len().min(OPUS_FRAME_SAMPLES);
        self.hist_buf[..len].copy_from_slice(&pcm[..len]);
        self.hist_len = len;
    }

    /// Drop the history because the emitted stream is about to be discontinuous
    /// (post-starvation fade-in, hole skip, stream reset).
    ///
    /// A splice reaches back into the history for its tail, so history that is
    /// not adjacent to the current frame would replay *stale* audio rather than
    /// one pitch period. `expand` then runs on the current frame alone, over the
    /// narrower but still non-degenerate `[OLA_LEN, 352]` period range.
    pub fn forget(&mut self) {
        self.hist_len = 0;
    }

    /// Whether a previous frame is staged for `expand` to correlate against.
    ///
    /// Exists so the manager's `remember` / `forget` wiring is testable at the
    /// seam it actually matters on — the decode path and the discontinuity
    /// branches — rather than by reaching into the buffer.
    #[cfg(test)]
    pub fn has_history(&self) -> bool {
        self.hist_len > 0
    }

    /// Conceal one frame by repeating the last played frame's pitch period, for
    /// the case the codec cannot: no decoder state to extrapolate from
    /// (`FrameDecoder::plc_is_valid`).
    ///
    /// This is NetEQ's `Expand` reaching a place ours could not. `expand_conceal`
    /// needs a *current* frame to splice into, so it is structurally unreachable at
    /// `occupied == 0`: `expand_inner` stages `[history | pcm]` and refuses when
    /// `anchor < hist_frames`, and with history alone `n = 480`,
    /// `hist_frames = 480`, `anchor = 352 < 480` → `None`. Same search, run over
    /// the history by itself.
    ///
    /// Writes exactly `OPUS_FRAME_SAMPLES` **un-muted** samples into `out`, copies
    /// them back over the history so the next concealed callback continues in phase
    /// (NetEQ appends its expansion to `sync_buffer_` for the same reason), and
    /// returns `true`. Returns `false` having written nothing when no history is
    /// staged or the window cannot hold a reference plus a crossfade — which leaves
    /// the caller on the codec path, unchanged.
    ///
    /// Geometry, every bound checked against the existing constants:
    ///
    /// ```text
    /// stage    accel_window[..960] = hist_buf[..960]    the slot already exists
    /// n        = 480 sample-frames                       the last played frame
    /// anchor   = n - OLA_LEN                    = 352
    /// limit    = min(SEARCH_RANGE 720, anchor - OLA_LEN 224) = 224
    /// search   find_pitch_period(480, 352, 224) -> best_d in [0, 224)
    /// P        = anchor - best_d in [129, 352]           = 136-375 Hz
    /// ```
    ///
    /// Non-degenerate by construction: `dec_len = 480/12 = 40 > ref_taps = 10`, so
    /// [`Self::find_pitch_period`]'s early return cannot fire, and `P >= 129 > 128`
    /// so neither can the period guard.
    ///
    /// One period is `hist[n-P..anchor]` verbatim — length `P - OLA_LEN`, at least
    /// 1 — then an `OLA_LEN` crossfade of `hist[anchor+i]` into `hist[best_d+i]`,
    /// repeated to fill the frame.
    ///
    /// **The period wrap carries zero discontinuity.** The crossfade ends at
    /// `w = 1` on `hist[best_d + OLA_LEN - 1] = hist[n - P - 1]`, and the next
    /// period opens on `hist[n - P]`, the immediately following sample *in the
    /// source*. Repeating `hist[n-P..n]` naively would instead re-open a
    /// correlation-bounded seam every period — up to 3.7 per emitted frame at the
    /// minimum period. This opens exactly one, at the entry, and
    /// [`Self::note_conceal_step`] measures that one.
    pub fn conceal_frame(&mut self, out: &mut [f32]) -> bool {
        let ch = OPUS_CHANNELS as usize;
        let hist_len = self.hist_len;
        if out.len() < OPUS_FRAME_SAMPLES || hist_len == 0 {
            return false;
        }
        let n = hist_len / ch;
        // Same floor as `expand_inner`: reference + crossfade + slack.
        if n < 2 * OLA_LEN + 16 {
            return false;
        }
        self.accel_window[..hist_len].copy_from_slice(&self.hist_buf[..hist_len]);
        self.accel_len = hist_len;

        let anchor = n - OLA_LEN;
        let search_limit = SEARCH_RANGE.min(anchor.saturating_sub(OLA_LEN));
        if search_limit == 0 {
            return false;
        }
        // No correlation gate, and deliberately so: upstream's `Expand` has none
        // either (`expand.cc:438-455` scores candidates and always emits one),
        // because what a concealed frame displaces is raw underrun. A badly
        // correlated period beats the hole it replaces.
        let (best_d, _) = self.find_pitch_period(n, anchor, search_limit);
        let period = anchor - best_d;
        // Guaranteed by the search bound (`best_d <= 223` at `n = 480`); kept so
        // the invariant is local to the splice rather than inferred from a caller.
        if period < OLA_LEN {
            return false;
        }

        let verbatim = period - OLA_LEN;
        let head = n - period;
        for p in 0..OPUS_FRAME_SAMPLES / ch {
            let q = p % period;
            for c in 0..ch {
                out[p * ch + c] = if q < verbatim {
                    self.accel_window[(head + q) * ch + c]
                } else {
                    let i = q - verbatim;
                    let fade_in = self.fade_ramp[i];
                    self.accel_window[(anchor + i) * ch + c] * (1.0 - fade_in)
                        + self.accel_window[(best_d + i) * ch + c] * fade_in
                };
            }
        }

        // Measured before the history is overwritten: the join between the last
        // real sample, `hist[n-1]`, and the source predecessor of the first sample
        // emitted, `hist[n-P-1]`.
        self.note_conceal_step(&self.accel_window, anchor, best_d);

        // Remember the extension un-muted. The caller applies the concealment fade
        // to `out` afterwards, so the gain reaches the DAC without compounding into
        // the next callback's source — and the next callback stays in phase.
        self.hist_buf[..OPUS_FRAME_SAMPLES].copy_from_slice(&out[..OPUS_FRAME_SAMPLES]);
        self.hist_len = OPUS_FRAME_SAMPLES;
        // Deliberately not `note_op()`: that counter exists so artifact tests can
        // assert loud *playing* audio is never time-stretched, and concealment
        // replaces audio that is not there. The 1 Hz `conceal_run` counts this.
        true
    }

    /// NetEQ Preemptive Expand (Method 1).
    /// Stretches audio by exactly one pitch period (up to 15ms) to slow down
    /// playback and prevent an imminent starvation gap.
    ///
    /// v16 repair — two coupled changes to the search geometry:
    ///
    /// * **The window is `[history | pcm]`**, porting upstream's input contract:
    ///   `PreemptiveExpand::Process` takes `[old_data | new_data]` and requires
    ///   the pair to be "(almost) 30 ms" (`preemptive_expand.cc:28-33`), where
    ///   `old_data` is already-played audio borrowed back from the sync buffer
    ///   (written back verbatim via `ReplaceAtIndex`) — **not** a packet taken
    ///   from the packet buffer. Only `pcm` and the inserted period are emitted;
    ///   the history is search material only.
    /// * **The lag floor is `OLA_LEN`, not 16 sample-frames**, matching
    ///   `accelerate` and upstream's `kMinLag = 10` at 4 kHz = 120 sample-frames
    ///   at 48 kHz (`time_stretch.cc:74-75`), plus the `pitch_period < OLA_LEN`
    ///   post-guard `accelerate` already carried.
    ///
    /// v15's one-frame window with a 16-sample-frame floor admitted periods in
    /// `[17, 352]`, and on lowpass-dominant material the NCC argmax landed on the
    /// shortest admissible lag: **50.0% of measured v15 expand splices had a
    /// period shorter than the crossfade (min 19 sample-frames = 0.4 ms), against
    /// `accelerate`'s 0.0%** over the same captures. Such a splice overlaps its
    /// own reference region — a comb filter with its first notch at `48000/(2P)`,
    /// plus a replayed tail. That is the "buzzing / fast-clicks on buffer
    /// increase" in the v15 field reports. The same geometry refused 75-93% of
    /// attempts, which is why growth authority measured 0.059-0.434 frames/s
    /// against the 2.6-5.6 frames/s the target step-ups needed.
    ///
    /// Returns `Some(n)` where `n` is the number of **interleaved samples
    /// inserted** (so the orchestrator can immediately correct the filtered
    /// buffer level, as NetEQ's `BufferLevelFilter` does), or `None` if no
    /// stretch was performed. On `Some`, `pcm` plus the inserted period has been
    /// written to `playback_buf`; on `None` nothing was written and the caller
    /// emits `pcm` itself.
    pub fn expand(
        &mut self,
        pcm: &[f32],
        rms: f32,
        playback_buf: &mut VecDeque<f32>,
    ) -> Option<usize> {
        self.expand_inner(pcm, rms, playback_buf, false)
    }

    /// NetEQ **`Expand`** (`expand.cc`) — concealment, for when the packet buffer
    /// has actually run dry.
    ///
    /// Identical geometry to [`Self::expand`] and **no correlation gate**, because
    /// upstream's `Expand` has none: it picks the best of `kNumCorrelationCandidates`
    /// lags by a correlation/distortion *ratio*
    /// ([expand.cc:438-455](TEMP/webrtc-neteq/expand.cc#L438)) and always emits.
    /// There is no path through `expand.cc` that returns "no output" — concealment
    /// is not allowed to decline, because the alternative is not "slightly worse
    /// audio", it is a hole.
    ///
    /// **These are two upstream operations, and v19 had them merged into one.**
    /// `PreemptiveExpand` grows the buffer while a packet is still in hand and can
    /// afford to wait for a good seam; `Expand` runs at `occupied <= 1` where
    /// waiting means silence. v19 applied `PreemptiveExpand`'s 0.9 NCC gate to
    /// both, so the underrun tier refused **83 of 90 attempts (7.8% accept) on
    /// 2.4GHz uncompressed** and emitted 830ms of raw underrun instead of spliced
    /// concealment. Those declines predict the damage they cause: windows carrying
    /// a `declined_underrun_ncc` average **2.556 starvations against 0.216**
    /// without (Spearman r=+0.323, p=5.6e-14, n=514). See `TEMP/v20-plan.md`.
    ///
    /// The trade this takes is deliberate and bounded: a poorly correlated splice
    /// at `occupied <= 1` replaces **raw underrun**, which is the worst artifact
    /// this module can produce. `splice_step` is the tripwire — it must stay
    /// <= 1.00, as it measured in v19 (n=204, max 1.000).
    ///
    /// Every other refusal path is geometric and unreachable at our frame size:
    /// `n >= 272` sample-frames is satisfied by a bare 480-frame packet with no
    /// history, and the search bounds already guarantee `pitch_period > OLA_LEN`.
    /// So `declined_underrun_ncc` must read **0** in the field; a non-zero value
    /// means a geometry refusal, not a quality one, and is a tripwire in exactly
    /// the way `declined_rms_mask` is.
    pub fn expand_conceal(
        &mut self,
        pcm: &[f32],
        rms: f32,
        playback_buf: &mut VecDeque<f32>,
    ) -> Option<usize> {
        self.expand_inner(pcm, rms, playback_buf, true)
    }

    /// Shared body of [`Self::expand`] and [`Self::expand_conceal`].
    ///
    /// `conceal` selects which upstream operation this is: `false` =
    /// `PreemptiveExpand` (gated), `true` = `Expand` (ungated). Nothing else
    /// differs — sharing the body is what stops the two from drifting apart, the
    /// same reason `find_pitch_period` is shared with `accelerate`.
    fn expand_inner(
        &mut self,
        pcm: &[f32],
        rms: f32,
        playback_buf: &mut VecDeque<f32>,
        conceal: bool,
    ) -> Option<usize> {
        let ch = OPUS_CHANNELS as usize;
        // Stage `[history | pcm]`. `pcm` alone must fit even with no history, and
        // the history is dropped rather than truncated if the pair would overflow.
        let hist_len = if self.hist_len + pcm.len() <= ACCEL_WINDOW_SAMPLES {
            self.hist_len
        } else {
            0
        };
        if pcm.len() > ACCEL_WINDOW_SAMPLES {
            return None;
        }
        self.accel_window[..hist_len].copy_from_slice(&self.hist_buf[..hist_len]);
        self.accel_window[hist_len..hist_len + pcm.len()].copy_from_slice(pcm);
        self.accel_len = hist_len + pcm.len();

        let n = self.accel_len / ch;
        let hist_frames = hist_len / ch;
        // Need enough audio to cover reference + an OLA_LEN crossfade + slack.
        if n < 2 * OLA_LEN + 16 {
            return None;
        }

        // The reference is the window's tail, as in `accelerate`. The emitted head
        // starts at `hist_frames`, so the crossfade must sit inside `pcm`:
        // `anchor >= hist_frames`, i.e. `pcm` must be at least OLA_LEN long.
        let anchor = n - OLA_LEN;
        if anchor < hist_frames {
            return None;
        }
        let search_limit = SEARCH_RANGE.min(anchor.saturating_sub(OLA_LEN));
        if search_limit == 0 {
            return None;
        }

        // Find the pitch period via the same coarse-then-refine search `accelerate`
        // uses. Sharing the search guarantees identical geometry and identical cost.
        let (best_d, best_corr) = self.find_pitch_period(n, anchor, search_limit);

        // Upstream's disjunction, not a bare threshold (`accelerate.cc:58`):
        //
        //     if ((best_correlation > correlation_threshold) || !active_speech)
        //
        // The correlation check is the quality gate; the VAD is an **escape**.
        // Upstream states the reason in `SetParametersForPassiveSpeech`
        // (`accelerate.cc:39-44`): "when the signal does not contain any active
        // speech, the correlation does not matter" — it forces the correlation
        // to zero and admits the splice anyway. There is nothing periodic in a
        // near-silent window for a seam to warble against.
        //
        // We ported the threshold in v14 and left the escape behind. The v14
        // field census priced that omission: `declined_ncc` took **78% of drain
        // attempts on ADB (405 against 90 splices) and 79% on 5 GHz (740
        // against 94)**, on the very links whose targets are small enough that
        // the drain is the only thing standing between a correct target and a
        // buffer parked above it.
        //
        // `SILENCE_RMS` is the activity threshold, reused rather than tuned
        // fresh: it is the same -46dBFS line the silence fast-forward shed and
        // free silence growth already treat as "nothing here to damage", and
        // both have been live for four rounds without an artifact report. This
        // adds a third behavioural path at the same threshold, which is why the
        // constant's doc comment now names all three.
        //
        // `conceal` skips the gate entirely — see [`Self::expand_conceal`]. That
        // is not a relaxation of this threshold, it is the recognition that the
        // concealment tier is a *different upstream operation* (`expand.cc`) which
        // carries no correlation gate at all.
        //
        // The threshold is [`EXPAND_NCC_THRESHOLD`] = 0.85, not `accelerate`'s
        // 0.9. Upstream runs both at 0.9; we split them because v19 measured
        // growth admitted on **11.9% / 7.1%** of attempts while the buffer sat
        // below its own low limit, with every other decline reason reading zero.
        // The constant carries the acceptance and artifact curves that priced the
        // move, including the −5.72dB knee at 0.80 that bounds it from below. The
        // drain keeps 0.9 because it is not the actuator that was starving.
        let active_speech = rms >= SILENCE_RMS;
        if !conceal && best_corr < EXPAND_NCC_THRESHOLD && active_speech {
            return None;
        }

        // Pitch period = distance between matching section and reference. The
        // search bound already guarantees this, but the guard is kept so the
        // invariant is local to the splice rather than inferred from the caller.
        let pitch_period = anchor - best_d;
        if pitch_period < OLA_LEN {
            return None;
        }

        // --- Insert one pitch period via overlap-add ---
        // Emission begins at `hist_frames`: the history was already played and is
        // search material only. Total emitted = `pcm` + `pitch_period`, so the
        // playback buffer never carries more than one period of surplus and the
        // next callback still runs `process_next_frame` (depth control, ingest,
        // drain and the static flush all live there).
        //
        // 1. [hist_frames..anchor] verbatim
        playback_buf.extend(&self.accel_window[hist_frames * ch..anchor * ch]);
        // 2. OLA crossfade between reference and the pitch-aligned candidate.
        //    Hands over completely to the candidate, so the tail below — which
        //    resumes at `best_d + OLA_LEN` — is slope-continuous with it.
        for i in 0..OLA_LEN {
            let fade_in = self.fade_ramp[i];
            let fade_out = 1.0 - fade_in;
            for c in 0..ch {
                let r = self.accel_window[(anchor + i) * ch + c];
                let s = self.accel_window[(best_d + i) * ch + c];
                playback_buf.push_back(r * fade_out + s * fade_in);
            }
        }
        // 3. Tail after the crossfade — one pitch period, ending exactly on the
        //    last sample of `pcm`, so the next frame remains contiguous.
        let tail_start = (best_d + OLA_LEN) * ch;
        if tail_start < self.accel_len {
            playback_buf.extend(&self.accel_window[tail_start..self.accel_len]);
        }

        // Inserted audio = one pitch period of sample-frames. The output is longer
        // than `pcm` by exactly this many sample-frames.
        self.note_splice_step(&self.accel_window, anchor, &self.accel_window, best_d);
        self.note_op();
        Some(pitch_period * ch)
    }

    /// NetEQ-style acceleration over a two-frame staging window.
    ///
    /// Searches for a pitch period via decimated autocorrelation across the
    /// staged window (prev frame + current frame = up to 20ms), then removes one
    /// pitch period via a crossfaded overlap-add.
    ///
    /// Key difference from the old cross-packet WSOLA: this correlates the
    /// signal **with itself** (autocorrelation), not two different packets.
    /// Autocorrelation on tonal audio (speech, music) almost always succeeds
    /// because periodic signals repeat themselves within a single window.
    ///
    /// Returns `Some(n)` where `n` is the number of **interleaved samples removed**
    /// (so the orchestrator can immediately correct the filtered buffer level, as
    /// NetEQ's `BufferLevelFilter` does), or `None` if no stretch was performed.
    ///
    /// The window must have been staged with `window_begin` (+ `window_extend`),
    /// and on every outcome the caller must emit the window exactly once — via
    /// `window()` for the returned splice, or `emit_window` when `None`.
    pub fn accelerate(
        &mut self,
        fast_mode: bool,
        rms: f32,
        playback_buf: &mut VecDeque<f32>,
    ) -> Option<usize> {
        let ch = OPUS_CHANNELS as usize;
        let n = self.accel_len / ch;
        // Need enough audio to cover reference + an OLA_LEN crossfade + slack.
        if n < 2 * OLA_LEN + 16 {
            return None;
        }

        // Reference: the TAIL of the window (last OLA_LEN sample-frames). Periods
        // up to SEARCH_RANGE away become reachable because the search scans the
        // whole window, not a single 480-frame.
        //
        // The lag bound is `anchor - OLA_LEN`, not `anchor`: a lag closer than
        // OLA_LEN to the reference yields a period too short to splice, and
        // admitting it would let the search pick a global best it must then
        // reject — discarding a valid shorter-lag candidate with it. Every `d`
        // the search can return is splicesable by construction, since
        // `best_d + pitch_period + OLA_LEN == n` for `pitch_period = anchor - best_d`.
        let anchor = n - OLA_LEN;
        let search_limit = SEARCH_RANGE.min(anchor.saturating_sub(OLA_LEN));
        if search_limit == 0 {
            return None;
        }

        // --- Step 1: Autocorrelation to find the pitch period ---
        let (best_d, best_corr) = self.find_pitch_period(n, anchor, search_limit);

        // NetEQ thresholds: 0.9 for normal, 0.5 for fast mode (kFastAccelerate).
        // Fast mode activates when buffer is extremely overfull — trades
        // slightly lower quality for much faster drain.
        //
        // The `|| !active_speech` half is upstream's (`accelerate.cc:58`) and is
        // documented at length on the `expand` gate above — same escape, same
        // `SILENCE_RMS` threshold, same reasoning. It applies to both tiers: a
        // near-silent window has nothing to warble whether or not the buffer is
        // in the emergency band, and gating the escape on tier would make the
        // *quiet* case stricter than the loud one.
        let threshold = if fast_mode { 0.5 } else { 0.9 };
        let active_speech = rms >= SILENCE_RMS;
        if best_corr < threshold && active_speech {
            return None;
        }

        // Pitch period = distance between matching section and reference.
        let pitch_period = anchor - best_d;
        if pitch_period < OLA_LEN {
            return None;
        }

        // --- Step 2: Remove one pitch period via overlap-add ---
        // A single removal per call. The prior multi-period fast path
        // (`(fs_mult_120 / peak_index) * peak_index`) was deleted with the
        // single-frame window; the widened window already removes up to 15ms
        // per op, so the drain rate no longer needs multiple periods per splice
        // to keep up with an overrun.
        let splice_start = best_d + pitch_period;
        if splice_start + OLA_LEN > n {
            return None; // Not enough room for the crossfade
        }

        // 1. [0..best_d] verbatim
        playback_buf.extend(&self.accel_window[..best_d * ch]);

        // 2. OLA crossfade between the two pitch-aligned sections. Ends fully on
        //    the late section, which the verbatim tail continues from.
        for i in 0..OLA_LEN {
            let fade_in = self.fade_ramp[i];
            let fade_out = 1.0 - fade_in;
            for c in 0..ch {
                let early = self.accel_window[(best_d + i) * ch + c];
                let late = self.accel_window[(splice_start + i) * ch + c];
                playback_buf.push_back(early * fade_out + late * fade_in);
            }
        }

        // 3. Tail after the crossfade
        let tail_start = (splice_start + OLA_LEN) * ch;
        if tail_start < self.accel_len {
            playback_buf.extend(&self.accel_window[tail_start..self.accel_len]);
        }

        // Removed audio = `pitch_period` sample-frames. The output is shorter than
        // the window by exactly this many sample-frames.
        self.note_splice_step(&self.accel_window, best_d, &self.accel_window, splice_start);
        self.note_op();
        Some(pitch_period * ch)
    }

    /// Coarse-then-refine pitch search over the staged window, shared by
    /// `accelerate` and `expand`.
    ///
    /// Returns `(best_d, best_corr)` — the lag whose OLA_LEN-tap normalized
    /// cross-correlation against the window's tail reference is highest, and that
    /// correlation. `anchor - best_d` is the pitch period; the caller applies the
    /// NCC threshold and the `pitch_period < OLA_LEN` guard.
    ///
    /// Decimated coarse sweep, then a full-rate refine around the best hills.
    /// A naive full-rate sweep of the 2-frame window (~592 lags x 128 taps)
    /// would be ~76k FMA per decision; coarse-then-refine lands near 12k.
    /// NetEQ correlates in the same 4kHz domain (`time_stretch.cc:56-60`).
    ///
    /// Shared rather than duplicated so the two operations cannot drift apart
    /// again: v15 shipped `expand` with a 16-sample-frame lag floor over a
    /// single frame against `accelerate`'s `OLA_LEN` floor over two, and the
    /// field measured a 50% degenerate-splice rate on one and 0% on the other.
    fn find_pitch_period(&mut self, n: usize, anchor: usize, search_limit: usize) -> (usize, f32) {
        let ch = OPUS_CHANNELS as usize;
        {
            let (mono, win) = (&mut self.mono_buf, &self.accel_window);
            for (i, m) in mono.iter_mut().take(n).enumerate() {
                let base = i * ch;
                *m = if ch == 2 {
                    (win[base] + win[base + 1]) * 0.5
                } else {
                    win[base]
                };
            }
        }
        let dec_len = n / PITCH_DECIMATION;
        let ref_taps = OLA_LEN / PITCH_DECIMATION;
        if dec_len <= ref_taps || ref_taps == 0 {
            return (0, f32::NEG_INFINITY);
        }
        {
            let (dec, mono) = (&mut self.mono_dec, &self.mono_buf);
            for (j, m) in dec.iter_mut().take(dec_len).enumerate() {
                *m = mono[j * PITCH_DECIMATION];
            }
        }
        let dec_ref_start = dec_len - ref_taps;
        let ref_energy_dec: f32 = self.mono_dec[dec_ref_start..dec_len]
            .iter()
            .map(|v| v * v)
            .sum();
        let max_dec = (search_limit / PITCH_DECIMATION).min(dec_ref_start);

        // Keep the strongest `COARSE_PEAKS` *distinct* coarse hills. Refining only
        // the single best bin can miss the true full-rate maximum, because
        // decimation blurs neighbouring periods into one hill. Adjacent bins are
        // merged rather than allowed to fill every slot — three bins from one hill
        // would refine the same lag range three times and leave real second and
        // third candidates untested.
        let mut peaks = [(f32::NEG_INFINITY, usize::MAX); COARSE_PEAKS];
        for k in 0..=max_dec {
            let ncc = self.coarse_ncc(k, dec_ref_start, ref_taps, ref_energy_dec);
            if let Some(slot) = peaks
                .iter()
                .position(|&(_, p)| p != usize::MAX && p.abs_diff(k) <= 1)
            {
                if ncc > peaks[slot].0 {
                    peaks[slot] = (ncc, k);
                }
                continue;
            }
            let mut weakest = 0usize;
            for (i, p) in peaks.iter().enumerate() {
                if p.0 < peaks[weakest].0 {
                    weakest = i;
                }
            }
            if ncc > peaks[weakest].0 {
                peaks[weakest] = (ncc, k);
            }
        }

        // Refine each coarse peak at full rate over the sample-frames it covers.
        let ref_start = anchor;
        let ref_energy: f32 = self.mono_buf[ref_start..ref_start + OLA_LEN]
            .iter()
            .map(|v| v * v)
            .sum();
        let mut best_d = 0usize;
        let mut best_corr = f32::NEG_INFINITY;
        for &(_, pk) in &peaks {
            if pk == usize::MAX {
                continue;
            }
            let lo = (pk * PITCH_DECIMATION).saturating_sub(PITCH_DECIMATION);
            let hi = ((pk + 1) * PITCH_DECIMATION + PITCH_DECIMATION)
                .min(search_limit)
                .min(anchor.saturating_sub(OLA_LEN));
            for d in lo..hi {
                let mut cross = 0.0f32;
                let mut cand_energy = 0.0f32;
                for i in 0..OLA_LEN {
                    let rv = self.mono_buf[ref_start + i];
                    let cv = self.mono_buf[d + i];
                    cross += rv * cv;
                    cand_energy += cv * cv;
                }
                let denom = (ref_energy * cand_energy).sqrt();
                let ncc = if denom > 1e-10 { cross / denom } else { 0.0 };
                if ncc > best_corr {
                    best_corr = ncc;
                    best_d = d;
                }
            }
        }
        (best_d, best_corr)
    }

    /// Normalized cross-correlation of the decimated window at coarse lag `k`,
    /// against the decimated reference at `dec_ref_start`.
    fn coarse_ncc(&self, k: usize, dec_ref_start: usize, ref_taps: usize, ref_energy: f32) -> f32 {
        let mut cross = 0.0f32;
        let mut cand_energy = 0.0f32;
        for t in 0..ref_taps {
            let cv = self.mono_dec[k + t];
            cross += cv * self.mono_dec[dec_ref_start + t];
            cand_energy += cv * cv;
        }
        let denom = (ref_energy * cand_energy).sqrt();
        if denom > 1e-10 { cross / denom } else { 0.0 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One frame of a steady stereo sine at `hz`, phase-continuous from `frame_idx`.
    fn tone_frame(hz: f32, amp: f32, frame_idx: u64) -> Vec<f32> {
        let ch = OPUS_CHANNELS as usize;
        let frames = OPUS_FRAME_SAMPLES / ch;
        let base = frame_idx * frames as u64;
        let mut pcm = vec![0.0f32; OPUS_FRAME_SAMPLES];
        for i in 0..frames {
            let t = (base + i as u64) as f32 / 48_000.0;
            let s = (2.0 * std::f32::consts::PI * hz * t).sin() * amp;
            for c in 0..ch {
                pcm[i * ch + c] = s;
            }
        }
        pcm
    }

    /// Deterministic white noise (no `rand` dependency in the audio crate's tests).
    fn noise_frame(amp: f32, seed: &mut u32) -> Vec<f32> {
        let mut pcm = vec![0.0f32; OPUS_FRAME_SAMPLES];
        for s in pcm.iter_mut() {
            *seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            *s = ((*seed >> 8) as f32 / 8_388_608.0 - 1.0) * amp;
        }
        pcm
    }

    mod window_geometry {
        use super::*;

        /// The defect this round exists to fix. A 100 Hz tone has a 480-sample
        /// period — exactly one frame, and therefore unreachable by the old
        /// single-frame search, whose splicesable periods stopped at 352 samples
        /// (136 Hz). Two staged frames must find and remove it.
        #[test]
        fn should_find_a_pitch_period_below_136hz_that_the_single_frame_search_missed() {
            let mut ts = TimeScaler::new();
            let mut out = VecDeque::new();

            // One frame only: the old geometry. 480 samples cannot hold a 480-sample
            // period plus an OLA_LEN crossfade, so this must decline.
            ts.window_begin(&tone_frame(100.0, 0.05, 0));
            assert!(
                ts.accelerate(false, 0.05, &mut out).is_none(),
                "a single 480-frame window cannot splice a 480-sample period — \
                 if this passes, the test no longer proves the widening did anything",
            );

            // Two frames: the period is now splicesable.
            out.clear();
            ts.window_begin(&tone_frame(100.0, 0.05, 0));
            assert!(ts.window_extend(&tone_frame(100.0, 0.05, 1)));
            let removed = ts
                .accelerate(false, 0.05, &mut out)
                .expect("100 Hz must be reachable in a two-frame window");

            let ch = OPUS_CHANNELS as usize;
            let period = removed / ch;
            assert!(
                (440..=520).contains(&period),
                "expected ~480 sample-frames (100 Hz at 48kHz), removed {period}",
            );
            assert_eq!(
                out.len(),
                2 * OPUS_FRAME_SAMPLES - removed,
                "output must be exactly the staged window minus the removed period",
            );
        }

        /// The window is rebuilt per drain decision, so a caller that stages only
        /// one frame (no contiguous successor available) must still behave as the
        /// old code did rather than panic on a short window.
        #[test]
        fn should_still_splice_a_single_staged_frame_without_a_successor() {
            let mut ts = TimeScaler::new();
            let mut out = VecDeque::new();
            // 200 Hz = 240 sample-frames, splicesable inside one frame (as before).
            ts.window_begin(&tone_frame(200.0, 0.05, 0));
            let removed = ts
                .accelerate(false, 0.05, &mut out)
                .expect("200 Hz was reachable before this change and must stay so");
            assert_eq!(out.len(), OPUS_FRAME_SAMPLES - removed);
        }

        /// An empty or stub window must decline, not index out of bounds.
        #[test]
        fn should_decline_an_empty_window_without_panicking() {
            let mut ts = TimeScaler::new();
            let mut out = VecDeque::new();
            // RMS 0.0 deliberately: that is *below* `SILENCE_RMS`, so the VAD
            // escape is standing and the correlation gate is fully bypassed.
            // These windows must therefore be refused on geometry alone — which
            // is the bounds check this test exists to guard.
            ts.window_begin(&[]);
            assert!(ts.accelerate(false, 0.0, &mut out).is_none());
            assert!(out.is_empty());

            ts.window_begin(&vec![0.0f32; 64]);
            assert!(ts.accelerate(true, 0.0, &mut out).is_none());
        }

        /// v16: no `expand` splice may ever have a period shorter than the
        /// crossfade window. Such a splice overlaps its own reference region — a
        /// comb filter with its first notch at `48000/(2P)`, plus a replayed
        /// tail. v15's one-frame window with a 16-sample-frame lag floor admitted
        /// [17, 352] and **50.0% of measured splices landed shorter than
        /// `OLA_LEN`** (min 19 sample-frames = 0.4 ms), against `accelerate`'s
        /// 0.0% over the same captures. That is the "buzzing / fast-clicks on
        /// buffer increase" in the v15 field reports.
        ///
        /// The floor does not *refuse* high-frequency content — a 2500 Hz tone is
        /// periodic at every multiple of its 19.2-sample period, so the search
        /// lands on a harmonic at or above the floor and splices cleanly. That is
        /// the repair working: the constraint moves the choice, it does not
        /// forfeit the splice. Swept across fundamentals both far below and far
        /// above the floor so a regression cannot hide in one octave.
        #[test]
        fn expand_should_never_splice_a_period_shorter_than_the_crossfade_window() {
            let ch = OPUS_CHANNELS as usize;
            let mut spliced = 0;
            for hz in [100.0f32, 150.0, 220.0, 330.0, 800.0, 1500.0, 2500.0, 4000.0] {
                let mut ts = TimeScaler::new();
                let mut out = VecDeque::new();
                ts.remember(&tone_frame(hz, 0.05, 0));
                if let Some(inserted) = ts.expand(&tone_frame(hz, 0.05, 1), 0.05, &mut out) {
                    spliced += 1;
                    let period = inserted / ch;
                    assert!(
                        period >= OLA_LEN,
                        "{hz} Hz spliced a {period}-sample-frame period, shorter than \
                         OLA_LEN={OLA_LEN} — the splice overlaps its own reference region"
                    );
                    assert_eq!(
                        out.len(),
                        OPUS_FRAME_SAMPLES + inserted,
                        "{hz} Hz: emission must be the current frame plus one period"
                    );
                }
            }
            // A ceiling-only assertion passes vacuously when nothing fires. The
            // floor must be shown to admit splices, not merely to refuse them.
            assert!(
                spliced >= 6,
                "the lag floor must move the search onto a harmonic, not forfeit the \
                 splice — only {spliced}/8 tones spliced at all"
            );
        }

        /// v16: the widened window must let `expand` reach the same low-frequency
        /// periods `accelerate` already could. A 100 Hz tone is exactly one frame
        /// (480 samples), unreachable by v15's single-frame `expand` but reachable
        /// by its two-frame `accelerate`.
        #[test]
        fn expand_should_reach_the_same_period_range_as_accelerate() {
            let mut ts = TimeScaler::new();
            let mut out = VecDeque::new();
            let a = tone_frame(100.0, 0.05, 0);
            let b = tone_frame(100.0, 0.05, 1);
            ts.remember(&a);
            let inserted = ts
                .expand(&b, 0.05, &mut out)
                .expect("100 Hz must be reachable in [history | current]");

            let ch = OPUS_CHANNELS as usize;
            let period = inserted / ch;
            assert!(
                (440..=520).contains(&period),
                "expected ~480 sample-frames (100 Hz at 48kHz), inserted {period}"
            );
            // Output is one frame plus the inserted period.
            assert_eq!(
                out.len(),
                OPUS_FRAME_SAMPLES + inserted,
                "expand must emit exactly the current frame plus one pitch period"
            );
        }

        /// v16: when expand and accelerate share the same search (`find_pitch_period`),
        /// they must agree on the pitch period for the same audio. Guards against
        /// the geometry drift that produced v15's 50% vs 0% degenerate rates.
        #[test]
        fn expand_and_accelerate_should_agree_on_the_pitch_period_for_the_same_window() {
            for hz in [100.0f32, 150.0, 220.0] {
                let mut ts_exp = TimeScaler::new();
                let mut out_exp = VecDeque::new();
                let a = tone_frame(hz, 0.05, 0);
                let b = tone_frame(hz, 0.05, 1);
                ts_exp.remember(&a);
                let inserted = ts_exp
                    .expand(&b, 0.05, &mut out_exp)
                    .unwrap_or_else(|| panic!("{hz} Hz must be reachable by expand"));

                let mut ts_acc = TimeScaler::new();
                let mut out_acc = VecDeque::new();
                ts_acc.window_begin(&a);
                assert!(ts_acc.window_extend(&b));
                let removed = ts_acc
                    .accelerate(false, 0.05, &mut out_acc)
                    .unwrap_or_else(|| panic!("{hz} Hz must be reachable by accelerate"));

                let ch = OPUS_CHANNELS as usize;
                let period_exp = inserted / ch;
                let period_acc = removed / ch;
                assert_eq!(
                    period_exp, period_acc,
                    "{hz} Hz: expand and accelerate must find the same pitch period \
                     (expand={period_exp}, accelerate={period_acc})"
                );
            }
        }

        /// `emit_window` is the no-splice path and must reproduce every staged
        /// sample exactly once — the invariant that keeps a declined accelerate
        /// from dropping or duplicating a frame of audio.
        #[test]
        fn emit_window_should_reproduce_every_staged_sample_once() {
            let mut ts = TimeScaler::new();
            let mut out = VecDeque::new();
            let f0 = tone_frame(440.0, 0.2, 0);
            let f1 = tone_frame(440.0, 0.2, 1);
            ts.window_begin(&f0);
            assert!(ts.window_extend(&f1));
            ts.emit_window(&mut out);

            let expected: Vec<f32> = f0.iter().chain(f1.iter()).copied().collect();
            assert_eq!(out.len(), expected.len());
            assert!(out.iter().zip(&expected).all(|(a, b)| a == b));
        }

        /// The window must never accept more than it can hold.
        #[test]
        fn window_extend_should_refuse_a_frame_that_does_not_fit() {
            let mut ts = TimeScaler::new();
            ts.window_begin(&tone_frame(440.0, 0.2, 0));
            assert!(ts.window_extend(&tone_frame(440.0, 0.2, 1)));
            assert_eq!(ts.window_headroom(), 0);
            assert!(!ts.window_extend(&tone_frame(440.0, 0.2, 2)));
            assert_eq!(ts.window().len(), ACCEL_WINDOW_SAMPLES);
        }
    }

    mod crossfade_ramp {
        use super::*;

        /// Two staged frames of a 200 Hz tone — period exactly 240 sample-frames —
        /// under a linear amplitude taper, phased so the window's **final** sample
        /// sits on a sine peak.
        ///
        /// The taper is what makes the seam observable at all. Normalized
        /// correlation is gain-invariant, so every 240-multiple lag still clears
        /// the 0.9 gate, but the aligned sections no longer hold equal *values*.
        /// On a steady tone `early[i] == late[i]`, and then any pair of weights
        /// summing to 1 produces byte-identical output — which is exactly how a
        /// bell-shaped fade stayed invisible to this suite for four rounds.
        fn tapered_tone_window() -> Vec<f32> {
            tapered_tone_window_phased(std::f32::consts::FRAC_PI_2)
        }

        /// The same window with the phase of its **final** sample chosen by the
        /// caller. `FRAC_PI_2` puts it on a sine peak (zero local slope); `0.0`
        /// puts it on a zero crossing, where the signal's own step is at its
        /// maximum — the two ends of the range the splice-step metric normalises
        /// against.
        fn tapered_tone_window_phased(end_phase: f32) -> Vec<f32> {
            let ch = OPUS_CHANNELS as usize;
            let frames = ACCEL_WINDOW_SAMPLES / ch;
            let last = (frames - 1) as f32;
            let period = 240.0f32;
            // Put i = frames-1 at `end_phase`: 2*pi*(frames-1)/P + phase == end_phase + 8*pi.
            let phase =
                end_phase - 2.0 * std::f32::consts::PI * last / period + 8.0 * std::f32::consts::PI;
            let mut pcm = vec![0.0f32; ACCEL_WINDOW_SAMPLES];
            for i in 0..frames {
                let gain = 1.0 - 0.7 * i as f32 / last;
                let s = 0.2 * gain * (2.0 * std::f32::consts::PI * i as f32 / period + phase).sin();
                for c in 0..ch {
                    pcm[i * ch + c] = s;
                }
            }
            pcm
        }

        /// The v18 defect, pinned at the source. `make_hann_window` built a full
        /// Hann *bell* (0 -> 1 -> 0), so `fade_in` came back to ~0 by the end of
        /// the fade: the crossfade closed on the **outgoing** signal while the
        /// verbatim tail resumed from the **incoming** one. A monotonic ramp is
        /// the only shape that hands over, and it is upstream's own — a linear
        /// alpha ramp in `TEMP/webrtc-neteq/audio_vector.cc:247-267`.
        #[test]
        fn the_crossfade_ramp_must_run_monotonically_from_zero_to_one() {
            let ts = TimeScaler::new();
            let ramp = &ts.fade_ramp;
            assert_eq!(ramp.len(), OLA_LEN);
            assert!(
                ramp[0].abs() < 1e-6,
                "the fade must open on the outgoing signal, got {}",
                ramp[0],
            );
            assert!(
                (ramp[OLA_LEN - 1] - 1.0).abs() < 1e-6,
                "the fade must close on the incoming signal, got {} — a bell closes near 0",
                ramp[OLA_LEN - 1],
            );
            for i in 1..OLA_LEN {
                assert!(
                    ramp[i] >= ramp[i - 1],
                    "the ramp turned back at i={i}: {} -> {}",
                    ramp[i - 1],
                    ramp[i],
                );
            }
        }

        /// Handover end-to-end through `overlap_add`. Both sides are constant, so
        /// the assertion holds whichever lag the search picks: the output opens on
        /// `pcm1`'s value, closes on `pcm2`'s, and reaches the verbatim tail
        /// without a step. Under the bell the fade returned to +0.25 and then
        /// stepped a full 1.0 into a tail sitting at -0.75.
        #[test]
        fn a_crossfade_must_close_on_the_incoming_signal_and_leave_no_step() {
            let ch = OPUS_CHANNELS as usize;
            let mut ts = TimeScaler::new();
            let pcm1 = vec![0.25f32; OPUS_FRAME_SAMPLES];
            let pcm2 = vec![-0.75f32; OPUS_FRAME_SAMPLES];
            ts.snapshot(&pcm1);
            let mut out = VecDeque::new();
            assert!(
                ts.overlap_add(pcm1.len(), &pcm2, true, &mut out),
                "a forced crossfade must splice — a declined one proves nothing",
            );

            let o: Vec<f32> = out.iter().copied().collect();
            let anchor = OPUS_FRAME_SAMPLES / ch - OLA_LEN;
            assert!(
                (o[anchor * ch] - 0.25).abs() < 1e-6,
                "fade opened on {}, not pcm1's 0.25",
                o[anchor * ch],
            );
            let close = (anchor + OLA_LEN - 1) * ch;
            assert!(
                (o[close] + 0.75).abs() < 1e-6,
                "fade closed on {}, not pcm2's -0.75",
                o[close],
            );
            // Every consecutive step must be a *fade* step. The raised-cosine
            // ramp's steepest increment is 0.5*pi/(OLA_LEN-1) = 0.0124, so a
            // 1.0-wide crossfade cannot move more than that per sample-frame.
            for i in 1..o.len() / ch {
                let step = (o[i * ch] - o[(i - 1) * ch]).abs();
                assert!(step < 0.02, "discontinuity of {step} at sample-frame {i}");
            }
        }

        /// `accelerate`'s `splice_start == anchor` identically, so its verbatim
        /// tail is always empty and the **last output sample is the fade's last
        /// sample**. A ramp makes that the window's own final sample, so the next
        /// callback's frame continues from where this one stopped. The bell left
        /// the *early* section's value there instead (~0.095 against 0.060 on this
        /// signal) and the following frame stepped into it.
        #[test]
        fn accelerate_must_close_the_window_on_its_own_final_sample() {
            let ch = OPUS_CHANNELS as usize;
            let win = tapered_tone_window();
            let mut ts = TimeScaler::new();
            let mut out = VecDeque::new();
            ts.window_begin(&win[..OPUS_FRAME_SAMPLES]);
            assert!(ts.window_extend(&win[OPUS_FRAME_SAMPLES..]));
            let removed = ts
                .accelerate(false, 0.09, &mut out)
                .expect("a gain-tapered 200 Hz tone must still clear the NCC gate");
            assert_eq!(out.len(), ACCEL_WINDOW_SAMPLES - removed);

            let o: Vec<f32> = out.iter().copied().collect();
            for c in 0..ch {
                let got = o[o.len() - ch + c];
                let want = win[ACCEL_WINDOW_SAMPLES - ch + c];
                assert!(
                    (got - want).abs() < 1e-4,
                    "accelerate closed on {got}, not the window's final sample {want}",
                );
            }
        }

        /// `expand` does have a verbatim tail, so its seam is internal: the fade
        /// must close on the candidate at `best_d + OLA_LEN - 1` so that the tail
        /// resuming at `best_d + OLA_LEN` carries the signal's own step and nothing
        /// more. The bell closed on the *reference* — the window's last sample —
        /// and then jumped a pitch period backwards into the tail.
        #[test]
        fn expand_must_hand_over_to_the_candidate_at_the_tail_seam() {
            let ch = OPUS_CHANNELS as usize;
            let win = tapered_tone_window();
            let mut ts = TimeScaler::new();
            let mut out = VecDeque::new();
            ts.remember(&win[..OPUS_FRAME_SAMPLES]);
            let inserted = ts
                .expand(&win[OPUS_FRAME_SAMPLES..], 0.09, &mut out)
                .expect("a gain-tapered 200 Hz tone must still clear the NCC gate");
            assert_eq!(out.len(), OPUS_FRAME_SAMPLES + inserted);

            let hist_frames = OPUS_FRAME_SAMPLES / ch;
            let anchor = ACCEL_WINDOW_SAMPLES / ch - OLA_LEN;
            let best_d = anchor - inserted / ch;
            // Output sample-frame `anchor - hist_frames + OLA_LEN - 1` closes the
            // fade; the one after it is the first verbatim tail sample.
            let close = anchor - hist_frames + OLA_LEN - 1;
            let o: Vec<f32> = out.iter().copied().collect();
            for c in 0..ch {
                let got = o[close * ch + c];
                let want = win[(best_d + OLA_LEN - 1) * ch + c];
                assert!(
                    (got - want).abs() < 1e-4,
                    "expand closed the fade on {got}, not the candidate's {want}",
                );
                let seam = (o[(close + 1) * ch + c] - got).abs();
                let natural =
                    (win[(best_d + OLA_LEN) * ch + c] - win[(best_d + OLA_LEN - 1) * ch + c]).abs();
                assert!(
                    seam <= natural + 1e-4,
                    "seam step {seam} exceeds the signal's own step {natural}",
                );
            }
        }
        /// v19's splice-quality metric, pinned to the property that makes a field
        /// reading interpretable. `splice_step` is the terminal seam of a splice,
        /// in units of the incoming signal's own steepest step across the
        /// crossfade region:
        ///
        /// ```text
        /// (|out_last - late_last| + |late_last - late_prev|) / max|Δlate|
        /// ```
        ///
        /// The first term is the **fade residual** — how far the last emitted
        /// sample sits from the incoming signal it was supposed to hand over to.
        /// A monotonic ramp closes at exactly 1.0, so that term is exactly zero
        /// and the ratio collapses to `|late_last - late_prev| / max|Δlate|`,
        /// which cannot exceed 1.00: the numerator is one of the steps the
        /// denominator maximises over. **1.00 is the ceiling a correct fade
        /// cannot cross**, so a field reading above it says the handover is
        /// leaving a step the signal never had. The v17 bell closed at ~0.0006
        /// instead, which puts nearly the whole gap between the two sections into
        /// the residual and drives the ratio well past 1.
        ///
        /// The raw form — `|out_last - late_next|` against the incoming signal's
        /// *next* sample — is not computable on `accelerate`, which is 66-75% of
        /// all splices on 2.4GHz: its geometry gives `splice_start + OLA_LEN == n`
        /// identically, so there is no verbatim tail and `late_next` lives in the
        /// next callback's frame. One definition readable at all three sites is
        /// worth more than an exact one readable at one.
        ///
        /// Asserted at all three splice sites, because they differ in which buffer
        /// plays which role and a metric wired to the wrong section would still
        /// return a plausible-looking number. Asserted at both ends of the
        /// normaliser's range, because a ceiling the signal never approaches
        /// bounds nothing: with the seam on a sine peak the local step is near
        /// zero and the ratio is ~0.04-0.07, while with the seam on a zero
        /// crossing — maximum slope — it lands at 0.77-0.84, just under the bound.
        #[test]
        fn the_reported_splice_step_should_never_exceed_the_signals_own_slope() {
            // (label, window, lower bound the readings must clear)
            let cases = [
                ("seam on a peak", tapered_tone_window(), 0.0f32),
                ("seam at max slope", tapered_tone_window_phased(0.0), 0.5),
            ];
            for (label, win, floor) in cases {
                let mut readings = Vec::new();

                // `accelerate`: both sections live inside the staged window, and
                // the fade's last sample *is* the window's last output sample.
                let mut ts = TimeScaler::new();
                let mut out = VecDeque::new();
                ts.window_begin(&win[..OPUS_FRAME_SAMPLES]);
                assert!(ts.window_extend(&win[OPUS_FRAME_SAMPLES..]));
                assert!(
                    ts.accelerate(false, 0.09, &mut out).is_some(),
                    "{label}: a gain-tapered 200 Hz tone must clear the NCC gate — \
                     a declined splice reports nothing and proves nothing",
                );
                readings.push(("accelerate", ts.take_splice_step()));
                assert!(
                    ts.take_splice_step().is_none(),
                    "{label}: the reading must be *taken*, not read — a value left \
                     behind would be re-counted into the next 1Hz window",
                );

                // `expand`: reference at the window tail, candidate a pitch period
                // back, and a verbatim tail after the fade.
                let mut ts = TimeScaler::new();
                let mut out = VecDeque::new();
                ts.remember(&win[..OPUS_FRAME_SAMPLES]);
                assert!(
                    ts.expand(&win[OPUS_FRAME_SAMPLES..], 0.09, &mut out)
                        .is_some(),
                    "{label}: expand must splice for its reading to exist",
                );
                readings.push(("expand", ts.take_splice_step()));

                // `overlap_add`: the two sections are in *different* buffers.
                let mut ts = TimeScaler::new();
                let mut out = VecDeque::new();
                ts.snapshot(&win[..OPUS_FRAME_SAMPLES]);
                assert!(
                    ts.overlap_add(
                        OPUS_FRAME_SAMPLES,
                        &win[OPUS_FRAME_SAMPLES..],
                        true,
                        &mut out,
                    ),
                    "{label}: a forced crossfade must splice",
                );
                readings.push(("overlap_add", ts.take_splice_step()));

                for (site, reading) in readings {
                    let step = reading.unwrap_or_else(|| {
                        panic!(
                            "{label}: {site} spliced without reporting a step — an \
                             unreported site is invisible in the field, which is \
                             the blindness this metric exists to remove"
                        )
                    });
                    assert!(
                        step <= 1.0 + 1e-4,
                        "{label}: {site} reported {step:.4}, above the 1.00 a \
                         monotonic ramp cannot exceed — the fade is not closing on \
                         the incoming signal",
                    );
                    assert!(
                        step >= floor,
                        "{label}: {site} reported {step:.4}, below the {floor} this \
                         geometry must reach — a ceiling the signal never \
                         approaches bounds nothing",
                    );
                }
            }
        }
    }

    mod correlation_gate {
        use super::*;

        /// The NCC veto is the whole artifact defence and this round does not
        /// touch it: widening *where* a good splice may be found grants no
        /// permission to make a worse one. Loud broadband noise has no pitch
        /// period and must still be refused.
        #[test]
        fn should_still_refuse_to_stretch_loud_broadband_noise() {
            let mut ts = TimeScaler::new();
            let mut out = VecDeque::new();
            let mut seed = 0x1234_5678u32;
            ts.window_begin(&noise_frame(0.3, &mut seed));
            assert!(ts.window_extend(&noise_frame(0.3, &mut seed)));
            // RMS 0.3 is 35x `SILENCE_RMS`, so `active_speech` is true and the
            // NCC gate is live. Passing a quiet RMS here would satisfy the
            // assertion's *letter* via the VAD escape while proving nothing
            // about the veto — see `an_active_speech_window_should_still_require_correlation`
            // for the escape's own coverage.
            assert!(
                ts.accelerate(false, 0.3, &mut out).is_none(),
                "white noise cleared the 0.9 NCC gate — the artifact veto is broken",
            );
            assert_eq!(ts.op_count(), 0);
        }

        /// The escape itself: the *same* unsplicable window, declared quiet.
        ///
        /// Paired deliberately with the test above — identical content, identical
        /// seed, identical staging, and the only difference is the RMS. That is
        /// what makes this falsifiable: if the OR is ever rewritten back to an
        /// AND, or the threshold comparison is inverted, exactly one of this pair
        /// fails and the other still passes. A single test either way would leave
        /// the mistake invisible.
        ///
        /// Upstream's reasoning, `accelerate.cc:39-44` — on passive speech it
        /// zeroes the correlation outright, because "the correlation does not
        /// matter" when there is no periodic signal to damage.
        #[test]
        fn a_low_energy_window_should_admit_a_splice_that_fails_correlation() {
            let mut ts = TimeScaler::new();
            let mut out = VecDeque::new();
            let mut seed = 0x1234_5678u32;
            ts.window_begin(&noise_frame(0.3, &mut seed));
            assert!(ts.window_extend(&noise_frame(0.3, &mut seed)));

            let removed = ts.accelerate(false, SILENCE_RMS * 0.5, &mut out).expect(
                "a below-SILENCE_RMS window must bypass the NCC gate — this is \
                     the 78-79% of ADB/5GHz drain attempts v14 refused",
            );
            assert!(removed > 0, "an admitted splice must remove real audio");
            assert_eq!(
                out.len(),
                ACCEL_WINDOW_SAMPLES - removed,
                "the escape must still splice correctly, not merely return Some",
            );
        }

        /// The boundary is `>=`, not `>`: a window sitting exactly on
        /// `SILENCE_RMS` counts as active and keeps its correlation requirement.
        ///
        /// Worth pinning because `SILENCE_RMS` now gates three behavioural paths
        /// (silence fast-forward shed, free silence growth, and this escape), and
        /// the other two treat the threshold as "quiet enough to damage nothing".
        /// An off-by-one in the comparison would widen the most artifact-prone of
        /// the three by one ulp while leaving the safe two untouched.
        #[test]
        fn an_active_speech_window_should_still_require_correlation() {
            let mut ts = TimeScaler::new();
            let mut out = VecDeque::new();
            let mut seed = 0x1234_5678u32;
            ts.window_begin(&noise_frame(0.3, &mut seed));
            assert!(ts.window_extend(&noise_frame(0.3, &mut seed)));

            assert!(
                ts.accelerate(false, SILENCE_RMS, &mut out).is_none(),
                "RMS exactly at SILENCE_RMS is active speech — the NCC gate must \
                 still veto unsplicable content",
            );
            assert_eq!(ts.op_count(), 0);
        }

        /// The fast tier drops the threshold 0.9 → 0.5, and the escape must not
        /// be tier-conditional: a quiet window is quiet in either tier. Guards
        /// against a future refactor that applies the escape only to the normal
        /// path — which would leave the emergency drain *stricter* on silence
        /// than the rate-limited one, exactly backwards.
        #[test]
        fn the_escape_should_apply_on_the_fast_tier_as_well() {
            let mut ts = TimeScaler::new();
            let mut out = VecDeque::new();
            let mut seed = 0xfeed_beefu32;
            ts.window_begin(&noise_frame(0.3, &mut seed));
            assert!(ts.window_extend(&noise_frame(0.3, &mut seed)));
            assert!(
                ts.accelerate(true, SILENCE_RMS * 0.5, &mut out).is_some(),
                "the VAD escape must be tier-independent",
            );
        }

        /// `expand` carries the same gate and must carry the same escape. It is
        /// the growth actuator commit 2 just put on the `filtered < low_limit`
        /// path, so a gate that vetoes 79% of attempts there costs occupancy on
        /// the links that are already below target.
        #[test]
        fn expand_should_take_the_same_escape_as_accelerate() {
            let mut seed = 0x0bad_c0deu32;
            let a = noise_frame(0.3, &mut seed);
            let b = noise_frame(0.3, &mut seed);

            let mut ts = TimeScaler::new();
            let mut loud = VecDeque::new();
            ts.remember(&a);
            assert!(
                ts.expand(&b, 0.3, &mut loud).is_none(),
                "loud unsplicable content must still be refused by expand",
            );

            let mut ts = TimeScaler::new();
            let mut quiet = VecDeque::new();
            ts.remember(&a);
            assert!(
                ts.expand(&b, SILENCE_RMS * 0.5, &mut quiet).is_some(),
                "a below-SILENCE_RMS window must bypass expand's NCC gate too",
            );
        }

        /// The concealment tier is upstream's `Expand` (`expand.cc`), which has no
        /// correlation gate — it always emits. This is the *same* loud unsplicable
        /// noise the test above proves `expand` refuses, so the pairing is what
        /// makes the claim falsifiable: identical input, two tiers, two outcomes.
        ///
        /// Without this, `declined_underrun_ncc` took 83 of 90 attempts on 2.4GHz
        /// uncompressed and emitted 830ms of raw underrun instead.
        #[test]
        fn the_concealment_tier_should_splice_even_when_correlation_is_poor() {
            let mut seed = 0x0bad_c0deu32;
            let a = noise_frame(0.3, &mut seed);
            let b = noise_frame(0.3, &mut seed);

            let mut gated = TimeScaler::new();
            let mut gated_out = VecDeque::new();
            gated.remember(&a);
            assert!(
                gated.expand(&b, 0.3, &mut gated_out).is_none(),
                "precondition: this content must fail the preemptive NCC gate, \
                 otherwise the concealment assertion below passes vacuously",
            );

            let mut ts = TimeScaler::new();
            let mut out = VecDeque::new();
            ts.remember(&a);
            let inserted = ts
                .expand_conceal(&b, 0.3, &mut out)
                .expect("concealment must never decline — upstream's Expand cannot");
            assert!(inserted > 0, "an admitted splice must insert real audio");
            assert_eq!(
                out.len(),
                b.len() + inserted,
                "concealment must emit the frame plus exactly the inserted period",
            );
        }

        /// The other half of commit 1's split: relaxing concealment must not
        /// relax growth. `expand` is still `PreemptiveExpand` and still applies
        /// [`EXPAND_NCC_THRESHOLD`], because a packet is in hand and the splice
        /// can afford to wait for a good seam.
        ///
        /// Loud broadband noise measures NCC ≈ 0.54 at best, so it fails 0.85 and
        /// 0.9 alike — this test is about the *presence* of the gate, and the pair
        /// below is about where it sits.
        #[test]
        fn the_preemptive_tier_should_still_decline_a_poorly_correlated_splice() {
            let mut seed = 0x0bad_c0deu32;
            let a = noise_frame(0.3, &mut seed);
            let b = noise_frame(0.3, &mut seed);

            let mut ts = TimeScaler::new();
            let mut out = VecDeque::new();
            ts.remember(&a);
            assert!(
                ts.expand(&b, 0.3, &mut out).is_none(),
                "the growth tier must keep its quality gate",
            );
            assert!(out.is_empty(), "a declined splice must write nothing");
            assert_eq!(ts.op_count(), 0);
        }

        /// The split: growth admits at [`EXPAND_NCC_THRESHOLD`], the drain does
        /// not follow it.
        ///
        /// A 200 Hz tone with 45% white noise measures NCC ≈ 0.865 through this
        /// module's real search — above 0.85, below 0.90. That band is the entire
        /// difference between the two thresholds, so this pair of tests cannot
        /// both pass by accident: if the constant were reverted to 0.9 the first
        /// fails (it would decline), and if `accelerate` were dragged down with it
        /// the second fails (it would admit). The band is asserted as a
        /// precondition so a future change in search geometry fails with a
        /// message about the material, not a confusing mismatch.
        #[test]
        fn preemptive_expand_should_admit_at_the_measured_threshold() {
            let mut seed = 0x1234_5678u32;
            let mk = |idx: u64, seed: &mut u32| {
                let t = tone_frame(200.0, 0.165, idx);
                let n = noise_frame(0.135, seed);
                t.iter()
                    .zip(n.iter())
                    .map(|(a, b)| a + b)
                    .collect::<Vec<f32>>()
            };
            let a = mk(0, &mut seed);
            let b = mk(1, &mut seed);

            let mut ts = TimeScaler::new();
            let mut out = VecDeque::new();
            ts.remember(&a);
            let inserted = ts.expand(&b, 0.3, &mut out).expect(
                "growth must admit a splice the drain still refuses — this is the \
                 11.9% / 7.1% field acceptance (2.4GHz uncompressed / 128kbps) that \
                 v20 raises, worth +5777 / +4738ms against 6607 / 1138ms of \
                 measured starvation",
            );
            assert!(inserted > 0, "an admitted splice must insert real audio");
            // Measured *after* the call: the staging window is built inside
            // `expand_inner`, so this reads the exact window the gate judged.
            let n = ts.accel_len / OPUS_CHANNELS as usize;
            let anchor = n - OLA_LEN;
            let lim = SEARCH_RANGE.min(anchor.saturating_sub(OLA_LEN));
            let (_, corr) = ts.find_pitch_period(n, anchor, lim);
            assert!(
                corr > EXPAND_NCC_THRESHOLD && corr < 0.9,
                "test material must land strictly between the two thresholds, or \
                 this admission says nothing about where the gate sits; measured \
                 {corr:.4}",
            );
        }

        /// The other half of the split: the drain keeps 0.9 even though growth
        /// no longer does. It is not the actuator that was starving — 128kbps
        /// discard was already falling through v19 — and it has the silence
        /// fast-forward shed as its artifact-free escape on quiet material, so
        /// there is no starvation in the field that this threshold is in the way
        /// of.
        #[test]
        fn the_drain_should_keep_its_own_threshold() {
            let mut seed = 0x1234_5678u32;
            let mk = |idx: u64, seed: &mut u32| {
                let t = tone_frame(200.0, 0.165, idx);
                let n = noise_frame(0.135, seed);
                t.iter()
                    .zip(n.iter())
                    .map(|(a, b)| a + b)
                    .collect::<Vec<f32>>()
            };
            let a = mk(0, &mut seed);
            let b = mk(1, &mut seed);

            let mut ts = TimeScaler::new();
            let mut out = VecDeque::new();
            ts.window_begin(&a);
            assert!(ts.window_extend(&b));
            let (_, corr) = {
                let n = ts.accel_len / OPUS_CHANNELS as usize;
                let anchor = n - OLA_LEN;
                let lim = SEARCH_RANGE.min(anchor.saturating_sub(OLA_LEN));
                ts.find_pitch_period(n, anchor, lim)
            };
            assert!(
                corr > EXPAND_NCC_THRESHOLD && corr < 0.9,
                "test material must land strictly between the two thresholds; \
                 measured {corr:.4}",
            );
            assert!(
                ts.accelerate(false, 0.3, &mut out).is_none(),
                "the drain keeps 0.9 — the same splice growth admits must be \
                 refused here",
            );
            assert_eq!(ts.op_count(), 0);
        }
    }

    mod decimated_search {
        use super::*;

        /// Guards the coarse/refine indexing. The decimated search is an
        /// optimisation, not a behaviour change: on tonal material it must land on
        /// the same pitch period an exhaustive full-rate sweep would have picked.
        #[test]
        fn decimated_search_should_agree_with_an_exhaustive_full_rate_sweep() {
            for hz in [100.0f32, 150.0, 220.0, 330.0] {
                let mut ts = TimeScaler::new();
                let mut out = VecDeque::new();
                ts.window_begin(&tone_frame(hz, 0.05, 0));
                assert!(ts.window_extend(&tone_frame(hz, 0.05, 1)));
                let removed = ts
                    .accelerate(false, 0.05, &mut out)
                    .unwrap_or_else(|| panic!("{hz} Hz must be reachable"));

                // Exhaustive reference sweep over the same staged window.
                let ch = OPUS_CHANNELS as usize;
                let n = ACCEL_WINDOW_SAMPLES / ch;
                let win = tone_frame(hz, 0.05, 0)
                    .iter()
                    .chain(tone_frame(hz, 0.05, 1).iter())
                    .copied()
                    .collect::<Vec<f32>>();
                let mono: Vec<f32> = (0..n)
                    .map(|i| (win[i * ch] + win[i * ch + 1]) * 0.5)
                    .collect();
                let anchor = n - OLA_LEN;
                let limit = SEARCH_RANGE.min(anchor - OLA_LEN);
                let ref_energy: f32 = mono[anchor..anchor + OLA_LEN].iter().map(|v| v * v).sum();
                let (mut best_d, mut best) = (0usize, f32::NEG_INFINITY);
                for d in 0..limit {
                    let mut cross = 0.0f32;
                    let mut energy = 0.0f32;
                    for i in 0..OLA_LEN {
                        cross += mono[anchor + i] * mono[d + i];
                        energy += mono[d + i] * mono[d + i];
                    }
                    let denom = (ref_energy * energy).sqrt();
                    let ncc = if denom > 1e-10 { cross / denom } else { 0.0 };
                    if ncc > best {
                        best = ncc;
                        best_d = d;
                    }
                }

                let exhaustive_period = anchor - best_d;
                let got_period = removed / ch;
                // Both must land on the same pitch multiple. A one-period slip is
                // a real disagreement, not float noise, so the tolerance is tight.
                assert!(
                    got_period.abs_diff(exhaustive_period) <= 2,
                    "{hz} Hz: decimated search removed {got_period}, exhaustive \
                     sweep would remove {exhaustive_period} — coarse/refine indexing is off",
                );
            }
        }
    }

    /// v21: concealment from the played history, for the case the codec cannot
    /// extrapolate (`FrameDecoder::plc_is_valid == false`).
    mod pitch_concealment {
        use super::*;

        /// One frame of gain-tapered 200 Hz tone — pitch period 240 sample-frames,
        /// which the search can reach (`P = 352 - best_d`, so `best_d = 112`, inside
        /// the `[0, 224)` bound).
        ///
        /// **The taper is load-bearing**, for the same reason it is in
        /// [`super::crossfade_ramp::tapered_tone_window`]: correlation is
        /// gain-invariant, so the search still locks on, but the repeated period no
        /// longer holds *equal values* to the one it replaces. On a steady tone every
        /// candidate lag at a pitch multiple is byte-identical to every other, and a
        /// wrongly-joined repetition would be indistinguishable from a correct one.
        ///
        /// **So is the end phase.** The naive repetition this splice is built to
        /// avoid joins `hist[n-1]` to `hist[n-P]`, two samples one period apart, so
        /// its error is the *taper's* gain difference times `sin(phase)`, while the
        /// correct join's error is the material's own slope, `cos(phase)`. Their
        /// ratio is `≈20·|tan(phase)|` — at a zero crossing the two joins are within
        /// 1.5× of each other and no assertion can separate them (measured: naive
        /// 0.00496 vs source 0.00339 with the final sample on a crossing). `π/4`
        /// puts the ratio near 25× while leaving a real slope at the seam.
        fn tapered_tone_frame() -> Vec<f32> {
            let ch = OPUS_CHANNELS as usize;
            let frames = OPUS_FRAME_SAMPLES / ch;
            let last = (frames - 1) as f32;
            let period = 240.0f32;
            let two_pi = 2.0 * std::f32::consts::PI;
            // Put i = frames-1 at pi/4, staying positive so the `.sin()` argument
            // never carries a large-magnitude phase.
            let phase = std::f32::consts::FRAC_PI_4 - two_pi * last / period + 8.0 * two_pi;
            let mut pcm = vec![0.0f32; OPUS_FRAME_SAMPLES];
            for i in 0..frames {
                let gain = 1.0 - 0.7 * i as f32 / last;
                let s = 0.2 * gain * (two_pi * i as f32 / period + phase).sin();
                for c in 0..ch {
                    pcm[i * ch + c] = s;
                }
            }
            pcm
        }

        /// The period the splice actually used. Re-runs the same search over the
        /// same staged window — `conceal_frame` leaves `accel_window` holding the
        /// history it copied in, and the search is a pure function of it, so this is
        /// the splice's own answer rather than a second opinion.
        fn period_used(ts: &mut TimeScaler) -> usize {
            let n = OPUS_FRAME_SAMPLES / OPUS_CHANNELS as usize;
            let anchor = n - OLA_LEN;
            let (best_d, _) = ts.find_pitch_period(n, anchor, SEARCH_RANGE.min(anchor - OLA_LEN));
            anchor - best_d
        }

        /// The v21 defect, at the layer that fixes it. `expand_conceal` cannot reach
        /// `occupied == 0` — `expand_inner` refuses when `anchor < hist_frames`, and
        /// with history alone that is `352 < 480` — so the codec was the only thing
        /// left, and on an uncompressed stream the codec has never been fed.
        #[test]
        fn concealment_should_repeat_the_last_frames_pitch_period() {
            let mut ts = TimeScaler::new();
            ts.remember(&tapered_tone_frame());
            let mut out = vec![0.0f32; OPUS_FRAME_SAMPLES];

            assert!(
                ts.conceal_frame(&mut out),
                "history is staged, so it must emit"
            );

            let peak = out.iter().fold(0.0f32, |m, s| m.max(s.abs()));
            assert!(
                peak > 0.01,
                "concealment emitted a peak of {peak} — the whole point is that \
                 this is audio and not the digital silence the codec path returns",
            );

            let period = period_used(&mut ts);
            assert!(
                (129..=352).contains(&period),
                "period {period} is outside the range the geometry allows",
            );
            // Same tolerance, and the same reason, as
            // `decimated_search_should_agree_with_an_exhaustive_full_rate_sweep`:
            // the taper biases the normalised correlation off the exact pitch
            // multiple by a sample (measured 239). A one-*period* slip would be a
            // real disagreement; a one-sample one is the material.
            assert!(
                period.abs_diff(240) <= 2,
                "a 200 Hz tone's period is 240 sample-frames, search returned {period}",
            );

            let ch = OPUS_CHANNELS as usize;
            for p in 0..(OPUS_FRAME_SAMPLES / ch - period) {
                for c in 0..ch {
                    assert_eq!(
                        out[p * ch + c],
                        out[(p + period) * ch + c],
                        "the emitted frame must be exactly {period}-periodic; it \
                         diverged at sample-frame {p}",
                    );
                }
            }
        }

        /// **The design claim, and the one that separates this from naive
        /// repetition.** Emitting `hist[n-P..n]` over and over re-opens a
        /// correlation-bounded seam at *every* period boundary — up to 3.7 of them
        /// per frame at the minimum period. Here the crossfade closes at `w = 1` on
        /// `hist[n-P-1]` and the next period opens on `hist[n-P]`, its immediate
        /// successor **in the source**, so the wrap carries the material's own step
        /// and nothing else.
        ///
        /// Both halves are asserted: the wrap step equals the source step exactly,
        /// *and* the naive alternative would have been materially worse on this
        /// material. Without the second half the first passes vacuously on anything
        /// smooth enough that the two agree.
        #[test]
        fn the_repeated_period_should_wrap_on_source_adjacent_samples() {
            let ch = OPUS_CHANNELS as usize;
            let hist = tapered_tone_frame();
            let mut ts = TimeScaler::new();
            ts.remember(&hist);
            let mut out = vec![0.0f32; OPUS_FRAME_SAMPLES];
            assert!(ts.conceal_frame(&mut out));

            let n = OPUS_FRAME_SAMPLES / ch;
            let period = period_used(&mut ts);
            let head = n - period;

            for c in 0..ch {
                let wrap_from = out[(period - 1) * ch + c];
                let wrap_to = out[period * ch + c];
                let src_prev = hist[(head - 1) * ch + c];
                let src_next = hist[head * ch + c];

                assert!(
                    (wrap_from - src_prev).abs() < 1e-6,
                    "the crossfade must close on hist[n-P-1] = {src_prev}, got {wrap_from}",
                );
                assert!(
                    (wrap_to - src_next).abs() < 1e-6,
                    "the next period must open on hist[n-P] = {src_next}, got {wrap_to}",
                );

                let ours = (wrap_to - wrap_from).abs();
                let source_step = (src_next - src_prev).abs();
                assert!(
                    (ours - source_step).abs() < 1e-6,
                    "the wrap must carry the source's own step {source_step}, got {ours}",
                );

                // Precondition on the comparison, not decoration: naive repetition
                // would join `hist[n-1]` to `hist[n-P]`.
                let naive = (src_next - hist[(n - 1) * ch + c]).abs();
                assert!(
                    naive > source_step * 4.0,
                    "this material cannot tell the two joins apart (naive {naive} \
                     vs source {source_step}) — the assertion above would pass \
                     whatever the wrap did",
                );
            }
        }

        /// The one hard join is the *entry*, and it has to be measured — but not
        /// through `note_splice_step`, whose terminal ramp weight is exactly 1.0 and
        /// therefore drives its residual to a structural zero for any argument
        /// choice. Reported as flawless regardless of the actual join is precisely
        /// the vacuous tripwire this repo has been bitten by before.
        ///
        /// It also must not land in `splice_step`, whose entire value is a **≤1.00
        /// ceiling** that says a non-monotonic fade came back. A hard join has no
        /// incoming signal to fade from and can legitimately exceed it.
        #[test]
        fn the_concealment_entry_seam_should_be_reported_in_its_own_field() {
            let mut ts = TimeScaler::new();
            ts.remember(&tapered_tone_frame());
            let mut out = vec![0.0f32; OPUS_FRAME_SAMPLES];
            assert!(ts.conceal_frame(&mut out));

            let step = ts
                .take_conceal_step()
                .expect("a tapered tone has slope to normalise against");
            assert!(
                step > 0.0,
                "the entry seam read {step} — a structural zero means the measurement \
                 is being taken at the crossfade handover weight, where `out_last` \
                 and `late_last` are the same number by construction",
            );
            assert!(
                step.is_finite(),
                "a non-finite reading would poison the window max",
            );
            assert!(
                ts.take_conceal_step().is_none(),
                "the reading must be taken, not read — a value left behind is \
                 re-counted into the next 1Hz window",
            );
            assert!(
                ts.take_splice_step().is_none(),
                "concealment must not write the fade-shape tripwire: its ≤1.00 \
                 ceiling is the only thing that distinguishes a monotonic ramp \
                 from the v18 bell, and a hard join can exceed 1.00 honestly",
            );
        }

        /// The history is replaced by the extension **un-muted**, so a second
        /// concealed callback continues the same period in phase instead of
        /// restarting it. NetEQ appends its expansion to `sync_buffer_` for exactly
        /// this reason.
        ///
        /// The continuation is not merely smooth, it is source-adjacent, for any
        /// period: callback 1's frame is exactly `P`-periodic, so its last sample
        /// `first[n-1]` is also `first[n-1-P]`, and callback 2 opens on
        /// `hist[n-P] = first[n-P]` — the very next sample. So the join carries one
        /// step of the original material and nothing else, which is what "in phase"
        /// has to mean to be assertable at all.
        #[test]
        fn a_second_concealed_callback_should_continue_the_first_in_phase() {
            let ch = OPUS_CHANNELS as usize;
            let mut ts = TimeScaler::new();
            ts.remember(&tapered_tone_frame());
            let mut first = vec![0.0f32; OPUS_FRAME_SAMPLES];
            assert!(ts.conceal_frame(&mut first));
            let period = period_used(&mut ts);

            let mut second = vec![0.0f32; OPUS_FRAME_SAMPLES];
            assert!(
                ts.conceal_frame(&mut second),
                "the extension must have been remembered, or a run of concealed \
                 callbacks re-enters from stale audio every time",
            );
            assert_eq!(
                period_used(&mut ts),
                period,
                "callback 2 must lock the same period — an exactly periodic history \
                 correlates 1.0 there, so a different answer means the extension was \
                 not what got staged",
            );

            let n = OPUS_FRAME_SAMPLES / ch;
            for c in 0..ch {
                assert!(
                    (first[(n - 1) * ch + c] - first[(n - 1 - period) * ch + c]).abs() < 1e-6,
                    "precondition: callback 1's frame must be {period}-periodic for \
                     the adjacency argument to hold",
                );
                let joined = (second[c] - first[(n - 1) * ch + c]).abs();
                let source_step =
                    (first[(n - period) * ch + c] - first[(n - 1 - period) * ch + c]).abs();
                assert!(
                    (joined - source_step).abs() < 1e-6,
                    "callback 2 opened {joined} away from callback 1's last sample, \
                     against the source's own step {source_step} — the phase was lost",
                );
            }
        }

        /// Both fallbacks, which are what make this change a no-op wherever it does
        /// not apply: no history staged (startup prebuffer, post-reset, after a
        /// `forget` on a discontinuity) means the caller stays on the codec path
        /// exactly as before, and `out` must come back untouched rather than
        /// half-written.
        #[test]
        fn concealment_should_decline_when_no_history_is_staged() {
            let mut ts = TimeScaler::new();
            let mut out = vec![7.0f32; OPUS_FRAME_SAMPLES];

            assert!(!ts.conceal_frame(&mut out), "a fresh scaler has no history");
            assert!(
                out.iter().all(|&s| s == 7.0),
                "a declined concealment must write nothing — the caller is about to \
                 decode into this buffer",
            );

            ts.remember(&tapered_tone_frame());
            assert!(
                ts.conceal_frame(&mut out),
                "precondition: staged history emits"
            );

            ts.forget();
            let mut out2 = vec![7.0f32; OPUS_FRAME_SAMPLES];
            assert!(
                !ts.conceal_frame(&mut out2),
                "`forget` is called on every discontinuity the manager knows about; \
                 concealing from dropped history would replay non-adjacent audio",
            );
            assert!(out2.iter().all(|&s| s == 7.0));
        }

        /// The output slice is the decoder's own buffer at the call site, so the
        /// length check is not theoretical — a short slice must decline rather than
        /// panic on the audio thread.
        #[test]
        fn concealment_should_decline_an_output_slice_too_short_for_a_frame() {
            let mut ts = TimeScaler::new();
            ts.remember(&tapered_tone_frame());
            let mut out = vec![0.0f32; OPUS_FRAME_SAMPLES - 1];
            assert!(!ts.conceal_frame(&mut out));
        }
    }
}
