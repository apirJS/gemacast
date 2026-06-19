#![cfg(target_os = "windows")]

//! Shared WASAPI utilities for format parsing and sample decoding.
//!
//! Used by both `wasapi_desktop` (Desktop capture) and `wasapi_loopback`
//! (Process capture) to avoid duplicating format negotiation and
//! sample conversion logic.

/// Parsed WASAPI mix format descriptor.
///
/// Extracted from `WAVEFORMATEX` / `WAVEFORMATEXTENSIBLE` via [`parse_mix_format`].
#[derive(Debug, Clone, Copy)]
pub struct WasapiFormat {
    pub native_rate: u32,
    pub native_channels: usize,
    pub bits_per_sample: u16,
    pub block_align: usize,
    pub is_float: bool,
}

/// IEEE Float sub-format GUID: `00000003-0000-0010-8000-00aa00389b71`
const KSDATAFORMAT_SUBTYPE_IEEE_FLOAT: windows::core::GUID =
    windows::core::GUID::from_u128(0x00000003_0000_0010_8000_00aa00389b71);

/// Parse a `WAVEFORMATEX` pointer into a [`WasapiFormat`].
///
/// Handles both `WAVE_FORMAT_EXTENSIBLE` (tag `0xFFFE`) and legacy format tags.
///
/// # Safety
///
/// `ptr` must be a valid, non-null pointer to a `WAVEFORMATEX` struct
/// allocated by `IAudioClient::GetMixFormat` (CoTaskMem).
pub unsafe fn parse_mix_format(
    ptr: *const windows::Win32::Media::Audio::WAVEFORMATEX,
) -> WasapiFormat {
    unsafe {
        let native_rate = (*ptr).nSamplesPerSec;
        let native_channels = (*ptr).nChannels as usize;
        let bits_per_sample = (*ptr).wBitsPerSample;
        let block_align = (*ptr).nBlockAlign as usize;

        let is_float = if (*ptr).wFormatTag == 0xFFFE {
            let ext = ptr as *const windows::Win32::Media::Audio::WAVEFORMATEXTENSIBLE;
            let sub_format = std::ptr::addr_of!((*ext).SubFormat).read_unaligned();
            sub_format == KSDATAFORMAT_SUBTYPE_IEEE_FLOAT
        } else {
            (*ptr).wFormatTag == 3 // WAVE_FORMAT_IEEE_FLOAT
        };

        WasapiFormat {
            native_rate,
            native_channels,
            bits_per_sample,
            block_align,
            is_float,
        }
    }
}

/// Decode raw WASAPI capture buffer bytes into f32 samples.
///
/// Supports IEEE Float 32-bit, PCM 16-bit, PCM 24-bit packed, and PCM 32-bit.
/// Unknown formats produce silence.
///
/// # Safety
///
/// `buffer_ptr` must be a valid pointer to at least `num_frames * format.block_align` bytes,
/// as returned by `IAudioCaptureClient::GetBuffer`.
pub unsafe fn decode_samples_to_f32(
    buffer_ptr: *const u8,
    format: &WasapiFormat,
    num_frames: usize,
    output: &mut Vec<f32>,
) {
    unsafe {
        let total_samples = num_frames * format.native_channels;
        output.clear();
        output.reserve(total_samples);

        if format.is_float && format.bits_per_sample == 32 {
            let float_ptr = buffer_ptr as *const f32;
            let float_slice = std::slice::from_raw_parts(float_ptr, total_samples);
            output.extend_from_slice(float_slice);
        } else if !format.is_float && format.bits_per_sample == 16 {
            let i16_ptr = buffer_ptr as *const i16;
            let i16_slice = std::slice::from_raw_parts(i16_ptr, total_samples);
            for &s in i16_slice {
                output.push(s as f32 / 32768.0);
            }
        } else if !format.is_float && format.bits_per_sample == 24 {
            let raw_bytes = std::slice::from_raw_parts(buffer_ptr, num_frames * format.block_align);
            let bytes_per_chunk = format.block_align / format.native_channels;
            for i in 0..total_samples {
                let offset = (i / format.native_channels) * format.block_align
                    + (i % format.native_channels) * bytes_per_chunk;

                if bytes_per_chunk == 3 {
                    if offset + 2 < raw_bytes.len() {
                        let bytes = [
                            0,
                            raw_bytes[offset],
                            raw_bytes[offset + 1],
                            raw_bytes[offset + 2],
                        ];
                        let val = i32::from_le_bytes(bytes);
                        output.push(val as f32 / 2147483648.0);
                    } else {
                        output.push(0.0);
                    }
                } else if bytes_per_chunk == 4 {
                    if offset + 3 < raw_bytes.len() {
                        let bytes = [
                            raw_bytes[offset],
                            raw_bytes[offset + 1],
                            raw_bytes[offset + 2],
                            raw_bytes[offset + 3],
                        ];
                        let val = i32::from_le_bytes(bytes);
                        output.push(val as f32 / 2147483648.0);
                    } else {
                        output.push(0.0);
                    }
                } else {
                    output.push(0.0);
                }
            }
        } else if !format.is_float && format.bits_per_sample == 32 {
            let i32_ptr = buffer_ptr as *const i32;
            let i32_slice = std::slice::from_raw_parts(i32_ptr, total_samples);
            for &s in i32_slice {
                output.push(s as f32 / 2147483648.0);
            }
        } else {
            // Unknown format — push silence
            output.resize(total_samples, 0.0);
        }
    }
}

/// Downmix multi-channel audio to stereo (interleaved L/R pairs).
///
/// - Mono (1ch): duplicates to both channels.
/// - Stereo (2ch): passthrough (copies directly).
/// - >2ch: takes the first two channels, discards the rest.
pub fn downmix_to_stereo(input: &[f32], channels: usize, output: &mut Vec<f32>) {
    output.clear();
    let frames = input.len() / channels;

    match channels {
        1 => {
            output.reserve(frames * 2);
            for &s in input.iter().take(frames) {
                output.push(s);
                output.push(s);
            }
        }
        2 => {
            output.reserve(input.len());
            output.extend_from_slice(input);
        }
        _ => {
            output.reserve(frames * 2);
            for frame in input.chunks_exact(channels) {
                // FL (0), FR (1), C (2)
                let center = frame.get(2).copied().unwrap_or(0.0) * 0.707;

                let mut left = frame[0] + center;
                let mut right = frame[1] + center;

                // LFE (3)
                if channels >= 4 {
                    let lfe = frame[3] * 0.3;
                    left += lfe;
                    right += lfe;
                }

                // RL (4), RR (5)
                if channels >= 6 {
                    left += frame[4] * 0.707;
                    right += frame[5] * 0.707;
                }

                // SL (6), SR (7)
                if channels >= 8 {
                    left += frame[6] * 0.707;
                    right += frame[7] * 0.707;
                }

                // Prevent clipping
                let norm = if channels >= 8 {
                    1.0 + 0.707 + 0.3 + 0.707 + 0.707
                } else if channels >= 6 {
                    1.0 + 0.707 + 0.3 + 0.707
                } else if channels >= 4 {
                    1.0 + 0.707 + 0.3
                } else {
                    1.0 + 0.707
                };

                output.push(left / norm);
                output.push(right / norm);
            }
        }
    }
}
