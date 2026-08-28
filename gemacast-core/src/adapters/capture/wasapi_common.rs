#![cfg(target_os = "windows")]

//! Shared WASAPI utilities for format parsing and sample decoding.
//!
//! Used by both `wasapi_desktop` (Desktop capture) and `wasapi_loopback`
//! (Process capture) to avoid duplicating format negotiation and
//! sample conversion logic.

use crate::domain::error::{AudioError, GemaCastError};
use crate::ports::capture::CaptureCounters;

/// Parsed WASAPI mix format descriptor.
///
/// Extracted from `WAVEFORMATEX` / `WAVEFORMATEXTENSIBLE` via [`parse_mix_format`].
///
/// Every field here is used downstream as a length, a divisor, or a pointer cast, so
/// [`parse_mix_format`] validates them all before this struct exists. A
/// `WasapiFormat` value can be trusted to have `native_channels > 0`,
/// `bits_per_sample > 0`, and `block_align == native_channels * bits_per_sample / 8`.
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

/// Size in bytes of the extension `WAVEFORMATEXTENSIBLE` adds after `WAVEFORMATEX`:
/// `wValidBitsPerSample` (2) + `dwChannelMask` (4) + `SubFormat` (16).
///
/// `cbSize` counts exactly those trailing bytes, and the `CoTaskMemFree`-able
/// allocation `GetMixFormat` returns is only `sizeof(WAVEFORMATEX) + cbSize` long.
/// Reading `SubFormat` without checking this first is a heap over-read of up to 22
/// bytes, which is why the check below is not cosmetic.
const WAVEFORMATEXTENSIBLE_EXTENSION_SIZE: u16 = 22;

