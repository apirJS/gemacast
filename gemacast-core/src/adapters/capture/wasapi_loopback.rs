#![cfg(target_os = "windows")]

use crate::{
    audio::{CaptureResampler, OPUS_FRAME_SAMPLES},
    domain::error::{AudioError, GemaCastError},
    ports::capture::{CaptureBackend, CaptureHandle},
};
use ringbuf::{HeapRb, traits::*};
use std::sync::Arc;
use tokio::sync::{Notify, mpsc};

use super::wasapi_common::{
    WasapiFormat, activate_process_loopback, decode_samples_to_f32, downmix_to_stereo,
    get_default_mix_format, parse_mix_format,
};

use windows::{
    Win32::{
        Foundation::{CloseHandle, HANDLE, WAIT_FAILED, WAIT_OBJECT_0},
        Media::Audio::{
            IAudioCaptureClient, IAudioClient, PROCESS_LOOPBACK_MODE,
            PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE,
        },
        System::{
            Com::CoUninitialize,
            Diagnostics::ToolHelp::{
                CreateToolhelp32Snapshot, PROCESSENTRY32, Process32First, Process32Next,
                TH32CS_SNAPPROCESS,
            },
            Threading::{CreateEventW, INFINITE, SetEvent, WaitForMultipleObjects},
        },
    },
    core::ComInterface,
};

type CommandResult = Result<(), String>;

enum CaptureCommand {
    Play(std::sync::mpsc::SyncSender<CommandResult>),
    Pause(std::sync::mpsc::SyncSender<CommandResult>),
    Shutdown,
}

pub struct WasapiLoopbackCapture {
    command_tx: std::sync::mpsc::Sender<CaptureCommand>,
    command_event: HANDLE,
    thread_handle: Option<std::thread::JoinHandle<()>>,
}

impl Drop for WasapiLoopbackCapture {
    fn drop(&mut self) {
        let _ = self.command_tx.send(CaptureCommand::Shutdown);
        unsafe {
            let _ = SetEvent(self.command_event);
        }
        if let Some(handle) = self.thread_handle.take() {
            let _ = handle.join();
        }
    }
}

impl CaptureBackend for WasapiLoopbackCapture {
    fn play(&mut self) -> Result<(), GemaCastError> {
        self.send_command(CaptureCommand::Play)
    }

    fn pause(&mut self) -> Result<(), GemaCastError> {
        self.send_command(CaptureCommand::Pause)
    }
}

impl WasapiLoopbackCapture {
    fn send_command(
        &self,
        make_command: fn(std::sync::mpsc::SyncSender<CommandResult>) -> CaptureCommand,
    ) -> Result<(), GemaCastError> {
        let (response_tx, response_rx) = std::sync::mpsc::sync_channel(1);
        self.command_tx
            .send(make_command(response_tx))
            .map_err(|_| capture_thread_stopped())?;

        unsafe {
            SetEvent(self.command_event).map_err(AudioError::WindowsApi)?;
        }

        response_rx
            .recv()
            .map_err(|_| capture_thread_stopped())?
            .map_err(|message| AudioError::CaptureInstanceFailed(message).into())
    }
}

fn capture_thread_stopped() -> GemaCastError {
    AudioError::CaptureInstanceFailed("WASAPI application-loopback thread stopped".into()).into()
}

pub fn create_wasapi_process_loopback(
    pid: u32,
) -> Result<CaptureHandle<super::PlatformCaptureBackend>, GemaCastError> {
    let CaptureHandle {
        backend,
        consumer,
        notify,
        stream_error_rx,
    } = create_wasapi_application_loopback(
        pid,
        PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE,
        "process",
    )?;

    Ok(CaptureHandle {
        backend: super::PlatformCaptureBackend::WasapiProcess(backend),
        consumer,
        notify,
        stream_error_rx,
    })
}

