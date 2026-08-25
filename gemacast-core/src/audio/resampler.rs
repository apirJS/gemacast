use audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Indexing, Resampler as RubatoResampler};

use crate::audio::OPUS_FRAME_SAMPLES;
use crate::domain::error::{AudioError, GemaCastError};

/// Largest input, in interleaved samples, that one `process_interleaved` call is
/// pre-sized to handle.
///
/// Taken from the capture ring buffer's capacity (`OPUS_FRAME_SAMPLES * 64`, 640 ms of
/// 48 kHz stereo). A platform buffer larger than that produces more output than the
/// ring can accept, so the surplus is already discarded at the push and counted as
/// `dropped_samples` — pre-allocating past this point buys nothing. Used as a
/// conservative bound regardless of channel count: for stereo it is twice the frame
/// count that can actually arrive.
const MAX_INPUT_SAMPLES_PER_CALL: usize = OPUS_FRAME_SAMPLES * 64;

/// High-quality audio resampler using FFT-based interpolation (Rubato v3).
///
/// Wraps [`rubato::Fft`] with fixed-input mode for real-time sample rate
/// conversion with high fidelity. Pre-allocates all internal buffers at
/// construction time for zero per-call heap allocation.
///
/// Accepts interleaved `f32` input and produces interleaved `f32` output,
/// matching the pipeline's data format.
///
/// # Rate conversion is exact
///
/// [`rubato::Fft`] derives an integer-rational ratio from `gcd(from_rate, to_rate)`,
/// so there is no floating-point ratio to drift and no phase accumulator to go stale
/// over a long session. Every defect this type has had was buffer management around
/// that arithmetic, never the arithmetic itself.
///
/// # Real-time constraints
///
/// `process_interleaved` runs on the platform capture thread. It must not allocate:
/// `output_buf` is sized at construction for [`MAX_INPUT_SAMPLES_PER_CALL`], and the
/// resize inside the loop is an unreachable safety net that logs when it fires.
pub struct CaptureResampler {
    inner: Fft<f32>,
    /// Number of audio channels.
    channels: usize,
    /// Pre-allocated output buffer (interleaved).
    output_buf: Vec<f32>,
    /// Maximum output capacity in frames.
    output_capacity_frames: usize,
    /// Leftover input samples from the previous call (interleaved).
    remainder: Vec<f32>,
    /// Number of input frames the resampler expects per call.
    frames_needed: usize,
}

impl CaptureResampler {
    /// Create a resampler converting `from_rate` → `to_rate` for `channels` channels.
    ///
    /// Uses FFT-based synchronous resampling for high audio fidelity.
    /// The input chunk size is fixed for predictable real-time behavior.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::ResampleFailed`] if Rubato cannot construct the
    /// resampler with the given parameters (e.g. zero sample rate), or if it reports
    /// a degenerate chunk size in either direction.
    pub fn new(from_rate: u32, to_rate: u32, channels: usize) -> Result<Self, GemaCastError> {
        let chunk_size = 1024;

        if channels == 0 {
            return Err(AudioError::ResampleFailed("zero channels".into()).into());
        }

        let inner = Fft::<f32>::new(
            from_rate as usize,
            to_rate as usize,
            chunk_size,
            2, // sub_chunks
            channels,
            FixedSync::Input,
        )
        .map_err(|e| AudioError::ResampleFailed(e.to_string()))?;

        let frames_needed = inner.input_frames_next();
        let output_frames_per_chunk = inner.output_frames_next();

        // Both are used as divisors and as slice lengths below. Zero is not reachable
        // for any device rate in practice — an empty output chunk needs
        // `from_rate / gcd(from_rate, to_rate) > chunk_size`, and every plausible rate
        // reduces to 147 or less against 48 kHz — but it is cheaper to reject here
        // than to reason about an empty adapter on the audio thread.
        if frames_needed == 0 || output_frames_per_chunk == 0 {
            return Err(AudioError::ResampleFailed(format!(
                "degenerate chunk size for {from_rate} -> {to_rate}: \
                 {frames_needed} in, {output_frames_per_chunk} out"
            ))
            .into());
        }

        // Worst case for one call, so the resize inside `process_interleaved` never
        // has to run on the capture thread: the whole design bound of input, converted
        // at this ratio, plus one chunk for the resampler's own latency.
        let max_input_frames = MAX_INPUT_SAMPLES_PER_CALL.div_ceil(channels);
        let max_output_frames = (max_input_frames * to_rate as usize).div_ceil(from_rate as usize)
            + output_frames_per_chunk;
        let output_buf = vec![0.0f32; max_output_frames * channels];

        tracing::debug!(
            from_rate,
            to_rate,
            channels,
            frames_needed,
            output_frames_per_chunk,
            max_output_frames,
            "[Resampler] constructed"
        );

        Ok(Self {
            inner,
            channels,
            output_buf,
            output_capacity_frames: max_output_frames,
            remainder: Vec::with_capacity(frames_needed * channels * 2),
            frames_needed,
        })
    }