/// Parse a `WAVEFORMATEX` pointer into a [`WasapiFormat`].
///
/// Handles both `WAVE_FORMAT_EXTENSIBLE` (tag `0xFFFE`) and legacy format tags.
///
/// Rejects a descriptor the rest of the pipeline cannot use rather than passing it
/// on. Three of the four values here reach arithmetic that would panic or over-read on
/// a degenerate input: `native_channels` is a divisor in both `downmix_to_stereo` and
/// the 24-bit decode branch, and `block_align` bounds the slice that branch reads.
///
/// # Errors
///
/// * [`AudioError::UnsupportedCaptureFormat`] if `nChannels` is 0.
/// * [`AudioError::CaptureInstanceFailed`] if `wBitsPerSample` or `nBlockAlign` is 0.
///
/// A `nBlockAlign` that merely *disagrees* with `nChannels * wBitsPerSample / 8` is
/// not an error: it is corrected to the computed value and logged. The WAVEFORMATEX
/// contract requires them to agree for PCM and float, the only families this decodes,
/// so a mismatch means the descriptor is wrong rather than that the device is unusual.
///
/// # Safety
///
/// `ptr` must be a valid, non-null pointer to a `WAVEFORMATEX` struct
/// allocated by `IAudioClient::GetMixFormat` (CoTaskMem), and the allocation must be
/// at least `size_of::<WAVEFORMATEX>() + (*ptr).cbSize` bytes long.
pub unsafe fn parse_mix_format(
    ptr: *const windows::Win32::Media::Audio::WAVEFORMATEX,
) -> Result<WasapiFormat, GemaCastError> {
    unsafe {
        let native_rate = (*ptr).nSamplesPerSec;
        let native_channels = (*ptr).nChannels as usize;
        let bits_per_sample = (*ptr).wBitsPerSample;
        let reported_block_align = (*ptr).nBlockAlign as usize;
        let format_tag = (*ptr).wFormatTag;
        let cb_size = (*ptr).cbSize;

        if native_channels == 0 {
            tracing::error!(
                native_rate,
                format_tag,
                bits_per_sample,
                "[WASAPI] mix format reports zero channels"
            );
            return Err(AudioError::UnsupportedCaptureFormat {
                rate: native_rate,
                channels: 0,
            }
            .into());
        }

        if bits_per_sample == 0 {
            // Only compressed families report 0 here, and none of them are decodable
            // by `decode_samples_to_f32`. Rejecting now is better than reaching the
            // unknown-format branch and streaming silence for the whole session.
            tracing::error!(
                native_rate,
                native_channels,
                format_tag,
                "[WASAPI] mix format reports zero bits per sample"
            );
            return Err(AudioError::CaptureInstanceFailed(format!(
                "WASAPI mix format has zero bits per sample \
                 (rate {native_rate}, {native_channels} ch, tag {format_tag:#06x})"
            ))
            .into());
        }

        let expected_block_align = native_channels * (bits_per_sample as usize / 8);
        let block_align = if reported_block_align == expected_block_align {
            reported_block_align
        } else {
            // Trusting the reported value here is what allows the 24-bit branch to
            // compute an offset past the end of the buffer WASAPI actually handed over.
            tracing::warn!(
                reported_block_align,
                expected_block_align,
                native_channels,
                bits_per_sample,
                "[WASAPI] nBlockAlign disagrees with channels * bits/8; using the computed value"
            );
            expected_block_align
        };

        if block_align == 0 {
            tracing::error!(
                native_rate,
                native_channels,
                bits_per_sample,
                "[WASAPI] mix format yields a zero block alignment"
            );
            return Err(AudioError::CaptureInstanceFailed(format!(
                "WASAPI mix format has zero block alignment \
                 (rate {native_rate}, {native_channels} ch, {bits_per_sample} bits)"
            ))
            .into());
        }

        let is_float = if format_tag == 0xFFFE {
            if cb_size >= WAVEFORMATEXTENSIBLE_EXTENSION_SIZE {
                let ext = ptr as *const windows::Win32::Media::Audio::WAVEFORMATEXTENSIBLE;
                let sub_format = std::ptr::addr_of!((*ext).SubFormat).read_unaligned();
                sub_format == KSDATAFORMAT_SUBTYPE_IEEE_FLOAT
            } else {
                // An EXTENSIBLE tag with a short extension is a malformed descriptor.
                // Treated as integer PCM rather than read past the allocation: if the
                // bit depth is one the decoder handles the stream still plays, and if
                // it is not, the unknown-format branch logs and emits silence.
                tracing::warn!(
                    cb_size,
                    required = WAVEFORMATEXTENSIBLE_EXTENSION_SIZE,
                    "[WASAPI] EXTENSIBLE format has a short extension; SubFormat not read"
                );
                false
            }
        } else {
            format_tag == 3 // WAVE_FORMAT_IEEE_FLOAT
        };

        Ok(WasapiFormat {
            native_rate,
            native_channels,
            bits_per_sample,
            block_align,
            is_float,
        })
    }
}

/// Decode raw WASAPI capture buffer bytes into f32 samples.
///
/// Supports IEEE Float 32-bit, PCM 16-bit, PCM 24-bit packed, and PCM 32-bit.
/// Unknown formats produce silence, counted in `unknown_format_buffers` and logged
/// once — silence that is indistinguishable from real silence is the reason a format
/// mismatch here has been undiagnosable from a field capture.
///
/// The per-sample arithmetic is duplicated from
/// [`crate::audio::mixdown::pcm_sample_to_f32`], which is the platform-neutral
/// reference definition and carries the tests. It is duplicated rather than called
/// because the 32-bit float path is a single `extend_from_slice` here and would become
/// a per-sample call; keep the two in step when changing either.
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
    counters: &CaptureCounters,
) {
    unsafe {
        // Defence in depth: `parse_mix_format` cannot return a zero here, but this is
        // a `pub unsafe fn` and both the divisor at the 24-bit branch and
        // `downmix_to_stereo` downstream would panic on integer division by zero.
        if format.native_channels == 0 {
            output.clear();
            return;
        }

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
            // Unknown format — push silence.
            //
            // Logged through a `Once` rather than per buffer: this fires on the capture
            // thread at the device's callback rate, so an unconditional warning would
            // be a logging burst on the real-time path. The counter carries the rate;
            // the log carries the descriptor needed to identify which format it was.
            CaptureCounters::add(&counters.unknown_format_buffers, 1);
            static UNKNOWN_FORMAT_LOG: std::sync::Once = std::sync::Once::new();
            UNKNOWN_FORMAT_LOG.call_once(|| {
                tracing::warn!(
                    native_rate = format.native_rate,
                    native_channels = format.native_channels,
                    bits_per_sample = format.bits_per_sample,
                    block_align = format.block_align,
                    is_float = format.is_float,
                    "[WASAPI] unsupported sample format; emitting silence for the rest of \
                     this session (logged once)"
                );
            });
            output.resize(total_samples, 0.0);
        }
    }
}

