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

use windows::Win32::{
    Foundation::CloseHandle,
    Media::Audio::{
        AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_EVENTCALLBACK, AUDCLNT_STREAMFLAGS_LOOPBACK,
        IAudioCaptureClient, IAudioClient, IMMDeviceEnumerator, MMDeviceEnumerator, eConsole,
        eRender,
    },
    System::{
        Com::{CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx},
        Threading::CreateEventW,
    },
};

struct SendClient(IAudioClient);
unsafe impl Send for SendClient {}
unsafe impl Sync for SendClient {}

struct WasapiDesktopCapture {
    client: SendClient,
    is_running: Arc<std::sync::atomic::AtomicBool>,
    thread_handle: Option<std::thread::JoinHandle<()>>,
}

impl CaptureBackend for WasapiDesktopCapture {
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

impl Drop for WasapiDesktopCapture {
    fn drop(&mut self) {
        self.is_running
            .store(false, std::sync::atomic::Ordering::Relaxed);
        if let Some(handle) = self.thread_handle.take() {
            let _ = handle.join();
        }
    }
}

/// Create a raw WASAPI desktop loopback capture handle.
///
/// Activates the default render endpoint in shared-mode loopback,
/// discovers the native mix format, and spawns a background thread
/// to pump audio into a ring buffer at 48kHz stereo.
///
/// # Errors
///
/// Returns [`AudioError::WindowsApi`] if any WASAPI call fails, or
/// [`AudioError::ResampleFailed`] if the Rubato resampler cannot be created.
pub fn create_wasapi_desktop_loopback() -> Result<CaptureHandle, GemaCastError> {
    unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED).map_err(AudioError::WindowsApi)?;

        // Get default render endpoint
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                .map_err(AudioError::WindowsApi)?;

        let device = enumerator
            .GetDefaultAudioEndpoint(eRender, eConsole)
            .map_err(AudioError::WindowsApi)?;

        // Activate IAudioClient directly (no async activation needed for endpoints)
        let audio_client: IAudioClient = device
            .Activate(CLSCTX_ALL, None)
            .map_err(AudioError::WindowsApi)?;

        // Query the native mix format
        let mix_format_ptr = audio_client
            .GetMixFormat()
            .map_err(AudioError::WindowsApi)?;

        let format = parse_mix_format(mix_format_ptr);

        tracing::info!(
            "[WASAPI Desktop] native_rate={}, native_channels={}, bits={}, block_align={}, is_float={}",
            format.native_rate,
            format.native_channels,
            format.bits_per_sample,
            format.block_align,
            format.is_float
        );

        // Initialize in shared-mode loopback
        let init_result = audio_client.Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_LOOPBACK | AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
            10_000_000, // 1 second buffer in 100ns units
            0,
            mix_format_ptr,
            None,
        );

        // Free the CoTaskMem-allocated format
        windows::Win32::System::Com::CoTaskMemFree(Some(mix_format_ptr as _));

        init_result.map_err(AudioError::WindowsApi)?;

        // Event-driven capture
        let event_handle =
            CreateEventW(None, false, false, None).map_err(AudioError::WindowsApi)?;

        audio_client
            .SetEventHandle(event_handle)
            .map_err(AudioError::WindowsApi)?;

        let capture_client: IAudioCaptureClient =
            audio_client.GetService().map_err(AudioError::WindowsApi)?;

        struct SendCaptureClient(IAudioCaptureClient);
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

        // Build resampler if needed (native rate differs from pipeline's 48kHz)
        let needs_resample = format.native_rate != 48000 || format.native_channels != 2;
        let mut resampler = if needs_resample {
            let resample_from_channels = if format.native_channels == 2 {
                format.native_channels
            } else {
                2 // after downmix
            };
            Some(CaptureResampler::new(
                format.native_rate,
                48000,
                resample_from_channels,
            )?)
        } else {
            None
        };

        let thread_handle = std::thread::spawn(move || {
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

                    // AUDCLNT_BUFFERFLAGS_SILENT = 0x2
                    if (flags & 2) != 0 || buffer_ptr.is_null() {
                        let silent_samples = vec![0.0f32; num_frames_available as usize * 2];
                        if rb_producer.vacant_len() >= silent_samples.len() {
                            let _ = rb_producer.push_slice(&silent_samples);
                        }
                    } else {
                        let src_frames = num_frames_available as usize;

                        // Decode raw bytes -> f32
                        decode_samples_to_f32(buffer_ptr, &format, src_frames, &mut decoded);

                        // Determine the samples to push
                        let final_samples: &[f32] = if needs_resample {
                            // Downmix to stereo if needed
                            let stereo_input = if format.native_channels != 2 {
                                downmix_to_stereo(
                                    &decoded,
                                    format.native_channels,
                                    &mut stereo_buf,
                                );
                                &stereo_buf
                            } else {
                                &decoded
                            };

                            // Resample to 48kHz
                            match resampler
                                .as_mut()
                                .unwrap()
                                .process_interleaved(stereo_input)
                            {
                                Ok(resampled) => resampled,
                                Err(_) => stereo_input, // Fallback: push un-resampled
                            }
                        } else {
                            // Already 48kHz stereo — passthrough
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

            let _ = CloseHandle(event_handle);
            notify_clone.notify_waiters();
        });

        Ok(CaptureHandle {
            backend: Box::new(WasapiDesktopCapture {
                client: SendClient(client_clone),
                is_running,
                thread_handle: Some(thread_handle),
            }),
            consumer: rb_consumer,
            notify,
            stream_error_rx,
        })
    }
}
