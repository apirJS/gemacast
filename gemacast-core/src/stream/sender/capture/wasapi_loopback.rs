#![cfg(target_os = "windows")]

use crate::{
    audio::OPUS_FRAME_SAMPLES,
    error::{AudioError, GemaCastError},
    stream::sender::capture::{CaptureBackend, CaptureHandle},
};
use ringbuf::{HeapRb, traits::*};
use std::sync::Arc;
use tokio::sync::{Notify, mpsc};

use windows::{
    Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, TH32CS_SNAPPROCESS},
    core::ComInterface,
};
use windows::{
    Win32::{
        Foundation::CloseHandle,
        Media::Audio::{
            IActivateAudioInterfaceCompletionHandler, IActivateAudioInterfaceCompletionHandler_Impl,
        },
    },
    core::IUnknown,
};
use windows::{
    Win32::{
        Media::Audio::IActivateAudioInterfaceAsyncOperation,
        System::Diagnostics::ToolHelp::{Process32First, Process32Next},
    },
    core::implement,
};
use windows::{
    Win32::{
        Media::Audio::{
            AUDIOCLIENT_ACTIVATION_PARAMS, AUDIOCLIENT_ACTIVATION_PARAMS_0,
            AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK, AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS,
            ActivateAudioInterfaceAsync, IAudioClient,
            PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE,
            VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
        },
        System::{
            Com::{COINIT_MULTITHREADED, CoInitializeEx, StructuredStorage::PROPVARIANT},
            Diagnostics::ToolHelp::PROCESSENTRY32,
            Variant::VT_BLOB,
        },
    },
    core::PCWSTR,
};

struct SendClient(IAudioClient);
unsafe impl Send for SendClient {}
unsafe impl Sync for SendClient {}

struct WasapiLoopbackCapture {
    client: SendClient,
}

impl CaptureBackend for WasapiLoopbackCapture {
    fn play(&mut self) -> Result<(), GemaCastError> {
        unsafe {
            self.client.0.Start().map_err(AudioError::WindowsApi)?;
        }
        Ok(())
    }

    fn pause(&mut self) -> Result<(), GemaCastError> {
        unsafe {
            self.client.0.Stop().map_err(AudioError::WindowsApi)?;
        }
        Ok(())
    }
}

