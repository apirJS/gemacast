#![cfg(target_os = "windows")]

use crate::{
    audio::{CaptureResampler, OPUS_FRAME_SAMPLES},
    error::{AudioError, GemaCastError},
    stream::sender::capture::{CaptureBackend, CaptureHandle},
};
use ringbuf::{HeapRb, traits::*};
use std::sync::Arc;
use tokio::sync::{Notify, mpsc};

use super::wasapi_common::{decode_samples_to_f32, downmix_to_stereo, parse_mix_format};

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
    is_running: Arc<std::sync::atomic::AtomicBool>,
}

impl Drop for WasapiLoopbackCapture {
    fn drop(&mut self) {
        self.is_running.store(false, std::sync::atomic::Ordering::Relaxed);
    }
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
        let format = parse_mix_format(mix_format_ptr);

        tracing::info!(
            "[WASAPI] Process loopback: native_rate={}, native_channels={}, bits={}, block_align={}, is_float={}",
            format.native_rate,
            format.native_channels,
            format.bits_per_sample,
            format.block_align,
            format.is_float
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
        let client_clone = audio_client.clone();

        let rb = HeapRb::<f32>::new(OPUS_FRAME_SAMPLES * 64);
        let (mut rb_producer, rb_consumer) = rb.split();
        let (_stream_error_tx, stream_error_rx) = mpsc::channel::<cpal::StreamError>(1);
        let notify = Arc::new(Notify::new());
        let notify_clone = notify.clone();
        let is_running = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let is_running_thread = is_running.clone();

        // Build resampler if native format differs from pipeline's 48kHz stereo
        let needs_resample = format.native_rate != 48000 || format.native_channels != 2;
        let mut resampler = if needs_resample {
            let resample_channels = if format.native_channels == 2 { 2 } else { 2 };
            Some(CaptureResampler::new(format.native_rate, 48000, resample_channels)?)
        } else {
            None
        };

        std::thread::spawn(move || {
            // Force whole-struct capture. Rust 2021+ closures capture individual
            // fields; accessing .0 directly would capture the bare !Send
            // IAudioCaptureClient instead of the Send wrapper.
            let send_capture_client = send_capture_client;
            let cap_client = send_capture_client.0;
            let mut decoded = Vec::with_capacity(4096);
            let mut stereo_buf = Vec::with_capacity(4096);

            while is_running_thread.load(std::sync::atomic::Ordering::Relaxed) {
                let wait_res =
                    windows::Win32::System::Threading::WaitForSingleObject(event_handle, 500);
                if wait_res != windows::Win32::Foundation::WAIT_OBJECT_0 {
                    continue;
                }

                let mut packet_length = match cap_client.GetNextPacketSize() {
                    Ok(len) => len,
                    Err(_) => break,
                };

                while packet_length > 0 {
                    let mut buffer_ptr: *mut u8 = std::ptr::null_mut();
                    let mut num_frames_available = 0;
                    let mut flags = 0;
                    let mut device_position = 0;
                    let mut qpc_position = 0;

                    if cap_client
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
                        let _ = cap_client.ReleaseBuffer(0);
                        packet_length = match cap_client.GetNextPacketSize() {
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

                        // Decode raw bytes → f32 using shared utilities
                        decode_samples_to_f32(buffer_ptr, &format, src_frames, &mut decoded);

                        // Determine final samples to push
                        let final_samples: &[f32] = if needs_resample {
                            // Downmix to stereo if needed
                            let stereo_input = if format.native_channels != 2 {
                                downmix_to_stereo(&decoded, format.native_channels, &mut stereo_buf);
                                &stereo_buf
                            } else {
                                &decoded
                            };

                            // Resample to 48kHz via Rubato
                            match resampler.as_mut().unwrap().process_interleaved(stereo_input) {
                                Ok(resampled) => resampled,
                                Err(_) => stereo_input,
                            }
                        } else {
                            &decoded
                        };

                        if rb_producer.vacant_len() >= final_samples.len() {
                            let _ = rb_producer.push_slice(final_samples);
                        }
                    }

                    let _ = cap_client.ReleaseBuffer(num_frames_available);

                    packet_length = match cap_client.GetNextPacketSize() {
                        Ok(len) => len,
                        Err(_) => break,
                    };
                }

                notify_clone.notify_one();
            }

            let _ = windows::Win32::Foundation::CloseHandle(event_handle);
        });

        Ok(CaptureHandle {
            backend: Box::new(WasapiLoopbackCapture {
                client: SendClient(client_clone),
                is_running,
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
