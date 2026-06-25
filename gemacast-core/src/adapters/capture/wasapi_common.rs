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

// ---------------------------------------------------------------------------
// Shared async activation helpers
// ---------------------------------------------------------------------------

use crate::domain::error::{AudioError, GemaCastError};
use windows::Win32::Media::Audio::PROCESS_LOOPBACK_MODE;
use windows::{
    Win32::{
        Media::Audio::{
            AUDIOCLIENT_ACTIVATION_PARAMS, AUDIOCLIENT_ACTIVATION_PARAMS_0,
            AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK, AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS,
            ActivateAudioInterfaceAsync, IActivateAudioInterfaceAsyncOperation,
            IActivateAudioInterfaceCompletionHandler,
            IActivateAudioInterfaceCompletionHandler_Impl, IAudioClient,
            VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
        },
        System::{
            Com::{COINIT_MULTITHREADED, CoInitializeEx, StructuredStorage::PROPVARIANT},
            Variant::VT_BLOB,
        },
    },
    core::{ComInterface, IUnknown, PCWSTR, implement},
};

/// Completion handler for `ActivateAudioInterfaceAsync`.
///
/// Receives the activated `IAudioClient` (or error) and sends it back
/// to the calling thread via a `std::sync::mpsc` channel.
#[implement(IActivateAudioInterfaceCompletionHandler)]
pub(crate) struct AudioActivator {
    pub sender: std::sync::mpsc::Sender<Result<IAudioClient, GemaCastError>>,
}

impl IActivateAudioInterfaceCompletionHandler_Impl for AudioActivator {
    fn ActivateCompleted(
        &self,
        activateoperation: Option<&IActivateAudioInterfaceAsyncOperation>,
    ) -> windows::core::Result<()> {
        let get_client = || -> windows::core::Result<IAudioClient> {
            let op = activateoperation
                .ok_or_else(|| windows::core::Error::from(windows::Win32::Foundation::E_POINTER))?;

            let mut status = windows::core::HRESULT(0);
            let mut unknown: Option<IUnknown> = None;

            unsafe {
                op.GetActivateResult(&mut status, &mut unknown)?;
            }

            status.ok()?;

            let unknown = unknown
                .ok_or_else(|| windows::core::Error::from(windows::Win32::Foundation::E_POINTER))?;

            unknown.cast::<IAudioClient>()
        };

        let payload = get_client().map_err(|e| AudioError::WindowsApi(e).into());
        let _ = self.sender.send(payload);

        Ok(())
    }
}

/// Activate a process loopback `IAudioClient` via `ActivateAudioInterfaceAsync`.
///
/// # Arguments
/// - `pid`: The target process ID.
/// - `mode`: `INCLUDE` to capture only the target tree, `EXCLUDE` to capture everything except it.
///
/// # Safety
///
/// Calls COM interfaces. COM must be initialized on the calling thread.
pub unsafe fn activate_process_loopback(
    pid: u32,
    mode: PROCESS_LOOPBACK_MODE,
) -> Result<IAudioClient, GemaCastError> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    };

    let loopback_params = AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
        ProcessLoopbackMode: mode,
        TargetProcessId: pid,
    };

    let mut activation_params = AUDIOCLIENT_ACTIVATION_PARAMS {
        ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
        Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 {
            ProcessLoopbackParams: loopback_params,
        },
    };

    let mut prop_variant = PROPVARIANT::default();
    unsafe {
        (*prop_variant.Anonymous.Anonymous).vt = VT_BLOB;
        (*prop_variant.Anonymous.Anonymous).Anonymous.blob.cbSize =
            std::mem::size_of::<AUDIOCLIENT_ACTIVATION_PARAMS>() as u32;
        (*prop_variant.Anonymous.Anonymous).Anonymous.blob.pBlobData =
            &mut activation_params as *mut _ as *mut u8;
    };

    let (sender, receiver) = std::sync::mpsc::channel();
    let activator: IActivateAudioInterfaceCompletionHandler = AudioActivator { sender }.into();

    unsafe {
        ActivateAudioInterfaceAsync(
            PCWSTR::from_raw(VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK.as_ptr()),
            &IAudioClient::IID,
            Some(&prop_variant),
            Some(&activator),
        )
        .map_err(AudioError::WindowsApi)?;
    };

    let result = receiver.recv().unwrap_or_else(|_| {
        Err(AudioError::WindowsApi(windows::core::Error::from(
            windows::Win32::Foundation::E_FAIL,
        ))
        .into())
    })?;

    Ok(result)
}

/// Query the default render endpoint's mix format.
///
/// Process loopback streams use the same shared-mode format as the system mixer,
/// so this gives us the correct format to pass to `IAudioClient::Initialize`.
///
/// # Safety
///
/// Calls COM interfaces. The returned pointer is CoTaskMem-allocated and must
/// be freed by the caller via `CoTaskMemFree`.
pub unsafe fn get_default_mix_format()
-> Result<*mut windows::Win32::Media::Audio::WAVEFORMATEX, GemaCastError> {
    use windows::Win32::Media::Audio::{
        IAudioClient, IMMDeviceEnumerator, MMDeviceEnumerator, eConsole, eRender,
    };
    use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance};

    unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                .map_err(AudioError::WindowsApi)?;

        let device = enumerator
            .GetDefaultAudioEndpoint(eRender, eConsole)
            .map_err(AudioError::WindowsApi)?;

        let audio_client: IAudioClient = device
            .Activate(CLSCTX_ALL, None)
            .map_err(AudioError::WindowsApi)?;

        let mix_format_ptr = audio_client
            .GetMixFormat()
            .map_err(AudioError::WindowsApi)?;

        Ok(mix_format_ptr)
    }
}