pub(crate) fn create_wasapi_application_loopback(
    pid: u32,
    mode: PROCESS_LOOPBACK_MODE,
    capture_kind: &'static str,
) -> Result<CaptureHandle<WasapiLoopbackCapture>, GemaCastError> {
    unsafe {
        let rb = HeapRb::<f32>::new(OPUS_FRAME_SAMPLES * 64);
        let (rb_producer, rb_consumer) = rb.split();
        let (stream_error_tx, stream_error_rx) = mpsc::channel::<cpal::StreamError>(1);
        let notify = Arc::new(Notify::new());
        let notify_clone = notify.clone();
        let capture_event =
            CreateEventW(None, false, false, None).map_err(AudioError::WindowsApi)?;
        let command_event = match CreateEventW(None, false, false, None) {
            Ok(event) => event,
            Err(error) => {
                let _ = CloseHandle(capture_event);
                return Err(AudioError::WindowsApi(error).into());
            }
        };
        let (command_tx, command_rx) = std::sync::mpsc::channel();
        let (init_tx, init_rx) = std::sync::mpsc::sync_channel(1);

        let thread_handle = std::thread::spawn(move || {
            run_application_loopback_thread(
                pid,
                mode,
                capture_kind,
                capture_event,
                command_event,
                command_rx,
                init_tx,
                rb_producer,
                notify_clone,
                stream_error_tx,
            );
        });

        match init_rx.recv() {
            Ok(Ok(())) => {}
            Ok(Err(message)) => {
                let _ = thread_handle.join();
                return Err(AudioError::CaptureInstanceFailed(message).into());
            }
            Err(_) => {
                let _ = thread_handle.join();
                return Err(capture_thread_stopped());
            }
        }

        Ok(CaptureHandle {
            backend: WasapiLoopbackCapture {
                command_tx,
                command_event,
                thread_handle: Some(thread_handle),
            },
            consumer: rb_consumer,
            notify,
            stream_error_rx,
        })
    }
}

#[allow(clippy::too_many_arguments)]
fn run_application_loopback_thread(
    pid: u32,
    mode: PROCESS_LOOPBACK_MODE,
    capture_kind: &'static str,
    capture_event: HANDLE,
    command_event: HANDLE,
    command_rx: std::sync::mpsc::Receiver<CaptureCommand>,
    init_tx: std::sync::mpsc::SyncSender<CommandResult>,
    mut rb_producer: ringbuf::HeapProd<f32>,
    notify: Arc<Notify>,
    stream_error_tx: mpsc::Sender<cpal::StreamError>,
) {
    let setup = unsafe { initialize_application_loopback(pid, mode, capture_kind, capture_event) };
    let (audio_client, capture_client, format, mut resampler) = match setup {
        Ok(setup) => setup,
        Err(error) => {
            let _ = init_tx.send(Err(error.to_string()));
            unsafe {
                let _ = CloseHandle(capture_event);
                let _ = CloseHandle(command_event);
                CoUninitialize();
            }
            notify.notify_waiters();
            return;
        }
    };

    if init_tx.send(Ok(())).is_err() {
        drop(capture_client);
        drop(audio_client);
        unsafe {
            let _ = CloseHandle(capture_event);
            let _ = CloseHandle(command_event);
            CoUninitialize();
        }
        return;
    }

    let mut started = false;
    let mut decoded = Vec::with_capacity(4096);
    let mut stereo_buf = Vec::with_capacity(4096);
    let wait_handles = [command_event, capture_event];

    'capture: loop {
        let wait_result = unsafe { WaitForMultipleObjects(&wait_handles, false, INFINITE) };
        if wait_result == WAIT_FAILED {
            let _ = stream_error_tx.try_send(cpal::StreamError::DeviceNotAvailable);
            break;
        }

        if wait_result.0 == WAIT_OBJECT_0.0 {
            while let Ok(command) = command_rx.try_recv() {
                match command {
                    CaptureCommand::Play(response_tx) => {
                        let result = if started {
                            Ok(())
                        } else {
                            unsafe { audio_client.Start() }
                                .map(|()| started = true)
                                .map_err(|error| error.to_string())
                        };
                        let _ = response_tx.send(result);
                    }
                    CaptureCommand::Pause(response_tx) => {
                        let result = if started {
                            unsafe { audio_client.Stop() }
                                .map(|()| started = false)
                                .map_err(|error| error.to_string())
                        } else {
                            Ok(())
                        };
                        let _ = response_tx.send(result);
                    }
                    CaptureCommand::Shutdown => break 'capture,
                }
            }
            continue;
        }

        if wait_result.0 == WAIT_OBJECT_0.0 + 1 && started {
            let result = unsafe {
                drain_capture_packets(
                    &capture_client,
                    &format,
                    &mut resampler,
                    &mut rb_producer,
                    &mut decoded,
                    &mut stereo_buf,
                )
            };
            if let Err(error) = result {
                let _ = stream_error_tx.try_send(cpal::StreamError::BackendSpecific {
                    err: cpal::BackendSpecificError {
                        description: error.to_string(),
                    },
                });
                break;
            }
            notify.notify_one();
        }
    }

    if started {
        unsafe {
            let _ = audio_client.Stop();
        }
    }
    drop(capture_client);
    drop(audio_client);
    unsafe {
        let _ = CloseHandle(capture_event);
        let _ = CloseHandle(command_event);
        CoUninitialize();
    }
    notify.notify_waiters();
}

