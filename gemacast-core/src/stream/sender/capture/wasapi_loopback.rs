#![cfg(target_os = "windows")]

use crate::{
    audio::{OPUS_CHANNELS, OPUS_FRAME_SAMPLES},
    error::{AudioCaptureError, GemaCastError},
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
            AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
            IActivateAudioInterfaceCompletionHandler,
            IActivateAudioInterfaceCompletionHandler_Impl,
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
            self.client
                .0
                .Start()
                .map_err(AudioCaptureError::WindowsApiError)?;
        }
        Ok(())
    }

    fn pause(&mut self) -> Result<(), GemaCastError> {
        unsafe {
            self.client
                .0
                .Stop()
                .map_err(AudioCaptureError::WindowsApiError)?;
        }
        Ok(())
    }
}

pub fn create_wasapi_process_loopback(pid: u32) -> Result<CaptureHandle, GemaCastError> {
    unsafe {
        let audio_client = activate_process_loopback(pid)?;
        let mix_format_ptr = audio_client
            .GetMixFormat()
            .map_err(AudioCaptureError::WindowsApiError)?;

        audio_client
            .Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
                10000000,
                0,
                mix_format_ptr,
                None,
            )
            .map_err(AudioCaptureError::WindowsApiError)?;

        let event_handle =
            windows::Win32::System::Threading::CreateEventW(None, false, false, None)
                .map_err(AudioCaptureError::WindowsApiError)?;

        audio_client
            .SetEventHandle(event_handle)
            .map_err(AudioCaptureError::WindowsApiError)?;

        let capture_client: windows::Win32::Media::Audio::IAudioCaptureClient = audio_client
            .GetService()
            .map_err(AudioCaptureError::WindowsApiError)?;

        windows::Win32::System::Com::CoTaskMemFree(Some(mix_format_ptr as _));

        struct SendCaptureClient(windows::Win32::Media::Audio::IAudioCaptureClient);
        unsafe impl Send for SendCaptureClient {}
        let send_capture_client = SendCaptureClient(capture_client);

        let rb = HeapRb::<f32>::new(OPUS_FRAME_SAMPLES * 64);
        let (mut rb_producer, rb_consumer) = rb.split();
        let (_error_tx, error_rx) = mpsc::channel::<cpal::StreamError>(1);
        let notify = Arc::new(Notify::new());
        let notify_clone = notify.clone();
        let client_clone = audio_client.clone();

        std::thread::spawn(move || {
            let send_capture_client = send_capture_client;
            let capture_client = send_capture_client.0;
            
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

                    if (flags & 2) != 0 {
                        let silent_samples =
                            vec![0.0f32; num_frames_available as usize * OPUS_CHANNELS as usize];
                        if rb_producer.vacant_len() >= silent_samples.len() {
                            let _ = rb_producer.push_slice(&silent_samples);
                        }
                    } else {
                        let byte_count = (num_frames_available * OPUS_CHANNELS as u32 * 4) as usize;
                        let audio_slice =
                            std::slice::from_raw_parts(buffer_ptr as *const f32, byte_count / 4);

                        if rb_producer.vacant_len() >= audio_slice.len() {
                            let _ = rb_producer.push_slice(audio_slice);
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
            error_rx,
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

        let payload = get_client().map_err(|e| AudioCaptureError::WindowsApiError(e).into());
        let _ = self.sender.send(payload);

        Ok(())
    }
}

#[allow(unused)]
unsafe fn activate_process_loopback(pid: u32) -> Result<IAudioClient, GemaCastError> {
    unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED)
            .map_err(AudioCaptureError::WindowsApiError)?;
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
        .map_err(AudioCaptureError::WindowsApiError)?;
    };

    let result = receiver.recv().unwrap_or_else(|_| {
        Err(
            AudioCaptureError::WindowsApiError(windows::core::Error::from(
                windows::Win32::Foundation::E_FAIL,
            ))
            .into(),
        )
    })?;

    Ok(result)
}

#[allow(unused)]
unsafe fn get_process_list() -> Result<std::collections::HashMap<u32, String>, GemaCastError> {
    let mut map = std::collections::HashMap::new();

    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)
            .map_err(AudioCaptureError::WindowsApiError)?;

        let mut entry = PROCESSENTRY32 {
            dwSize: std::mem::size_of::<PROCESSENTRY32>() as u32,
            ..Default::default()
        };

        if Process32First(snapshot, &mut entry).is_ok() {
            while Process32Next(snapshot, &mut entry).is_ok() {
                let name = String::from_utf8_lossy(
                    &entry
                        .szExeFile
                        .iter()
                        .copied()
                        .take_while(|b| *b != 0)
                        .collect::<Vec<u8>>(),
                )
                .into_owned();

                let lower_name = name.to_lowercase();
                if lower_name != "audiodg.exe" && lower_name != "svchost.exe" {
                    map.insert(entry.th32ProcessID, name);
                }
            }
        }

        CloseHandle(snapshot).map_err(AudioCaptureError::WindowsApiError)?;
    };

    Ok(map)
}

#[allow(unused)]
unsafe fn get_audio_process_list() -> Result<std::collections::HashSet<u32>, GemaCastError> {
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
                .map_err(AudioCaptureError::WindowsApiError)?;

        let device: IMMDevice = enumerator
            .GetDefaultAudioEndpoint(eRender, eConsole)
            .map_err(AudioCaptureError::WindowsApiError)?;

        let session_manager: IAudioSessionManager2 = device
            .Activate(CLSCTX_ALL, None)
            .map_err(AudioCaptureError::WindowsApiError)?;

        let session_enumerator: IAudioSessionEnumerator = session_manager
            .GetSessionEnumerator()
            .map_err(AudioCaptureError::WindowsApiError)?;

        let session_count = session_enumerator
            .GetCount()
            .map_err(AudioCaptureError::WindowsApiError)?;

        for i in 0..session_count {
            use windows::{
                Win32::Media::Audio::{IAudioSessionControl, IAudioSessionControl2},
                core::Interface,
            };

            let session: IAudioSessionControl = session_enumerator
                .GetSession(i)
                .map_err(AudioCaptureError::WindowsApiError)?;

            let session2: IAudioSessionControl2 =
                session.cast().map_err(AudioCaptureError::WindowsApiError)?;

            let pid = session2
                .GetProcessId()
                .map_err(AudioCaptureError::WindowsApiError)?;

            set.insert(pid);
        }
    }

    Ok(set)
}