/// Downmix multi-channel audio to stereo (interleaved L/R pairs).
///
/// Re-exported from [`crate::audio::mixdown`], which is platform-neutral so this
/// arithmetic can be tested on every CI leg rather than only the Windows one. Two
/// known defects in the multi-channel fold are documented there.
pub use crate::audio::mixdown::downmix_to_stereo;

// ---------------------------------------------------------------------------
// Shared async activation helpers
// ---------------------------------------------------------------------------

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
            Com::{
                COINIT_MULTITHREADED, CoInitializeEx, IAgileObject, IAgileObject_Impl,
                StructuredStorage::PROPVARIANT,
            },
            Variant::VT_BLOB,
        },
    },
    core::{ComInterface, IUnknown, PCWSTR, implement},
};

/// Completion handler for `ActivateAudioInterfaceAsync`.
///
/// Receives the activated `IAudioClient` (or error) and sends it back
/// to the calling thread via a `std::sync::mpsc` channel.
#[implement(IActivateAudioInterfaceCompletionHandler, IAgileObject)]
pub(crate) struct AudioActivator {
    pub tx: std::sync::mpsc::Sender<Result<IAudioClient, GemaCastError>>,
}

impl IAgileObject_Impl for AudioActivator {}

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
        let _ = self.tx.send(payload);

        Ok(())
    }
}

