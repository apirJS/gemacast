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
    activate_process_loopback, decode_samples_to_f32, downmix_to_stereo, get_default_mix_format,
    parse_mix_format,
};

use windows::Win32::Media::Audio::PROCESS_LOOPBACK_MODE_EXCLUDE_TARGET_PROCESS_TREE;

struct SendClient(windows::Win32::Media::Audio::IAudioClient);
unsafe impl Send for SendClient {}
unsafe impl Sync for SendClient {}

pub struct WasapiDesktopCapture {
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

/// Create a desktop loopback capture handle using the modern Application Loopback API.
///
/// Uses `PROCESS_LOOPBACK_MODE_EXCLUDE_TARGET_PROCESS_TREE` with Gemacast's own PID
/// as the target. This captures all system audio **except** Gemacast's own process tree,
/// which:
/// 1. Bypasses OEM Audio Processing Objects (APOs) for clean, unprocessed audio
/// 2. Prevents feedback loops from Gemacast's own audio
///
/// Requires Windows 10 Build 20348+. Falls back to CPAL via the factory layer
/// on older Windows versions.
///
/// # Errors
///
/// Returns [`AudioError::WindowsApi`] if any WASAPI call fails, or
/// [`AudioError::ResampleFailed`] if the Rubato resampler cannot be created.
pub fn create_wasapi_desktop_loopback()
-> Result<CaptureHandle<super::PlatformCaptureBackend>, GemaCastError> {
    unsafe {
        // Use the modern Application Loopback API with EXCLUDE mode.
        // Target our own PID so we capture everything except Gemacast itself.
        let own_pid = std::process::id();
        let audio_client =
            activate_process_loopback(own_pid, PROCESS_LOOPBACK_MODE_EXCLUDE_TARGET_PROCESS_TREE)?;

        // Process loopback IAudioClients don't support GetMixFormat() (returns E_NOTIMPL).
        // We must query the system's shared-mode mix format from the default render endpoint.
        let mix_format_ptr = get_default_mix_format()?;
        let format = parse_mix_format(mix_format_ptr);

        tracing::info!(
            "[WASAPI Desktop] Application Loopback (EXCLUDE mode): native_rate={}, native_channels={}, bits={}, block_align={}, is_float={}",
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

            let _ = windows::Win32::Foundation::CloseHandle(event_handle);
            notify_clone.notify_waiters();
        });

        Ok(CaptureHandle {
            backend: super::PlatformCaptureBackend::WasapiDesktop(WasapiDesktopCapture {
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