pub fn create_wasapi_process_loopback(pid: u32) -> Result<CaptureHandle, GemaCastError> {
    unsafe {
        let audio_client = activate_process_loopback(pid)?;

        // Process loopback IAudioClients don't support GetMixFormat() (returns E_NOTIMPL).
        // We must query the system's shared-mode mix format from the default render endpoint,
        // which all process audio streams conform to.
        let mix_format_ptr = get_default_mix_format()?;

        let native_rate = (*mix_format_ptr).nSamplesPerSec;
        let native_channels = (*mix_format_ptr).nChannels as usize;
        let bits_per_sample = (*mix_format_ptr).wBitsPerSample;
        let block_align = (*mix_format_ptr).nBlockAlign as usize;

        // Determine if the format is IEEE float or PCM integer
        let is_float = if (*mix_format_ptr).wFormatTag == 0xFFFE {
            // WAVE_FORMAT_EXTENSIBLE — check SubFormat GUID
            let ext = mix_format_ptr as *const windows::Win32::Media::Audio::WAVEFORMATEXTENSIBLE;
            let float_guid = windows::core::GUID::from_u128(0x00000003_0000_0010_8000_00aa00389b71);
            let sub_format = std::ptr::addr_of!((*ext).SubFormat).read_unaligned();
            sub_format == float_guid
        } else {
            (*mix_format_ptr).wFormatTag == 3 // WAVE_FORMAT_IEEE_FLOAT
        };

        tracing::info!(
            "[WASAPI] Process loopback: native_rate={}, native_channels={}, bits={}, block_align={}, is_float={}",
            native_rate,
            native_channels,
            bits_per_sample,
            block_align,
            is_float
        );

        let init_result = audio_client.Initialize(
            windows::Win32::Media::Audio::AUDCLNT_SHAREMODE_SHARED,
            windows::Win32::Media::Audio::AUDCLNT_STREAMFLAGS_LOOPBACK
                | windows::Win32::Media::Audio::AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
            10000000,
            0,
            mix_format_ptr,
            None,
        );

        // Free the CoTaskMem-allocated format
        windows::Win32::System::Com::CoTaskMemFree(Some(mix_format_ptr as _));

        init_result.map_err(AudioError::WindowsApi)?;

        let event_handle =
            windows::Win32::System::Threading::CreateEventW(None, false, false, None)
                .map_err(AudioError::WindowsApi)?;

        audio_client
            .SetEventHandle(event_handle)
            .map_err(AudioError::WindowsApi)?;

        let capture_client: windows::Win32::Media::Audio::IAudioCaptureClient =
            audio_client.GetService().map_err(AudioError::WindowsApi)?;

        struct SendCaptureClient(windows::Win32::Media::Audio::IAudioCaptureClient);
        unsafe impl Send for SendCaptureClient {}
        let send_capture_client = SendCaptureClient(capture_client);

        let rb = HeapRb::<f32>::new(OPUS_FRAME_SAMPLES * 64);
        let (mut rb_producer, rb_consumer) = rb.split();
        let (_stream_error_tx, stream_error_rx) = mpsc::channel::<cpal::StreamError>(1);
        let notify = Arc::new(Notify::new());
        let notify_clone = notify.clone();
        let client_clone = audio_client.clone();

        // NOTE: Do NOT call audio_client.Start() here!
        // The encode loop in AudioCaptureInstance calls capture.backend.play()
        // which calls Start(). Calling it here would cause AUDCLNT_E_NOT_STOPPED
        // when play() tries to start it again, silently killing the encode loop.

        std::thread::spawn(move || {
            let send_capture_client = send_capture_client;
            let capture_client = send_capture_client.0;

            let mut phase: f32 = 0.0;
            let phase_inc = native_rate as f32 / 48000.0;

            loop {
                let wait_res =
                    windows::Win32::System::Threading::WaitForSingleObject(event_handle, 1000);
                if wait_res != windows::Win32::Foundation::WAIT_OBJECT_0 {
                    continue;
                }

                let mut packet_length = match capture_client.GetNextPacketSize() {
                    Ok(len) => len,
                    Err(_) => break,
                };

                while packet_length > 0 {
                    let mut buffer_ptr: *mut u8 = std::ptr::null_mut();
                    let mut num_frames_available = 0;
                    let mut flags = 0;
                    let mut device_position = 0;
                    let mut qpc_position = 0;

                    if capture_client
                        .GetBuffer(
                            &mut buffer_ptr,
                            &mut num_frames_available,
                            &mut flags,
                            Some(&mut device_position),
                            Some(&mut qpc_position),
                        )
                        .is_err()
                    {
                        break;
                    }

                    if num_frames_available == 0 {
                        let _ = capture_client.ReleaseBuffer(0);

                        packet_length = match capture_client.GetNextPacketSize() {
                            Ok(len) => len,
                            Err(_) => break,
                        };
                        continue;
                    }

                    if (flags & 2) != 0 || buffer_ptr.is_null() {
                        let silent_samples = vec![0.0f32; num_frames_available as usize * 2];
                        if rb_producer.vacant_len() >= silent_samples.len() {
                            let _ = rb_producer.push_slice(&silent_samples);
                        }
                    } else {
                        let src_frames = num_frames_available as usize;
                        let total_bytes = src_frames * block_align;
                        let raw_bytes = std::slice::from_raw_parts(buffer_ptr, total_bytes);

                        // Decode raw bytes into f32 samples based on format
                        let total_samples = src_frames * native_channels;
                        let mut decoded = Vec::with_capacity(total_samples);

                        if is_float && bits_per_sample == 32 {
                            // IEEE Float 32-bit — direct reinterpret
                            let float_ptr = buffer_ptr as *const f32;
                            let float_slice = std::slice::from_raw_parts(float_ptr, total_samples);
                            decoded.extend_from_slice(float_slice);
                        } else if !is_float && bits_per_sample == 16 {
                            // PCM 16-bit signed integer
                            let i16_ptr = buffer_ptr as *const i16;
                            let i16_slice = std::slice::from_raw_parts(i16_ptr, total_samples);
                            for &s in i16_slice {
                                decoded.push(s as f32 / 32768.0);
                            }
                        } else if !is_float && bits_per_sample == 24 {
                            // PCM 24-bit packed (3 bytes per sample)
                            let bytes_per_sample = 3usize;
                            for i in 0..total_samples {
                                let offset = (i / native_channels) * block_align
                                    + (i % native_channels) * bytes_per_sample;
                                if offset + 2 < raw_bytes.len() {
                                    let b0 = raw_bytes[offset] as i32;
                                    let b1 = raw_bytes[offset + 1] as i32;
                                    let b2 = raw_bytes[offset + 2] as i32;
                                    let val = (b2 << 24) | (b1 << 16) | (b0 << 8);
                                    decoded.push(val as f32 / 2147483648.0);
                                } else {
                                    decoded.push(0.0);
                                }
                            }
                        } else if !is_float && bits_per_sample == 32 {
                            // PCM 32-bit signed integer
                            let i32_ptr = buffer_ptr as *const i32;
                            let i32_slice = std::slice::from_raw_parts(i32_ptr, total_samples);
                            for &s in i32_slice {
                                decoded.push(s as f32 / 2147483648.0);
                            }
                        } else {
                            // Unknown format — push silence
                            decoded.resize(total_samples, 0.0);
                        }

                        // Now resample/downmix the decoded f32 samples
                        if native_rate == 48000 && native_channels == 2 {
                            if rb_producer.vacant_len() >= decoded.len() {
                                let _ = rb_producer.push_slice(&decoded);
                            }
                        } else {
                            // Inline linear resampler and downmixer
                            let target_frames = (src_frames as f32 / phase_inc).ceil() as usize;
                            let mut out = Vec::with_capacity(target_frames * 2);

                            while (phase as usize) < src_frames {
                                let idx = phase as usize;
                                let frac = phase - idx as f32;

                                let next_idx = (idx + 1).min(src_frames - 1);

                                let (l, r, next_l, next_r);

                                if native_channels == 1 {
                                    l = decoded[idx];
                                    r = decoded[idx];
                                    next_l = decoded[next_idx];
                                    next_r = decoded[next_idx];
                                } else {
                                    l = decoded[idx * native_channels];
                                    r = decoded[idx * native_channels + 1];
                                    next_l = decoded[next_idx * native_channels];
                                    next_r = decoded[next_idx * native_channels + 1];
                                }

                                out.push(l + (next_l - l) * frac);
                                out.push(r + (next_r - r) * frac);

                                phase += phase_inc;
                            }

                            phase -= src_frames as f32;

                            if rb_producer.vacant_len() >= out.len() {
                                let _ = rb_producer.push_slice(&out);
                            }
                        }
                    }

                    let _ = capture_client.ReleaseBuffer(num_frames_available);

                    packet_length = match capture_client.GetNextPacketSize() {
                        Ok(len) => len,
                        Err(_) => break,
                    };
                }
                
                notify_clone.notify_one();
            }
        });

        Ok(CaptureHandle {
            backend: Box::new(WasapiLoopbackCapture {
                client: SendClient(client_clone),
            }),
            consumer: rb_consumer,
            notify,
            stream_error_rx,
        })
    }
}