/// How long to wait for `ActivateCompleted` before giving up.
///
/// `ActivateAudioInterfaceAsync` completes on a system thread pool, and nothing
/// guarantees it ever calls back — an unavailable audio service, or a target process
/// that exits between the PID lookup and the activation, both leave the completion
/// handler unfired. An unbounded `recv()` here blocks the caller forever with no
/// diagnostic, and this runs during connection setup, so the phone sees a silent hang
/// rather than a failure.
const ACTIVATION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Activate a process loopback `IAudioClient` via `ActivateAudioInterfaceAsync`.
///
/// # Arguments
/// - `pid`: The target process ID.
/// - `mode`: `INCLUDE` to capture only the target tree, `EXCLUDE` to capture everything except it.
///
/// # Errors
///
/// [`AudioError::CaptureInstanceFailed`] if the activation does not complete within
/// [`ACTIVATION_TIMEOUT`], or [`AudioError::WindowsApi`] if it completes with a
/// failure.
///
/// # Safety
///
/// Calls COM interfaces. COM must be initialized on the calling thread.
pub unsafe fn activate_process_loopback(
    pid: u32,
    mode: PROCESS_LOOPBACK_MODE,
) -> Result<IAudioClient, GemaCastError> {
    unsafe {
        // Deliberately discarded. This thread is normally already initialized by
        // `run_application_loopback_thread`, in which case a second call with the same
        // apartment type returns `S_FALSE`, and a call with a different one returns
        // `RPC_E_CHANGED_MODE` — both benign here, because either way COM is usable and
        // the matching `CoUninitialize` is owned by whoever initialized first. Only a
        // genuine initialization failure would matter, and it surfaces immediately at
        // the `ActivateAudioInterfaceAsync` call below.
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    };

    let loopback_params = AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
        ProcessLoopbackMode: mode,
        TargetProcessId: pid,
    };

    // Heap-allocated, not a stack local, and that is the whole reason the timeout below
    // is safe to add.
    //
    // The PROPVARIANT hands WASAPI a raw pointer to these parameters. Microsoft's own
    // sample keeps them on the stack and then blocks on the completion event with
    // `INFINITE`, so nothing documents them as consumed before
    // `ActivateAudioInterfaceAsync` returns — the frame simply always outlives the
    // operation there. A `recv_timeout` breaks that: the function can now return while
    // the activation is still outstanding, and a stack local would leave WASAPI holding
    // a dangling pointer. Owning the allocation separately lets the timeout path leak
    // it deliberately instead.
    let activation_params = Box::into_raw(Box::new(AUDIOCLIENT_ACTIVATION_PARAMS {
        ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
        Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 {
            ProcessLoopbackParams: loopback_params,
        },
    }));

    let mut prop_variant = PROPVARIANT::default();
    unsafe {
        (*prop_variant.Anonymous.Anonymous).vt = VT_BLOB;
        (*prop_variant.Anonymous.Anonymous).Anonymous.blob.cbSize =
            std::mem::size_of::<AUDIOCLIENT_ACTIVATION_PARAMS>() as u32;
        (*prop_variant.Anonymous.Anonymous).Anonymous.blob.pBlobData = activation_params as *mut u8;
    };

    let (tx, receiver) = std::sync::mpsc::channel();
    let activator: IActivateAudioInterfaceCompletionHandler = AudioActivator { tx }.into();

    let request = unsafe {
        ActivateAudioInterfaceAsync(
            PCWSTR::from_raw(VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK.as_ptr()),
            &IAudioClient::IID,
            Some(&prop_variant),
            Some(&activator),
        )
    };
    if let Err(error) = request {
        // The call never took ownership of anything, so the parameters are free to go.
        drop(unsafe { Box::from_raw(activation_params) });
        return Err(AudioError::WindowsApi(error).into());
    }

    let completion = receiver.recv_timeout(ACTIVATION_TIMEOUT);

    match &completion {
        // The handler fired, so the activation is over and WASAPI is done with the
        // parameters. This is the only path on which freeing them is provably sound.
        Ok(_) => drop(unsafe { Box::from_raw(activation_params) }),
        Err(_) => {
            // Leaked on purpose: the operation may still be outstanding and still
            // holding this pointer. One `AUDIOCLIENT_ACTIVATION_PARAMS` (8 bytes) per
            // failed activation is a bounded cost on a path that already means capture
            // could not start, and it is the alternative to a use-after-free.
            std::mem::forget(unsafe { Box::from_raw(activation_params) });
        }
    }

    match completion {
        Ok(payload) => payload,
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            tracing::error!(
                pid,
                timeout_secs = ACTIVATION_TIMEOUT.as_secs(),
                "[WASAPI] process loopback activation timed out"
            );
            Err(AudioError::CaptureInstanceFailed(format!(
                "WASAPI process loopback activation for PID {pid} timed out after {}s",
                ACTIVATION_TIMEOUT.as_secs()
            ))
            .into())
        }
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            // The handler was dropped without sending, which means COM released it
            // without ever calling `ActivateCompleted`.
            tracing::error!(
                pid,
                "[WASAPI] process loopback activation handler dropped without completing"
            );
            Err(AudioError::WindowsApi(windows::core::Error::from(
                windows::Win32::Foundation::E_FAIL,
            ))
            .into())
        }
    }
}

/// Friendly name of an audio endpoint (`"Speakers (Realtek(R) Audio)"`), for logs only.
///
/// `None` if the property store cannot be read or the value is not a string. A name we
/// could not read must never fail a capture that is otherwise fine.
///
/// # Safety
///
/// Calls COM interfaces on `device`.
unsafe fn endpoint_friendly_name(
    device: &windows::Win32::Media::Audio::IMMDevice,
) -> Option<String> {
    use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
    use windows::Win32::System::Com::STGM_READ;
    use windows::Win32::System::Com::StructuredStorage::PropVariantClear;
    use windows::Win32::System::Variant::VT_LPWSTR;

    unsafe {
        let store = device.OpenPropertyStore(STGM_READ).ok()?;
        let mut value = store.GetValue(&PKEY_Device_FriendlyName).ok()?;

        // Check the tag before touching the union: reading the wrong arm is UB, and a
        // driver is free to store this property as something other than a string.
        let name = if value.Anonymous.Anonymous.vt == VT_LPWSTR {
            value.Anonymous.Anonymous.Anonymous.pwszVal.to_string().ok()
        } else {
            None
        };

        let _ = PropVariantClear(&mut value);
        name
    }
}