unsafe fn initialize_application_loopback(
    pid: u32,
    mode: PROCESS_LOOPBACK_MODE,
    capture_kind: &str,
    capture_event: HANDLE,
) -> Result<
    (
        IAudioClient,
        IAudioCaptureClient,
        WasapiFormat,
        Option<CaptureResampler>,
    ),
    GemaCastError,
> {
    let audio_client = unsafe { activate_process_loopback(pid, mode)? };
    let mix_format_ptr = unsafe { get_default_mix_format()? };
    let format = unsafe { parse_mix_format(mix_format_ptr) };

    tracing::info!(
        capture_kind,
        "[WASAPI] Application loopback: native_rate={}, native_channels={}, bits={}, block_align={}, is_float={}",
        format.native_rate,
        format.native_channels,
        format.bits_per_sample,
        format.block_align,
        format.is_float
    );

    let init_result = unsafe {
        audio_client.Initialize(
            windows::Win32::Media::Audio::AUDCLNT_SHAREMODE_SHARED,
            windows::Win32::Media::Audio::AUDCLNT_STREAMFLAGS_LOOPBACK
                | windows::Win32::Media::Audio::AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
            10_000_000,
            0,
            mix_format_ptr,
            None,
        )
    };
    unsafe {
        windows::Win32::System::Com::CoTaskMemFree(Some(mix_format_ptr as _));
    }
    init_result.map_err(AudioError::WindowsApi)?;

    unsafe {
        audio_client
            .SetEventHandle(capture_event)
            .map_err(AudioError::WindowsApi)?;
    }
    let capture_client = unsafe { audio_client.GetService().map_err(AudioError::WindowsApi)? };
    let resampler = if format.native_rate != 48_000 || format.native_channels != 2 {
        Some(CaptureResampler::new(format.native_rate, 48_000, 2)?)
    } else {
        None
    };

    Ok((audio_client, capture_client, format, resampler))
}

unsafe fn drain_capture_packets(
    capture_client: &IAudioCaptureClient,
    format: &WasapiFormat,
    resampler: &mut Option<CaptureResampler>,
    rb_producer: &mut ringbuf::HeapProd<f32>,
    decoded: &mut Vec<f32>,
    stereo_buf: &mut Vec<f32>,
) -> Result<(), GemaCastError> {
    let mut packet_length = unsafe {
        capture_client
            .GetNextPacketSize()
            .map_err(AudioError::WindowsApi)?
    };

    while packet_length > 0 {
        let mut buffer_ptr = std::ptr::null_mut();
        let mut frames = 0;
        let mut flags = 0;
        let mut device_position = 0;
        let mut qpc_position = 0;
        unsafe {
            capture_client
                .GetBuffer(
                    &mut buffer_ptr,
                    &mut frames,
                    &mut flags,
                    Some(&mut device_position),
                    Some(&mut qpc_position),
                )
                .map_err(AudioError::WindowsApi)?;
        }

        let process_result = (|| -> Result<(), GemaCastError> {
            if frames == 0 {
                Ok(())
            } else if (flags & 2) != 0 || buffer_ptr.is_null() {
                let silent_samples = frames as usize * 2;
                if rb_producer.vacant_len() >= silent_samples {
                    for _ in 0..silent_samples {
                        let _ = rb_producer.try_push(0.0);
                    }
                }
                Ok(())
            } else {
                unsafe {
                    decode_samples_to_f32(buffer_ptr, format, frames as usize, decoded);
                }
                let final_samples = if let Some(resampler) = resampler.as_mut() {
                    let stereo_input = if format.native_channels == 2 {
                        decoded.as_slice()
                    } else {
                        downmix_to_stereo(decoded, format.native_channels, stereo_buf);
                        stereo_buf.as_slice()
                    };
                    resampler
                        .process_interleaved(stereo_input)
                        .map_err(|error| AudioError::ResampleFailed(error.to_string()))?
                } else {
                    decoded.as_slice()
                };
                if rb_producer.vacant_len() >= final_samples.len() {
                    let _ = rb_producer.push_slice(final_samples);
                }
                Ok(())
            }
        })();

        unsafe {
            capture_client
                .ReleaseBuffer(frames)
                .map_err(AudioError::WindowsApi)?;
        }
        process_result?;
        packet_length = unsafe {
            capture_client
                .GetNextPacketSize()
                .map_err(AudioError::WindowsApi)?
        };
    }

    Ok(())
}

// AudioActivator, activate_process_loopback, and get_default_mix_format
// are now shared from wasapi_common.rs

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