#[implement(IActivateAudioInterfaceCompletionHandler)]
struct AudioActivator {
    sender: std::sync::mpsc::Sender<Result<IAudioClient, GemaCastError>>,
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

unsafe fn activate_process_loopback(pid: u32) -> Result<IAudioClient, GemaCastError> {
    unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED).map_err(AudioError::WindowsApi)?;
    };
    let loopback_params = AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
        ProcessLoopbackMode: PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE,
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
/// Process loopback streams use the same shared-mode format as the system mixer,
/// so this gives us the correct format to pass to IAudioClient::Initialize.
unsafe fn get_default_mix_format()
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

const SYSTEM_PROCESS_FILTER: &[&str] = &[
    "audiodg.exe",
    "svchost.exe",
    "csrss.exe",
    "dwm.exe",
    "lsass.exe",
    "smss.exe",
    "wininit.exe",
    "winlogon.exe",
    "services.exe",
    "system",
    "idle",
    "registry",
    "fontdrvhost.exe",
    "conhost.exe",
    "sihost.exe",
    "taskhostw.exe",
    "ctfmon.exe",
    "runtimebroker.exe",
    "searchhost.exe",
    "startmenuexperiencehost.exe",
    "textinputhost.exe",
    "shellexperiencehost.exe",
    "applicationframehost.exe",
    "securityhealthservice.exe",
    "ntoskrnl.exe",
    "spoolsv.exe",
    "lsaiso.exe",
    "dllhost.exe",
    "wmiprvse.exe",
    "searchindexer.exe",
    "msdtc.exe",
    "sgrmbroker.exe",
    "memorycompression",
    "systemsettings.exe",
    "securityhealthsystray.exe",
    "smartscreen.exe",
    "compactoverlay.exe",
    "lockapp.exe",
    "gamebar.exe",
    "gamebarpresencewriter.exe",
    "widgetservice.exe",
    "widgets.exe",
    "phoneexperiencehost.exe",
    "yourphone.exe",
    "crashpad_handler.exe",
];