/// Query the default render endpoint's mix format, and the name of that endpoint.
///
/// Process loopback streams use the same shared-mode format as the system mixer,
/// so this gives us the correct format to pass to `IAudioClient::Initialize`.
///
/// The name is returned alongside rather than read by a second call on purpose: it must
/// describe the *same* `IMMDevice` the format came from. Enumerating again could land on
/// a different endpoint if the user changes their default output in between, and a log
/// line naming the wrong device is worse than one naming none.
///
/// # Safety
///
/// Calls COM interfaces. The returned pointer is CoTaskMem-allocated and must
/// be freed by the caller via `CoTaskMemFree`.
pub unsafe fn get_default_mix_format() -> Result<
    (
        *mut windows::Win32::Media::Audio::WAVEFORMATEX,
        Option<String>,
    ),
    GemaCastError,
> {
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

        let endpoint_name = endpoint_friendly_name(&device);

        let audio_client: IAudioClient = device
            .Activate(CLSCTX_ALL, None)
            .map_err(AudioError::WindowsApi)?;

        let mix_format_ptr = audio_client
            .GetMixFormat()
            .map_err(AudioError::WindowsApi)?;

        Ok((mix_format_ptr, endpoint_name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering::Relaxed;
    use windows::Win32::Media::Audio::{WAVEFORMATEX, WAVEFORMATEXTENSIBLE};

    /// A `WAVEFORMATEX` with the fields [`parse_mix_format`] reads, and nothing else.
    ///
    /// `nAvgBytesPerSec` is left at zero because the parser never looks at it; if that
    /// changes, this helper has to grow a field rather than the test silently passing on
    /// a zero.
    fn base_format(tag: u16, channels: u16, rate: u32, bits: u16, cb_size: u16) -> WAVEFORMATEX {
        WAVEFORMATEX {
            wFormatTag: tag,
            nChannels: channels,
            nSamplesPerSec: rate,
            nAvgBytesPerSec: 0,
            nBlockAlign: channels * (bits / 8),
            wBitsPerSample: bits,
            cbSize: cb_size,
        }
    }

    /// A `WAVEFORMATEXTENSIBLE` whose extension is fully populated in memory.
    ///
    /// `cb_size` is passed separately from the real extension contents on purpose: what
    /// several of these tests exercise is a descriptor whose length field disagrees with
    /// what is actually there.
    fn extensible_format(cb_size: u16, sub_format: windows::core::GUID) -> WAVEFORMATEXTENSIBLE {
        WAVEFORMATEXTENSIBLE {
            Format: base_format(0xFFFE, 2, 48_000, 32, cb_size),
            SubFormat: sub_format,
            ..Default::default()
        }
    }

    /// These tests only run on the Windows CI leg.
    ///
    /// That is inherent rather than an oversight: `WAVEFORMATEX` is a Win32 type and
    /// this whole file is `#![cfg(target_os = "windows")]`. The parts of the decode path
    /// that *can* be tested everywhere were moved to [`crate::audio::mixdown`] for
    /// exactly this reason, and what is left here is the pointer handling, which needs
    /// the real struct layout to mean anything.
    ///
    /// # Falsification
    ///
    /// The hardening this covers has five independent parts, so each was reverted in
    /// turn and the suite re-run. What each revert broke:
    ///
    /// | reverted guard | tests that fail |
    /// | --- | --- |
    /// | `nChannels == 0` rejection | `should_reject_zero_channels…` |
    /// | `wBitsPerSample == 0` rejection | `should_reject_zero_bits_per_sample` |
    /// | `nBlockAlign` cross-check | `should_use_the_computed_block_align…` |
    /// | zero-`block_align` rejection | `should_reject_a_bit_depth_below_one_byte…` |
    /// | `cbSize >= 22` gate | `should_ignore_a_subformat_that_cbsize_says_is_absent`, `should_read_the_subformat_when_the_extension_is_exactly_long_enough` |
    /// | unknown-format counter | `should_count_and_silence_an_undecodable_format` |
    /// | zero-channel early return in the decoder | `should_return_without_dividing…` |
    ///
    /// Two tests pass under every revert and are contract nets rather than
    /// discriminators, which is worth knowing before trusting them:
    /// `should_uphold_the_invariants_its_doc_comment_promises` (its descriptors are
    /// already self-consistent, so the cross-check has nothing to correct) and
    /// `should_not_read_the_subformat_when_the_extension_is_short` (it pins the
    /// allocation boundary for a sanitizer run; under a plain `cargo test` the
    /// out-of-bounds read returns whatever follows the allocation, which does not spell
    /// the float GUID either way).
    mod parse_mix_format {
        use super::*;

        #[test]
        fn should_accept_the_common_shared_mode_float_format() {
            // What every current Windows mixer reports: EXTENSIBLE, 32-bit float, with
            // the full 22-byte extension present.
            let ext = extensible_format(
                WAVEFORMATEXTENSIBLE_EXTENSION_SIZE,
                KSDATAFORMAT_SUBTYPE_IEEE_FLOAT,
            );

            let format =
                unsafe { parse_mix_format(&ext as *const _ as *const WAVEFORMATEX) }.unwrap();

            assert_eq!(format.native_rate, 48_000);
            assert_eq!(format.native_channels, 2);
            assert_eq!(format.bits_per_sample, 32);
            assert_eq!(format.block_align, 8);
            assert!(format.is_float);
        }

        #[test]
        fn should_reject_zero_channels_rather_than_pass_a_divisor_of_zero_downstream() {
            // The root of both division-by-zero panics: `downmix_to_stereo` divides by
            // this, and so does the 24-bit decode branch.
            let format = base_format(0xFFFE, 0, 48_000, 32, WAVEFORMATEXTENSIBLE_EXTENSION_SIZE);

            match unsafe { parse_mix_format(&format as *const _) } {
                Err(GemaCastError::Audio(AudioError::UnsupportedCaptureFormat {
                    rate,
                    channels,
                })) => {
                    assert_eq!(rate, 48_000);
                    assert_eq!(channels, 0);
                }
                Ok(parsed) => panic!("expected rejection, got {parsed:?}"),
                Err(other) => panic!("expected UnsupportedCaptureFormat, got {other:?}"),
            }
        }

        #[test]
        fn should_reject_zero_bits_per_sample() {
            // Compressed families report this. None of them are decodable here, and
            // accepting one streams silence for the whole session.
            let format = base_format(0xFFFE, 2, 48_000, 0, WAVEFORMATEXTENSIBLE_EXTENSION_SIZE);

            assert!(matches!(
                unsafe { parse_mix_format(&format as *const _) },
                Err(GemaCastError::Audio(AudioError::CaptureInstanceFailed(_)))
            ));
        }

        #[test]
        fn should_reject_a_bit_depth_below_one_byte_because_it_yields_a_zero_block_align() {
            // 4-bit ADPCM. `bits / 8` truncates to 0, so the computed block alignment is
            // 0 even though neither the channel count nor the bit depth is — this is the
            // only way the zero-block-align branch is reachable, and it has to be, since
            // `block_align` bounds the slice the 24-bit branch reads.
            let mut format = base_format(1, 2, 48_000, 4, 0);
            format.nBlockAlign = 1; // what a real ADPCM descriptor reports

            assert!(matches!(
                unsafe { parse_mix_format(&format as *const _) },
                Err(GemaCastError::Audio(AudioError::CaptureInstanceFailed(_)))
            ));
        }

        #[test]
        fn should_use_the_computed_block_align_when_the_reported_one_disagrees() {
            // A descriptor claiming 32-bit stereo but a 4-byte frame. Trusting the
            // reported value makes the 24-bit branch's `offset` arithmetic address past
            // the end of the buffer WASAPI actually handed over.
            let mut format = base_format(1, 2, 48_000, 32, 0);
            format.nBlockAlign = 4;

            let parsed = unsafe { parse_mix_format(&format as *const _) }.unwrap();

            assert_eq!(parsed.block_align, 8, "must be channels * bits / 8");
        }

        #[test]
        fn should_uphold_the_invariants_its_doc_comment_promises() {
            // `WasapiFormat`'s doc tells downstream code it may divide by
            // `native_channels` and index by `block_align` without checking. Pin that
            // for every descriptor the parser accepts, not just the float one.
            for bits in [16u16, 24, 32] {
                for channels in [1u16, 2, 6, 8] {
                    let format = base_format(1, channels, 44_100, bits, 0);
                    let parsed = unsafe { parse_mix_format(&format as *const _) }.unwrap();

                    assert!(parsed.native_channels > 0);
                    assert!(parsed.bits_per_sample > 0);
                    assert_eq!(
                        parsed.block_align,
                        parsed.native_channels * (parsed.bits_per_sample as usize / 8)
                    );
                }
            }
        }

        #[test]
        fn should_read_the_legacy_float_tag_without_an_extension() {
            let float = base_format(3, 2, 48_000, 32, 0);
            assert!(
                unsafe { parse_mix_format(&float as *const _) }
                    .unwrap()
                    .is_float
            );

            let pcm = base_format(1, 2, 48_000, 16, 0);
            assert!(
                !unsafe { parse_mix_format(&pcm as *const _) }
                    .unwrap()
                    .is_float
            );
        }

        #[test]
        fn should_ignore_a_subformat_that_cbsize_says_is_absent() {
            // The behavioural half of the `cbSize` gate, and the case a driver really
            // produces: the extension is fully populated in memory but `cbSize` reports
            // it as absent. Every byte read here is in bounds, so what is under test is
            // purely whether the parser honours the length field.
            //
            // This is the discriminating test for the gate. The boundary test below
            // cannot be: with the gate removed it reads whatever follows the allocation,
            // and that is overwhelmingly unlikely to spell the float GUID, so it returns
            // the same `is_float == false` either way.
            let ext = extensible_format(0, KSDATAFORMAT_SUBTYPE_IEEE_FLOAT);

            let parsed =
                unsafe { parse_mix_format(&ext as *const _ as *const WAVEFORMATEX) }.unwrap();

            assert!(
                !parsed.is_float,
                "cbSize is the only thing that says the SubFormat is readable"
            );
        }

        #[test]
        fn should_not_read_the_subformat_when_the_extension_is_short() {
            // The memory-safety half, reproduced: an EXTENSIBLE tag whose `cbSize` says
            // the extension is absent, in an allocation that really is only
            // `size_of::<WAVEFORMATEX>()` bytes long. `GetMixFormat` returns exactly
            // `size_of::<WAVEFORMATEX>() + cbSize` bytes, so this is the shape the old
            // code cast to `WAVEFORMATEXTENSIBLE` and read `SubFormat` out of — 16 bytes
            // starting past the end.
            //
            // A `Vec` of one element allocates exactly one element, so a sanitizer or
            // Miri run has a real boundary to trip on. Reading a stack local would hit
            // neighbouring stack slots instead and prove nothing. Under a plain `cargo
            // test` this passes either way — see the note in the test above.
            let owned = vec![base_format(0xFFFE, 2, 48_000, 32, 0)];
            assert_eq!(
                std::mem::size_of_val(owned.as_slice()),
                std::mem::size_of::<WAVEFORMATEX>(),
                "the allocation must end where a real GetMixFormat one would"
            );

            let parsed = unsafe { parse_mix_format(owned.as_ptr()) }.unwrap();

            // Falls back to integer PCM. 32-bit integer is a format the decoder handles,
            // so the stream still plays rather than going silent.
            assert!(!parsed.is_float);
            assert_eq!(parsed.bits_per_sample, 32);
        }

        #[test]
        fn should_read_the_subformat_when_the_extension_is_exactly_long_enough() {
            // The boundary itself: `cbSize` exactly 22 must be read, not rejected. Off
            // by one in the other direction and every modern mixer's float format is
            // silently decoded as integer — full-scale float samples reinterpreted as
            // tiny integers, which is inaudible rather than obviously broken.
            let mut ext = extensible_format(
                WAVEFORMATEXTENSIBLE_EXTENSION_SIZE,
                KSDATAFORMAT_SUBTYPE_IEEE_FLOAT,
            );

            assert!(
                unsafe { parse_mix_format(&ext as *const _ as *const WAVEFORMATEX) }
                    .unwrap()
                    .is_float
            );

            ext.Format.cbSize = WAVEFORMATEXTENSIBLE_EXTENSION_SIZE - 1;
            assert!(
                !unsafe { parse_mix_format(&ext as *const _ as *const WAVEFORMATEX) }
                    .unwrap()
                    .is_float,
                "one byte short is still short"
            );
        }
    }

    mod decode_samples_to_f32 {
        use super::*;

        fn float_format(channels: usize) -> WasapiFormat {
            WasapiFormat {
                native_rate: 48_000,
                native_channels: channels,
                bits_per_sample: 32,
                block_align: channels * 4,
                is_float: true,
            }
        }

        #[test]
        fn should_decode_a_float_buffer_verbatim() {
            let samples = [0.25f32, -0.5, 0.75, -1.0];
            let mut output = Vec::new();
            let counters = CaptureCounters::default();

            unsafe {
                decode_samples_to_f32(
                    samples.as_ptr() as *const u8,
                    &float_format(2),
                    2,
                    &mut output,
                    &counters,
                );
            }

            assert_eq!(output, samples);
            assert!(
                counters.all_clear(),
                "a supported format must count nothing"
            );
        }

        #[test]
        fn should_return_without_dividing_by_a_zero_channel_count() {
            // Defence in depth for the two panics: `parse_mix_format` no longer lets a
            // zero through, but this is a `pub unsafe fn` and the 24-bit branch's
            // `block_align / native_channels` runs before its loop, so "the loop never
            // executes" does not save it.
            //
            // The pointer is dangling-but-aligned rather than null on purpose. With
            // `native_channels == 0` the computed slice length is 0, so a dangling
            // aligned pointer is sound and removing the guard fails on the division —
            // the defect being pinned. A null pointer instead trips
            // `slice::from_raw_parts`'s own non-null precondition, which aborts the
            // process non-unwinding and takes the rest of the suite's results with it.
            let mut output = vec![1.0f32; 8];
            let counters = CaptureCounters::default();
            let mut format = float_format(0);
            format.bits_per_sample = 24;
            format.is_float = false;

            unsafe {
                decode_samples_to_f32(
                    std::ptr::NonNull::<u8>::dangling().as_ptr(),
                    &format,
                    4,
                    &mut output,
                    &counters,
                );
            }

            assert!(
                output.is_empty(),
                "no samples can be described by 0 channels"
            );
        }

        #[test]
        fn should_count_and_silence_an_undecodable_format() {
            // 12-bit PCM: a real WAVEFORMATEX can report it, and no branch here handles
            // it. Before the counter this emitted zeros indistinguishable from real
            // silence, which is why a format mismatch was undiagnosable from a field
            // capture.
            let mut format = float_format(2);
            format.bits_per_sample = 12;
            format.is_float = false;
            let mut output = Vec::new();
            let counters = CaptureCounters::default();

            unsafe {
                decode_samples_to_f32(std::ptr::null(), &format, 3, &mut output, &counters);
            }

            assert_eq!(output, vec![0.0f32; 6], "3 frames * 2 channels of silence");
            assert_eq!(
                counters.unknown_format_buffers.load(Relaxed),
                1,
                "the counter is the only signal this happened"
            );
        }
    }
}