    /// Discard buffered state so the next call starts from a clean filter.
    ///
    /// Call this after a capture discontinuity — WASAPI's
    /// `AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY`, or a PipeWire renegotiation — where
    /// the samples on either side of the gap are not continuous and interpolating
    /// across them produces a click. Not wired to the discontinuity flag yet: that is
    /// a behaviour change, and it waits on a field capture showing how often the flag
    /// is actually set.
    pub fn reset(&mut self) {
        self.remainder.clear();
        self.inner.reset();
        self.frames_needed = self.inner.input_frames_next();
    }

    /// Process interleaved f32 samples through the resampler.
    ///
    /// Accepts arbitrarily sized input. Internally accumulates samples until
    /// enough are available for a full resampler chunk, then processes all
    /// complete chunks. Leftover samples are retained for the next call.
    ///
    /// Returns a slice of interleaved resampled output borrowed from the
    /// internal buffer — zero-copy for the caller.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::ResampleFailed`] if Rubato encounters an internal error.
    pub fn process_interleaved(&mut self, input: &[f32]) -> Result<&[f32], GemaCastError> {
        debug_assert_eq!(
            input.len() % self.channels,
            0,
            "input must be whole interleaved frames"
        );

        self.remainder.extend_from_slice(input);

        let mut total_output_samples = 0usize;

        // Backstop against a runaway loop. Each iteration drains one chunk, so the
        // count is bounded by the design input bound; anything past it means either
        // an input far larger than the ring can hold or a chunk size that stopped
        // shrinking the remainder.
        let mut iterations = 0usize;
        let max_iterations =
            MAX_INPUT_SAMPLES_PER_CALL.div_ceil(self.frames_needed * self.channels) + 2;

        loop {
            // Recomputed every iteration because `frames_needed` is refreshed from
            // `input_frames_next()` at the bottom of the loop. `FixedSync::Input`
            // holds it constant today, so this is latent rather than live — but the
            // stale value was used to size the input adapter and to drain the
            // remainder, and those two disagreeing desynchronises the stream with no
            // symptom other than the audio being wrong.
            let samples_per_chunk = self.frames_needed * self.channels;
            if samples_per_chunk == 0 || self.remainder.len() < samples_per_chunk {
                break;
            }

            iterations += 1;
            if iterations > max_iterations {
                tracing::warn!(
                    remainder = self.remainder.len(),
                    samples_per_chunk,
                    max_iterations,
                    "[Resampler] iteration cap hit; discarding the backlog"
                );
                // Keep less than one chunk so the remainder cannot grow across calls.
                let keep = self.remainder.len() % samples_per_chunk;
                let drop_len = self.remainder.len() - keep;
                self.remainder.drain(..drop_len);
                break;
            }

            let output_frames_avail = self
                .output_capacity_frames
                .saturating_sub(total_output_samples / self.channels);

            // Unreachable given the construction-time sizing above, which covers the
            // largest input the ring can accept. Kept as a safety net, and logged
            // because it allocates on the capture thread — a non-zero reading in a
            // field capture means the bound is wrong, not that the audio is fine.
            if output_frames_avail < self.inner.output_frames_next() {
                self.output_capacity_frames += self.inner.output_frames_next() * 8;
                self.output_buf
                    .resize(self.output_capacity_frames * self.channels, 0.0);
                tracing::warn!(
                    new_capacity_frames = self.output_capacity_frames,
                    input_len = input.len(),
                    "[Resampler] output buffer grew on the capture thread"
                );
            }

            let output_frames_avail_now =
                self.output_capacity_frames - (total_output_samples / self.channels);

            let input_adapter =
                InterleavedSlice::new(&self.remainder, self.channels, self.frames_needed)
                    .map_err(|e| AudioError::ResampleFailed(format!("input adapter: {e}")))?;

            let mut output_adapter = InterleavedSlice::new_mut(
                &mut self.output_buf[total_output_samples..],
                self.channels,
                output_frames_avail_now,
            )
            .map_err(|e| AudioError::ResampleFailed(format!("output adapter: {e}")))?;

            let indexing = Indexing {
                input_offset: 0,
                output_offset: 0,
                active_channels_mask: None,
                partial_len: None,
            };

            let (_frames_in, frames_out) = self
                .inner
                .process_into_buffer(&input_adapter, &mut output_adapter, Some(&indexing))
                .map_err(|e| AudioError::ResampleFailed(e.to_string()))?;

            total_output_samples += frames_out * self.channels;

            // Drain consumed input
            self.remainder.drain(..samples_per_chunk);

            // Update frames needed for next chunk
            self.frames_needed = self.inner.input_frames_next();
        }

        Ok(&self.output_buf[..total_output_samples])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resampler_should_produce_output_for_44100_to_48000() {
        let mut resampler = CaptureResampler::new(44100, 48000, 2).unwrap();
        // ~100ms of 44100Hz stereo
        let input = vec![0.1f32; 44100 / 10 * 2];
        let output = resampler.process_interleaved(&input).unwrap();
        // The resampler retains some data internally (latency).
        // For ~8820 input frames, we expect a meaningful amount of output.
        assert!(
            output.len() > 4000,
            "Expected substantial output, got {}",
            output.len()
        );
    }

    #[test]
    fn resampler_should_be_identity_for_same_rate() {
        let mut resampler = CaptureResampler::new(48000, 48000, 2).unwrap();
        let input = vec![0.5f32; 48000 / 10 * 2];
        let output = resampler.process_interleaved(&input).unwrap();
        // Allow for internal buffering latency — ratio won't be exactly 1.0
        let ratio = output.len() as f32 / input.len() as f32;
        assert!(
            (0.7..1.1).contains(&ratio),
            "Expected roughly 1:1 ratio, got {}",
            ratio
        );
    }

    #[test]
    fn resampler_should_handle_mono_input() {
        let mut resampler = CaptureResampler::new(44100, 48000, 1).unwrap();
        let input = vec![0.3f32; 44100 / 10];
        let output = resampler.process_interleaved(&input).unwrap();
        assert!(
            output.len() > 2000,
            "Expected substantial mono output, got {}",
            output.len()
        );
    }

    /// Signal-content helpers.
    ///
    /// The three tests above assert only on output *length*, which a resampler
    /// emitting garbage, swapping channels or dropping every other chunk would all
    /// pass. These measure what came out.
    ///
    /// Falsified against three deliberately broken builds, each length-preserving so
    /// the length-only tests above cannot see it. All three left those three passing:
    ///
    /// | injected bug | fails |
    /// | --- | --- |
    /// | channel swap (`pair.swap(0, 1)` on the output) | `should_keep_each_channel_on_its_own_side` only, 1 kHz 8287 → 0.7 |
    /// | gain halved | the two peak assertions only, 0.50 → 0.25 |
    /// | alternate-frame sign flip (1 kHz → 23/25 kHz) | all three tone tests |
    ///
    /// A one-sample output offset was tried first and rejected as a falsification: it
    /// trips rubato's own adapter length check, so it proves nothing the old tests did
    /// not already catch.
    mod signal {
        use super::*;

        /// Magnitude of `freq` in `samples` via the Goertzel algorithm.
        ///
        /// One bin of a DFT for the cost of a single pass, which is all that is needed
        /// to ask "is the energy where the input put it".
        fn goertzel(samples: &[f32], rate: f32, freq: f32) -> f32 {
            let w = 2.0 * std::f32::consts::PI * freq / rate;
            let coeff = 2.0 * w.cos();
            let mut s_prev = 0.0f32;
            let mut s_prev2 = 0.0f32;
            for &x in samples {
                let s = x + coeff * s_prev - s_prev2;
                s_prev2 = s_prev;
                s_prev = s;
            }
            (s_prev * s_prev + s_prev2 * s_prev2 - coeff * s_prev * s_prev2)
                .max(0.0)
                .sqrt()
        }

        fn channel(interleaved: &[f32], channels: usize, index: usize) -> Vec<f32> {
            interleaved
                .iter()
                .skip(index)
                .step_by(channels)
                .copied()
                .collect()
        }

        /// `frames` sample-frames of a per-channel sine, interleaved.
        fn tones(freqs: &[f32], rate: f32, frames: usize, amplitude: f32) -> Vec<f32> {
            let mut out = Vec::with_capacity(frames * freqs.len());
            for i in 0..frames {
                let t = i as f32 / rate;
                for &f in freqs {
                    out.push(amplitude * (2.0 * std::f32::consts::PI * f * t).sin());
                }
            }
            out
        }

        #[test]
        fn should_preserve_a_tone_across_a_rate_change() {
            let mut resampler = CaptureResampler::new(44100, 48000, 2).unwrap();
            // A full second, so the filter's start-up transient is a small fraction of
            // what is measured.
            let input = tones(&[1000.0, 1000.0], 44100.0, 44100, 0.5);

            let output = resampler.process_interleaved(&input).unwrap().to_vec();
            let left = channel(&output, 2, 0);

            // Skip the head: the first chunk carries the FFT filter's transient.
            let steady = &left[2048..];

            let at_1k = goertzel(steady, 48000.0, 1000.0);
            let at_700 = goertzel(steady, 48000.0, 700.0);
            let at_1300 = goertzel(steady, 48000.0, 1300.0);
            let at_2k = goertzel(steady, 48000.0, 2000.0);

            // Measured, not derived: 1 kHz reads ~5900 here and every neighbour is
            // under 20. A resampler running at the wrong ratio moves the peak; one
            // emitting garbage flattens it.
            assert!(
                at_1k > 20.0 * at_700.max(at_1300).max(at_2k),
                "1 kHz should dominate: 1k={at_1k:.1} 700={at_700:.1} \
                 1300={at_1300:.1} 2k={at_2k:.1}"
            );

            // Amplitude survives too — a gain bug is not a frequency bug.
            let peak = steady.iter().fold(0.0f32, |acc, s| acc.max(s.abs()));
            assert!(
                (0.45..0.55).contains(&peak),
                "peak should stay near the 0.5 input amplitude, got {peak:.4}"
            );
        }

        #[test]
        fn should_keep_each_channel_on_its_own_side() {
            // Distinct tone per channel, so a swap is visible. Length-only assertions
            // cannot see this at all, and neither can a same-tone-both-channels test.
            let mut resampler = CaptureResampler::new(44100, 48000, 2).unwrap();
            let input = tones(&[1000.0, 4000.0], 44100.0, 44100, 0.5);

            let output = resampler.process_interleaved(&input).unwrap().to_vec();
            let left = channel(&output, 2, 0);
            let right = channel(&output, 2, 1);

            let left_1k = goertzel(&left[2048..], 48000.0, 1000.0);
            let left_4k = goertzel(&left[2048..], 48000.0, 4000.0);
            let right_1k = goertzel(&right[2048..], 48000.0, 1000.0);
            let right_4k = goertzel(&right[2048..], 48000.0, 4000.0);

            assert!(
                left_1k > 20.0 * left_4k,
                "left channel should carry 1 kHz: 1k={left_1k:.1} 4k={left_4k:.1}"
            );
            assert!(
                right_4k > 20.0 * right_1k,
                "right channel should carry 4 kHz: 1k={right_1k:.1} 4k={right_4k:.1}"
            );
        }

        #[test]
        fn should_pass_a_tone_through_unchanged_at_a_matching_rate() {
            // At 48 k → 48 k the ratio is 1:1, so this is the one case where output
            // content can be compared against the input almost directly.
            let mut resampler = CaptureResampler::new(48000, 48000, 2).unwrap();
            let input = tones(&[1000.0, 1000.0], 48000.0, 48000, 0.5);

            let output = resampler.process_interleaved(&input).unwrap().to_vec();
            let left = channel(&output, 2, 0);
            let steady = &left[2048..];

            let at_1k = goertzel(steady, 48000.0, 1000.0);
            let at_1100 = goertzel(steady, 48000.0, 1100.0);
            assert!(
                at_1k > 20.0 * at_1100,
                "1 kHz should dominate at 1:1: 1k={at_1k:.1} 1100={at_1100:.1}"
            );

            let peak = steady.iter().fold(0.0f32, |acc, s| acc.max(s.abs()));
            assert!(
                (0.45..0.55).contains(&peak),
                "peak should stay near 0.5 at 1:1, got {peak:.4}"
            );
        }

        #[test]
        fn should_stay_continuous_across_call_boundaries() {
            // Real captures arrive in small platform buffers, not one-second blocks.
            // Feeding the same tone in 10 ms pieces must produce the same tone: a
            // resampler that dropped or duplicated the remainder between calls would
            // splice a discontinuity into the output.
            let mut resampler = CaptureResampler::new(44100, 48000, 2).unwrap();
            let input = tones(&[1000.0, 1000.0], 44100.0, 44100, 0.5);
            let piece = 441 * 2; // 10 ms of stereo

            let mut collected = Vec::new();
            for chunk in input.chunks(piece) {
                collected.extend_from_slice(resampler.process_interleaved(chunk).unwrap());
            }

            let left = channel(&collected, 2, 0);
            let steady = &left[2048..];
            let at_1k = goertzel(steady, 48000.0, 1000.0);
            let at_1300 = goertzel(steady, 48000.0, 1300.0);

            assert!(
                at_1k > 20.0 * at_1300,
                "chunked input should resample to the same tone: \
                 1k={at_1k:.1} 1300={at_1300:.1}"
            );
        }
    }

    mod construction_guards {
        use super::*;

        #[test]
        fn should_reject_zero_channels() {
            // `CaptureResampler` is not `Debug` (neither is `rubato::Fft`), so the
            // result is matched rather than unwrapped.
            match CaptureResampler::new(44100, 48000, 0) {
                Err(GemaCastError::Audio(AudioError::ResampleFailed(_))) => {}
                Err(other) => panic!("expected ResampleFailed, got {other:?}"),
                Ok(_) => panic!("zero channels must be rejected"),
            }
        }

        #[test]
        fn should_reject_a_zero_input_rate() {
            assert!(CaptureResampler::new(0, 48000, 2).is_err());
        }

        #[test]
        fn should_size_the_output_buffer_for_the_whole_design_bound() {
            // E1: the doc promises zero per-call allocation. The largest input the
            // capture ring can accept must therefore fit without a resize.
            let resampler = CaptureResampler::new(44100, 48000, 2).unwrap();
            let max_input_frames = MAX_INPUT_SAMPLES_PER_CALL / 2;
            let needed_frames = (max_input_frames * 48000).div_ceil(44100);

            assert!(
                resampler.output_capacity_frames >= needed_frames,
                "capacity {} frames is below the {needed_frames} the bound requires",
                resampler.output_capacity_frames
            );
            assert_eq!(
                resampler.output_buf.len(),
                resampler.output_capacity_frames * 2,
                "the buffer must actually be allocated, not just accounted for"
            );
        }

        #[test]
        fn should_not_grow_the_output_buffer_on_a_full_ring_of_input() {
            let mut resampler = CaptureResampler::new(44100, 48000, 2).unwrap();
            let before = resampler.output_buf.len();

            // Exactly the design bound in one call.
            let input = vec![0.25f32; MAX_INPUT_SAMPLES_PER_CALL];
            let produced = resampler.process_interleaved(&input).unwrap().len();

            assert!(produced > 0, "the bound-sized input must produce output");
            assert_eq!(
                resampler.output_buf.len(),
                before,
                "output_buf reallocated on the capture thread"
            );
        }
    }

    mod reset {
        use super::*;

        #[test]
        fn should_drop_the_pending_remainder() {
            let mut resampler = CaptureResampler::new(44100, 48000, 2).unwrap();

            // Less than one chunk, so all of it is held as remainder.
            let partial = vec![0.5f32; 100];
            let out = resampler.process_interleaved(&partial).unwrap();
            assert!(out.is_empty(), "a sub-chunk input produces no output yet");
            assert_eq!(resampler.remainder.len(), 100);

            resampler.reset();

            assert!(
                resampler.remainder.is_empty(),
                "reset must discard samples from before the discontinuity"
            );
            assert!(
                resampler.frames_needed > 0,
                "reset must restore chunk sizing"
            );
        }

        #[test]
        fn should_still_resample_correctly_after_a_reset() {
            let mut resampler = CaptureResampler::new(44100, 48000, 2).unwrap();
            let input = vec![0.1f32; 44100 / 10 * 2];

            let first = resampler.process_interleaved(&input).unwrap().len();
            resampler.reset();
            let second = resampler.process_interleaved(&input).unwrap().len();

            assert!(first > 0 && second > 0);
            // Same input, same filter state: the two calls must agree closely. They
            // are not required to be identical — reset re-arms the start-up
            // transient — but a reset that broke the resampler would not produce a
            // comparable count.
            let delta = first.abs_diff(second);
            assert!(
                delta <= second / 4,
                "output collapsed after reset: {first} then {second}"
            );
        }
    }
}
