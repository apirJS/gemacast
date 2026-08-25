#![cfg(target_os = "windows")]

use crate::{
    audio::{CaptureResampler, OPUS_FRAME_SAMPLES},
    domain::error::{AudioError, GemaCastError},
    ports::capture::{CaptureBackend, CaptureCounters, CaptureHandle},
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
        counters,
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
        counters,
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
        // One `Arc`, two owners: the capture thread writes it, the handle hands it to
        // `capture_pool` to log. Desktop and process capture share this whole function
        // (`wasapi_desktop.rs` is a type alias), so both modes are covered here.
        let counters = Arc::new(CaptureCounters::default());
        let counters_thread = counters.clone();

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
                counters_thread,
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
            counters,
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
    counters: Arc<CaptureCounters>,
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
                    &counters,
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

/// Buffer duration asked of `IAudioClient::Initialize`, in 100-nanosecond units.
///
/// 10 000 000 is one second. That is a *capacity* request and not a latency figure —
/// the capture period is set by the event WASAPI signals, not by how much the ring can
/// hold, so a generous capacity only decides how long a scheduler stall can run before
/// the driver starts overwriting undelivered packets. The granted value is logged by
/// [`log_granted_buffer`] because a shared-mode client does not always get what it asks
/// for, and there is otherwise no way to tell from a field capture what it got.
const REQUESTED_BUFFER_DURATION_100NS: i64 = 10_000_000;

/// Log the buffer size WASAPI actually granted, next to what was asked for.
///
/// Never fails the caller. `GetBufferSize` is a diagnostic read here, and a capture that
/// works is worth more than a log line that is complete — but a warn on the failing path
/// keeps the absence visible rather than making it look like the call was never made.
///
/// The granted size is quoted in frames, so it goes through
/// [`frames_to_ms`](crate::audio::frames_to_ms) against the *native* rate rather than
/// 48 kHz: this is the device's own buffer, measured before any resampling.
fn log_granted_buffer(audio_client: &IAudioClient, format: &WasapiFormat, capture_kind: &str) {
    match unsafe { audio_client.GetBufferSize() } {
        Ok(frames) => {
            let granted_ms = crate::audio::frames_to_ms(frames, format.native_rate);
            tracing::info!(
                capture_kind,
                granted_frames = frames,
                "[WASAPI] Buffer: requested {:.1} ms, granted {} frames ({})",
                REQUESTED_BUFFER_DURATION_100NS as f64 / 10_000.0,
                frames,
                match granted_ms {
                    Some(ms) => format!("{ms:.1} ms"),
                    None => "rate unknown".to_owned(),
                }
            );
        }
        Err(error) => tracing::warn!(
            capture_kind,
            "[WASAPI] Buffer: requested {:.1} ms, granted size unavailable ({error})",
            REQUESTED_BUFFER_DURATION_100NS as f64 / 10_000.0,
        ),
    }
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
    let format = match unsafe { parse_mix_format(mix_format_ptr) } {
        Ok(format) => format,
        Err(error) => {
            // The pointer is CoTaskMem-allocated and owned by us from here, so it has
            // to be freed on the rejection path too — the success path frees it after
            // `Initialize` consumes it below.
            unsafe {
                windows::Win32::System::Com::CoTaskMemFree(Some(mix_format_ptr as _));
            }
            return Err(error);
        }
    };

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
            REQUESTED_BUFFER_DURATION_100NS,
            0,
            mix_format_ptr,
            None,
        )
    };
    unsafe {
        windows::Win32::System::Com::CoTaskMemFree(Some(mix_format_ptr as _));
    }
    init_result.map_err(AudioError::WindowsApi)?;

    log_granted_buffer(&audio_client, &format, capture_kind);

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
    counters: &CaptureCounters,
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
                // AUDCLNT_BUFFERFLAGS_SILENT (0x2), or a null buffer, which WASAPI is
                // permitted to hand back for a silent packet. Emitting zeros keeps the
                // sample clock continuous; the count is what makes an idle desktop
                // distinguishable from a capture that stopped delivering.
                CaptureCounters::add(&counters.silent_buffers, 1);
                let silent_samples = frames as usize * 2;
                if rb_producer.vacant_len() >= silent_samples {
                    for _ in 0..silent_samples {
                        let _ = rb_producer.try_push(0.0);
                    }
                } else {
                    CaptureCounters::add(&counters.dropped_samples, silent_samples as u64);
                }
                Ok(())
            } else {
                unsafe {
                    decode_samples_to_f32(buffer_ptr, format, frames as usize, decoded, counters);
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
                } else {
                    // Whole-packet drop, same policy as the PipeWire push. Counting it
                    // is the point; whether a partial push would be better is a
                    // decision for after a field capture shows how often this fires.
                    CaptureCounters::add(&counters.dropped_samples, final_samples.len() as u64);
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

/// Decode a `PROCESSENTRY32`'s `szExeFile` into a `String`.
///
/// The field is a fixed 260-byte buffer holding a NUL-terminated ANSI name, so the
/// terminator has to be found rather than assumed and the bytes are not necessarily
/// valid UTF-8 — a name in a non-Latin ANSI code page decodes lossily here rather than
/// failing. That is acceptable for a display name and for the lowercase comparisons
/// against [`SYSTEM_PROCESS_FILTER`], which only ever match ASCII.
fn entry_exe_name(entry: &PROCESSENTRY32) -> String {
    let bytes: Vec<u8> = entry
        .szExeFile
        .iter()
        .copied()
        .take_while(|b| *b != 0)
        .collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Call `visit` once for every process in a fresh Toolhelp32 snapshot.
///
/// `Process32First` both validates the snapshot **and** fills in the first entry, so
/// the correct shape is visit-then-advance. Using `Process32First` purely as a validity
/// check and then `Process32Next` as the loop condition — the shape this replaces —
/// silently drops whichever process the snapshot lists first. Enumeration order is not
/// documented, so that is not reliably a process nobody cares about.
///
/// # Safety
///
/// Calls `CreateToolhelp32Snapshot`, `Process32First`, `Process32Next` and
/// `CloseHandle`. Safe to call from any thread. The snapshot is closed before this
/// returns on every path, including if `visit` panics — the handle is owned locally and
/// released by the guard below.
unsafe fn for_each_process(mut visit: impl FnMut(&PROCESSENTRY32)) -> Result<(), GemaCastError> {
    unsafe {
        let snapshot =
            CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).map_err(AudioError::WindowsApi)?;

        // Closes the snapshot on the way out whatever happens inside `visit`.
        struct SnapshotGuard(windows::Win32::Foundation::HANDLE);
        impl Drop for SnapshotGuard {
            fn drop(&mut self) {
                unsafe {
                    let _ = CloseHandle(self.0);
                }
            }
        }
        let _guard = SnapshotGuard(snapshot);

        let mut entry = PROCESSENTRY32 {
            dwSize: std::mem::size_of::<PROCESSENTRY32>() as u32,
            ..Default::default()
        };

        if Process32First(snapshot, &mut entry).is_err() {
            // An empty or unreadable snapshot. Not an error: the caller gets an empty
            // result, which is what it would have got from the old code too.
            return Ok(());
        }

        loop {
            visit(&entry);
            if Process32Next(snapshot, &mut entry).is_err() {
                return Ok(());
            }
        }
    }
}

/// Enumerate all running processes, returning a map of PID → display name.
/// System and infrastructure processes are filtered out.
///
/// # Safety
///
/// Calls Win32 Toolhelp32 snapshot APIs via [`for_each_process`]. Safe to call
/// from any thread.
pub unsafe fn get_process_list() -> Result<std::collections::HashMap<u32, String>, GemaCastError> {
    let mut map = std::collections::HashMap::new();

    unsafe {
        for_each_process(|entry| {
            // PID 0 is the Idle pseudo-process. It owns no image, cannot be opened, and
            // cannot hold an audio session, so it is never a capture target — but
            // Toolhelp32 reports it like any other entry, under the synthetic bracketed
            // name `[System Process]`.
            //
            // Filtering it here by PID rather than by name is deliberate.
            // [`SYSTEM_PROCESS_FILTER`] matches *executable* names, and `[System
            // Process]` is not one — it is text the kernel supplies in place of a name,
            // so no entry in that list was ever going to catch it.
            //
            // It did not appear in the picker before because the enumeration bug this
            // code replaced dropped whichever process the snapshot listed first, and PID
            // 0 is what Toolhelp32 lists first in practice. That made it an accidental
            // filter for exactly one entry, and an unreliable one: the order is not
            // documented, so on any snapshot that led with something else, a real process
            // went missing from the picker instead. Fixing the enumeration exposed PID 0;
            // this makes the exclusion explicit and independent of ordering.
            if entry.th32ProcessID == 0 {
                return;
            }

            let raw_name = entry_exe_name(entry);

            let lower = raw_name.to_lowercase();
            if SYSTEM_PROCESS_FILTER.contains(&lower.as_str()) {
                return;
            }

            let display_name = raw_name
                .strip_suffix(".exe")
                .or_else(|| raw_name.strip_suffix(".EXE"))
                .unwrap_or(&raw_name)
                .to_string();

            map.insert(entry.th32ProcessID, display_name);
        })?;
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

    // A snapshot failure leaves the map empty, and an empty map means the walk below
    // finds no parent and returns `pid` unchanged — the same fallback the explicit
    // early return used to provide.
    let walked = unsafe {
        for_each_process(|entry| {
            parent_map.insert(
                entry.th32ProcessID,
                (
                    entry.th32ParentProcessID,
                    entry_exe_name(entry).to_lowercase(),
                ),
            );
        })
    };
    if let Err(error) = walked {
        tracing::warn!(
            %error,
            pid,
            "[WASAPI] could not enumerate processes; capturing the target PID alone"
        );
        return pid;
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Process enumeration, which needs a live Windows box rather than mocked input.
    ///
    /// These run against the real machine, so they assert properties that hold on any
    /// Windows system rather than exact counts: that the walk reaches this very test
    /// process, and that it does not skip an entry.
    ///
    /// The coverage splits deliberately, and mistaking one for the other would leave a
    /// gap: `should_visit_every_entry_including_the_first` pins the *Toolhelp32
    /// semantics* that make the fix necessary — it walks a snapshot by hand and does not
    /// call `for_each_process` at all, because only a single shared snapshot makes the
    /// off-by-one deterministic. `should_reach_this_test_process` is the one that
    /// exercises `for_each_process` itself.
    mod process_enumeration {
        use super::*;

        #[test]
        fn should_visit_every_entry_including_the_first() {
            // The falsification, run inside one snapshot so both walks see an identical
            // process list — taking two snapshots would let a process start or exit
            // between them and turn the off-by-one into noise.
            //
            // Walk A is visit-then-advance, the shape `for_each_process` uses. Walk B is
            // the shape it replaces: `Process32First` treated purely as a validity check,
            // then `Process32Next` as the loop condition. `Process32First` rewinds the
            // snapshot, so B enumerates the same list from the top and its deficit is
            // exactly the entry A visits first.
            unsafe {
                let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).unwrap();
                let mut entry = PROCESSENTRY32 {
                    dwSize: std::mem::size_of::<PROCESSENTRY32>() as u32,
                    ..Default::default()
                };

                assert!(
                    Process32First(snapshot, &mut entry).is_ok(),
                    "a Windows machine always has at least one process"
                );
                let mut visit_then_advance = vec![entry.th32ProcessID];
                while Process32Next(snapshot, &mut entry).is_ok() {
                    visit_then_advance.push(entry.th32ProcessID);
                }

                assert!(Process32First(snapshot, &mut entry).is_ok());
                let mut advance_then_visit = Vec::new();
                while Process32Next(snapshot, &mut entry).is_ok() {
                    advance_then_visit.push(entry.th32ProcessID);
                }

                let _ = CloseHandle(snapshot);

                assert_eq!(
                    advance_then_visit.len() + 1,
                    visit_then_advance.len(),
                    "the old shape drops exactly one process"
                );
                assert_eq!(
                    &visit_then_advance[1..],
                    &advance_then_visit[..],
                    "and it is the first one, not an arbitrary one"
                );
            }
        }

        #[test]
        fn should_reach_this_test_process() {
            // An end-to-end check that `for_each_process` and `entry_exe_name` agree with
            // each other: the test binary is running, is not in `SYSTEM_PROCESS_FILTER`,
            // and therefore must appear with a name.
            let mut seen = Vec::new();
            unsafe { for_each_process(|entry| seen.push(entry.th32ProcessID)) }.unwrap();

            assert!(
                seen.contains(&std::process::id()),
                "the walk missed the process doing the walking"
            );

            let listed = unsafe { get_process_list() }.unwrap();
            let own_name = listed
                .get(&std::process::id())
                .expect("the test binary must be listed");
            assert!(
                !own_name.is_empty() && !own_name.ends_with(".exe"),
                "display names have the extension stripped, got {own_name:?}"
            );
        }

        #[test]
        fn should_close_the_snapshot_even_if_the_visitor_panics() {
            // The guard exists because `visit` is caller-supplied. Without it a panicking
            // visitor leaks a kernel handle per call, and the old code's explicit
            // `CloseHandle` after the loop had exactly that hole.
            let panicked = std::panic::catch_unwind(|| {
                unsafe { for_each_process(|_| panic!("visitor blew up")) }.unwrap();
            });
            assert!(panicked.is_err(), "the panic must propagate to the caller");
            // Nothing here can observe the handle count, so this pins the propagation
            // path and leaves the release to `SnapshotGuard`'s `Drop`. A second walk
            // succeeding at least shows the snapshot API is still usable.
            assert!(unsafe { get_process_list() }.is_ok());
        }

        /// PID 0 must not reach the picker.
        ///
        /// This is a regression test in the literal sense: fixing the enumeration bug
        /// above removed an *accidental* filter. The old code skipped whichever entry the
        /// snapshot listed first, and Toolhelp32 lists PID 0 first, so `[System Process]`
        /// was excluded by a coincidence of ordering rather than by any rule. With the
        /// walk corrected it appeared in the process list, where it is not selectable —
        /// the Idle pseudo-process owns no image and can never hold an audio session.
        ///
        /// The two halves are separate assertions on purpose. The first pins that the
        /// exclusion happens in `get_process_list` and not by luck; the second pins that
        /// `for_each_process` still *sees* PID 0, because the fix must not reintroduce a
        /// skip in the walk — `get_root_ancestor_pid` walks parent links through the same
        /// enumeration and a missing entry there would break the tree climb instead.
        #[test]
        fn should_not_offer_the_idle_pseudo_process_as_a_capture_source() {
            let listed = unsafe { get_process_list() }.unwrap();

            assert!(
                !listed.contains_key(&0),
                "PID 0 is the Idle pseudo-process and cannot be captured; got {:?}",
                listed.get(&0)
            );

            let mut walked = Vec::new();
            unsafe { for_each_process(|entry| walked.push(entry.th32ProcessID)) }.unwrap();
            assert!(
                walked.contains(&0),
                "the exclusion belongs in `get_process_list`, not in the walk — \
                 `get_root_ancestor_pid` needs every entry to climb parent links"
            );
        }
    }
}