/// Enumerate all running processes, returning a map of PID → display name.
/// System and infrastructure processes are filtered out.
///
/// # Safety
///
/// Calls Win32 Toolhelp32 snapshot APIs (`CreateToolhelp32Snapshot`,
/// `Process32First`, `Process32Next`, `CloseHandle`). Safe to call
/// from any thread.
pub unsafe fn get_process_list() -> Result<std::collections::HashMap<u32, String>, GemaCastError> {
    let mut map = std::collections::HashMap::new();

    unsafe {
        let snapshot =
            CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).map_err(AudioError::WindowsApi)?;

        let mut entry = PROCESSENTRY32 {
            dwSize: std::mem::size_of::<PROCESSENTRY32>() as u32,
            ..Default::default()
        };

        if Process32First(snapshot, &mut entry).is_ok() {
            while Process32Next(snapshot, &mut entry).is_ok() {
                let raw_name = String::from_utf8_lossy(
                    &entry
                        .szExeFile
                        .iter()
                        .copied()
                        .take_while(|b| *b != 0)
                        .collect::<Vec<u8>>(),
                )
                .into_owned();

                let lower = raw_name.to_lowercase();
                if SYSTEM_PROCESS_FILTER.contains(&lower.as_str()) {
                    continue;
                }

                let display_name = raw_name
                    .strip_suffix(".exe")
                    .or_else(|| raw_name.strip_suffix(".EXE"))
                    .unwrap_or(&raw_name)
                    .to_string();

                map.insert(entry.th32ProcessID, display_name);
            }
        }

        CloseHandle(snapshot).map_err(AudioError::WindowsApi)?;
    };

    Ok(map)
}

