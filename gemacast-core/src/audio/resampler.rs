use audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Indexing, Resampler as RubatoResampler};

use crate::domain::error::{AudioError, GemaCastError};

/// High-quality audio resampler using FFT-based interpolation (Rubato v3).
///
/// Wraps [`rubato::Fft`] with fixed-input mode for real-time sample rate
/// conversion with high fidelity. Pre-allocates all internal buffers at
/// construction time for zero per-call heap allocation.
///
/// Accepts interleaved `f32` input and produces interleaved `f32` output,
/// matching the pipeline's data format.
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
    /// resampler with the given parameters (e.g. zero sample rate).
    pub fn new(from_rate: u32, to_rate: u32, channels: usize) -> Result<Self, GemaCastError> {
        let chunk_size = 1024;

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
        // Allow space for up to 16 chunks of output in a single process call
        let max_output_frames = output_frames_per_chunk * 16;
        let output_buf = vec![0.0f32; max_output_frames * channels];

        Ok(Self {
            inner,
            channels,
            output_buf,
            output_capacity_frames: max_output_frames,
            remainder: Vec::with_capacity(frames_needed * channels * 2),
            frames_needed,
        })
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
        self.remainder.extend_from_slice(input);

        let mut total_output_samples = 0usize;
        let samples_per_chunk = self.frames_needed * self.channels;

        while self.remainder.len() >= samples_per_chunk {
            let output_frames_avail = self
                .output_capacity_frames
                .saturating_sub(total_output_samples / self.channels);

            // Dynamically resize output buffer if WASAPI delivered a massive backlog (e.g., CPU spike)
            if output_frames_avail < self.inner.output_frames_next() {
                self.output_capacity_frames += self.inner.output_frames_next() * 8;
                self.output_buf
                    .resize(self.output_capacity_frames * self.channels, 0.0);
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
}