/// Query the default audio endpoint's session manager for PIDs with active
/// audio sessions. Returns the set of process IDs currently producing audio.
///
/// # Safety
///
/// Calls COM interfaces (`CoInitializeEx`, `CoCreateInstance`,
/// `IAudioSessionManager2`, `IAudioSessionEnumerator`). Safe to call
/// from any thread; COM is initialized with `COINIT_MULTITHREADED`.
pub unsafe fn get_audio_process_list() -> Result<std::collections::HashSet<u32>, GemaCastError> {
    let mut set = std::collections::HashSet::new();

    unsafe {
        use windows::Win32::{
            Media::Audio::{
                IAudioSessionEnumerator, IAudioSessionManager2, IMMDevice, IMMDeviceEnumerator,
                MMDeviceEnumerator, eConsole, eRender,
            },
            System::Com::{CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx},
        };

        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);

        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                .map_err(AudioError::WindowsApi)?;

        let device: IMMDevice = enumerator
            .GetDefaultAudioEndpoint(eRender, eConsole)
            .map_err(AudioError::WindowsApi)?;

        let session_manager: IAudioSessionManager2 = device
            .Activate(CLSCTX_ALL, None)
            .map_err(AudioError::WindowsApi)?;

        let session_enumerator: IAudioSessionEnumerator = session_manager
            .GetSessionEnumerator()
            .map_err(AudioError::WindowsApi)?;

        let session_count = session_enumerator
            .GetCount()
            .map_err(AudioError::WindowsApi)?;

        for i in 0..session_count {
            use windows::Win32::Media::Audio::{IAudioSessionControl, IAudioSessionControl2};

            let session: IAudioSessionControl = session_enumerator
                .GetSession(i)
                .map_err(AudioError::WindowsApi)?;

            let session2: IAudioSessionControl2 = session.cast().map_err(AudioError::WindowsApi)?;

            let pid = session2.GetProcessId().map_err(AudioError::WindowsApi)?;

            set.insert(pid);
        }
    }

    Ok(set)
}

/// Walk the process tree upward from `pid` to find the topmost ancestor
/// whose executable name matches `exe_lower` (lowercased, with or without `.exe`).
/// Returns the root ancestor PID so `INCLUDE_TARGET_PROCESS_TREE` captures
/// the entire tree's audio — critical for multi-process apps like Chrome
/// where audio is produced by a child renderer, not the main browser process.
pub fn get_root_ancestor_pid(pid: u32, exe_lower: &str) -> u32 {
    // Build a mapping of pid -> (parent_pid, exe_name_lower) for all processes
    let mut parent_map = std::collections::HashMap::<u32, (u32, String)>::new();

    unsafe {
        let Ok(snapshot) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            return pid;
        };

        let mut entry = PROCESSENTRY32 {
            dwSize: std::mem::size_of::<PROCESSENTRY32>() as u32,
            ..Default::default()
        };

        if Process32First(snapshot, &mut entry).is_ok() {
            // Process32First already populates entry with the first process
            let raw_name = String::from_utf8_lossy(
                &entry
                    .szExeFile
                    .iter()
                    .copied()
                    .take_while(|b| *b != 0)
                    .collect::<Vec<u8>>(),
            )
            .into_owned();

            parent_map.insert(
                entry.th32ProcessID,
                (entry.th32ParentProcessID, raw_name.to_lowercase()),
            );

            while Process32Next(snapshot, &mut entry).is_ok() {
                let raw_name = String::from_utf8_lossy(
                    &entry
                        .szExeFile
                        .iter()
                        .copied()
                        .take_while(|b| *b != 0)
                        .collect::<Vec<u8>>(),
                )
                .into_owned();

                parent_map.insert(
                    entry.th32ProcessID,
                    (entry.th32ParentProcessID, raw_name.to_lowercase()),
                );
            }
        }

        let _ = CloseHandle(snapshot);
    }

    // Walk upward from `pid` as long as the parent has the same exe name
    let target_exe = format!("{}.exe", exe_lower);
    let mut current = pid;
    let mut visited = std::collections::HashSet::new();
    visited.insert(current);

    while let Some((parent_pid, parent_exe)) = parent_map.get(&current) {
        if *parent_pid == 0 || visited.contains(parent_pid) {
            break;
        }
        // Check if parent has the same executable name
        if *parent_exe == target_exe || *parent_exe == exe_lower {
            current = *parent_pid;
            visited.insert(current);
        } else {
            break;
        }
    }

    current
}
