#![cfg(target_os = "linux")]

//! PipeWire per-process audio capture backend.
//!
//! Captures audio from a specific process by discovering its PipeWire node
//! via the Registry and targeting it with the `TARGET_OBJECT` stream property.
//!
//! # Process Discovery
//!
//! PipeWire assigns each audio stream a node in its session graph. Each node
//! carries properties including `application.process.id`. We use the Registry
//! to find the node belonging to our target PID, then configure the capture
//! stream to connect exclusively to that node.

use crate::domain::error::{AudioError, GemaCastError};
use crate::ports::capture::{CaptureBackend, CaptureHandle};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use pipewire as pw;
use pw::properties::properties;

use pw::stream::{StreamBox as Stream, StreamFlags};

use super::pipewire_common::{self, create_pw_ring_buffer, push_pw_audio_to_ringbuf};

/// PipeWire per-process audio capture backend.
///
/// Captures audio from a specific process identified by its PID.
/// Implements [`CaptureBackend`] with the same lifecycle as
/// [`super::wasapi_loopback::WasapiLoopbackCapture`].
pub struct PipeWireProcessCapture {
    is_running: Arc<AtomicBool>,
    thread_handle: Option<std::thread::JoinHandle<()>>,
}

impl CaptureBackend for PipeWireProcessCapture {
    fn play(&mut self) -> Result<(), GemaCastError> {
        Ok(())
    }

    fn pause(&mut self) -> Result<(), GemaCastError> {
        Ok(())
    }
}

impl Drop for PipeWireProcessCapture {
    fn drop(&mut self) {
        self.is_running.store(false, Ordering::Relaxed);
        if let Some(handle) = self.thread_handle.take() {
            let _ = handle.join();
        }
    }
}

/// Create a per-process audio capture handle using PipeWire.
///
/// Discovers the PipeWire node for the given PID via the Registry,
/// then creates a capture stream targeting that specific node.
///
/// # Arguments
///
/// * `pid` - The process ID of the application to capture audio from.
///
/// # Errors
///
/// * [`AudioError::ProcessNotFound`] if no PipeWire node is found for the PID.
/// * [`AudioError::PipeWireConnectionFailed`] if PipeWire cannot be initialized.
/// * [`AudioError::PipeWireError`] for runtime PipeWire errors.
pub fn create_pipewire_process_loopback(
    pid: u32,
) -> Result<CaptureHandle<super::PlatformCaptureBackend>, GemaCastError> {
    // First, discover the PipeWire node ID for this PID.
    let target_node_id = discover_node_for_pid(pid)?;

    let (mut producer, resources, stream_error_tx) = create_pw_ring_buffer();
    let notify_clone = resources.notify.clone();

    let is_running = Arc::new(AtomicBool::new(true));
    let is_running_thread = is_running.clone();

    let thread_handle = std::thread::spawn(move || {
        pw::init();

        let result = run_process_capture_loop(
            target_node_id,
            &mut producer,
            &notify_clone,
            &is_running_thread,
            stream_error_tx,
        );

        if let Err(e) = result {
            tracing::error!("[PipeWire Process] Capture loop error: {}", e);
        }

        notify_clone.notify_waiters();
    });

    Ok(CaptureHandle {
        backend: super::PlatformCaptureBackend::PipeWireProcess(PipeWireProcessCapture {
            is_running,
            thread_handle: Some(thread_handle),
        }),
        consumer: resources.consumer,
        notify: resources.notify,
        stream_error_rx: resources.stream_error_rx,
    })
}

/// Discover the PipeWire node ID for a process with the given PID.
///
/// Connects to PipeWire, enumerates all nodes via the Registry, and
/// matches the `application.process.id` property against the target PID.
///
/// # Returns
///
/// The PipeWire node `object.serial` or ID as a string suitable for
/// use with the `TARGET_OBJECT` stream property.
fn discover_node_for_pid(pid: u32) -> Result<String, GemaCastError> {
    pw::init();

    let mainloop = pw::main_loop::MainLoopRc::new(None)
        .map_err(|e| AudioError::PipeWireConnectionFailed(format!("MainLoop: {e}")))?;

    let context = pw::context::ContextRc::new(&mainloop, None)
        .map_err(|e| AudioError::PipeWireConnectionFailed(format!("Context: {e}")))?;

    let core = context
        .connect_rc(None)
        .map_err(|e| AudioError::PipeWireConnectionFailed(format!("Core: {e}")))?;

    let registry = core
        .get_registry()
        .map_err(|e| AudioError::PipeWireError(format!("Registry: {e}")))?;

    let target_pid_str = pid.to_string();
    let found_node_id = Arc::new(std::sync::Mutex::new(None::<String>));
    let found_clone = found_node_id.clone();
    let mainloop_weak = mainloop.downgrade();

    let _listener = registry
        .add_listener_local()
        .global(move |global| {
            // Only look at Node-type objects
            if global.type_ != pw::types::ObjectType::Node {
                return;
            }

            if let Some(props) = global.props {
                let app_pid = props.get("application.process.id");
                let media_class = props.get("media.class");

                // Match: correct PID AND audio output stream
                if app_pid == Some(&target_pid_str) {
                    if let Some(class) = media_class {
                        if class.contains("Stream/Output/Audio") {
                            let node_id = global.id.to_string();
                            tracing::info!(
                                "[PipeWire] Found node {} for PID {} (class: {})",
                                node_id,
                                pid,
                                class
                            );
                            *found_clone.lock().unwrap() = Some(node_id);

                            // We found our target, quit the main loop
                            if let Some(ml) = mainloop_weak.upgrade() {
                                ml.quit();
                            }
                        }
                    }
                }
            }
        })
        .register();

    // Run the main loop briefly to enumerate nodes.
    // Use a timeout to avoid hanging if the PID has no audio node.
    let mainloop_weak2 = mainloop.downgrade();
    let _timer = mainloop.loop_().add_timer(move |_| {
        if let Some(ml) = mainloop_weak2.upgrade() {
            ml.quit();
        }
    });
    if let Some(ref timer_source) = Some(_timer) {
        timer_source.update_timer(
            Some(std::time::Duration::from_secs(2)),
            None, // One-shot
        );
    }

    mainloop.run();

    let result = found_node_id
        .lock()
        .unwrap()
        .take()
        .ok_or(GemaCastError::Audio(AudioError::ProcessNotFound(pid)))?;

    Ok(result)
}

/// Internal: runs the PipeWire capture loop for a specific node.
fn run_process_capture_loop(
    target_node_id: String,
    producer: &mut ringbuf::HeapProd<f32>,
    notify: &Arc<tokio::sync::Notify>,
    is_running: &Arc<AtomicBool>,
    stream_error_tx: tokio::sync::mpsc::Sender<cpal::StreamError>,
) -> Result<(), GemaCastError> {
    let mainloop = pw::main_loop::MainLoopRc::new(None)
        .map_err(|e| AudioError::PipeWireConnectionFailed(format!("MainLoop: {e}")))?;

    let context = pw::context::ContextRc::new(&mainloop, None)
        .map_err(|e| AudioError::PipeWireConnectionFailed(format!("Context: {e}")))?;

    let core = context
        .connect_rc(None)
        .map_err(|e| AudioError::PipeWireConnectionFailed(format!("Core: {e}")))?;

    // Create a capture stream targeting the specific application node.
    let props = properties! {
        *pw::keys::MEDIA_TYPE => "Audio",
        *pw::keys::MEDIA_CATEGORY => "Capture",
        *pw::keys::MEDIA_ROLE => "Music",
        *pw::keys::NODE_NAME => "gemacast-process-capture",
        "target.object" => target_node_id.as_str(),
    };

    let stream = Stream::new(&core, "gemacast-process-capture", props)
        .map_err(|e| AudioError::PipeWireError(format!("Stream::new: {e}")))?;

    let producer_ptr = producer as *mut ringbuf::HeapProd<f32>;
    let notify_ptr = notify as *const Arc<tokio::sync::Notify>;
    let is_running_ptr = is_running as *const Arc<AtomicBool>;

    let is_running_err = is_running.clone();
    let mainloop_weak3 = mainloop.downgrade();

    let _listener = stream
        .add_local_listener::<()>()
        .state_changed(move |_, _, old_state, new_state| {
            tracing::debug!(
                "[PipeWire Process] stream state changed {:?} -> {:?}",
                old_state,
                new_state
            );
            match new_state {
                pw::stream::StreamState::Error(err) => {
                    tracing::error!("[PipeWire Process] stream error: {}", err);
                    is_running_err.store(false, Ordering::Relaxed);
                    let _ = stream_error_tx.try_send(cpal::StreamError::DeviceNotAvailable);
                    if let Some(ml) = mainloop_weak3.upgrade() {
                        ml.quit();
                    }
                }
                pw::stream::StreamState::Unconnected => {
                    tracing::warn!("[PipeWire Process] stream disconnected");
                    is_running_err.store(false, Ordering::Relaxed);
                    let _ = stream_error_tx.try_send(cpal::StreamError::DeviceNotAvailable);
                    if let Some(ml) = mainloop_weak3.upgrade() {
                        ml.quit();
                    }
                }
                _ => {}
            }
        })
        .process(move |stream, _| {
            let producer = unsafe { &mut *producer_ptr };
            let notify = unsafe { &*notify_ptr };
            let is_running = unsafe { &*is_running_ptr };

            if !is_running.load(Ordering::Relaxed) {
                return;
            }

            if let Some(mut buffer) = stream.dequeue_buffer() {
                let datas = buffer.datas_mut();
                if let Some(data) = datas.first_mut() {
                    let chunk = data.chunk();
                    let offset = chunk.offset() as usize;
                    let size = chunk.size() as usize;

                    if let Some(slice) = data.data()
                        && offset + size <= slice.len()
                    {
                        let audio_bytes = &slice[offset..offset + size];
                        let n_samples = size / std::mem::size_of::<f32>();

                        unsafe {
                            push_pw_audio_to_ringbuf(
                                audio_bytes.as_ptr() as *const f32,
                                n_samples,
                                producer,
                                notify,
                            );
                        }
                    }
                }
            }
        })
        .register()
        .map_err(|e| AudioError::PipeWireError(format!("Listener: {e}")))?;

    // Build audio format params: 48kHz stereo F32LE interleaved
    let values = pipewire_common::build_audio_params();
    let mut params = [pw::spa::pod::Pod::from_bytes(&values)
        .ok_or_else(|| AudioError::PipeWireError("Invalid pod bytes".to_string()))?];

    // We must use AUTOCONNECT even with TARGET_OBJECT, otherwise WirePlumber
    // won't attempt to connect our capture stream to the targeted node.
    stream
        .connect(
            pw::spa::utils::Direction::Input,
            None,
            StreamFlags::AUTOCONNECT | StreamFlags::MAP_BUFFERS | StreamFlags::RT_PROCESS,
            &mut params,
        )
        .map_err(|e| AudioError::PipeWireError(format!("Stream connect: {e}")))?;

    tracing::info!(
        "[PipeWire Process] Capture stream connected to node {}, entering main loop",
        target_node_id
    );

    // Periodic check to quit the loop when is_running goes false
    let is_running_timer = is_running.clone();
    let mainloop_weak = mainloop.downgrade();
    let _timer = mainloop.loop_().add_timer(move |_| {
        if !is_running_timer.load(Ordering::Relaxed)
            && let Some(ml) = mainloop_weak.upgrade()
        {
            ml.quit();
        }
    });
    if let Some(ref timer_source) = Some(_timer) {
        timer_source.update_timer(
            Some(std::time::Duration::from_millis(100)),
            Some(std::time::Duration::from_millis(100)),
        );
    }

    mainloop.run();

    tracing::info!("[PipeWire Process] Capture main loop exited");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::capture::pipewire_common::is_pipewire_available;

    #[test]
    fn test_process_capture_not_found() {
        if is_pipewire_available() {
            // PID 999999 is guaranteed not to have a PipeWire node
            let result = create_pipewire_process_loopback(999999);

            assert!(result.is_err(), "Expected error for non-existent PID");
            if let Err(e) = result {
                match e {
                    GemaCastError::Audio(AudioError::ProcessNotFound(pid)) => {
                        assert_eq!(pid, 999999);
                    }
                    _ => panic!("Expected ProcessNotFound error, got {:?}", e),
                }
            }
        } else {
            println!("PipeWire is not available, skipping process capture not found test.");
        }
    }

    #[test]
    fn test_process_capture_end_to_end() {
        if is_pipewire_available() {
            // To test end-to-end process capture, we spawn a dummy audio process.
            // `pw-play` is part of pipewire-bin and plays audio to the graph.
            // We'll spawn it, grab its PID, and try to capture it.
            let mut child = match std::process::Command::new("pw-play")
                // Using /dev/urandom as a dummy source is safe
                .arg("/dev/urandom")
                .spawn()
            {
                Ok(child) => child,
                Err(e) => {
                    println!("Failed to spawn pw-play ({}), skipping end-to-end test.", e);
                    return;
                }
            };

            let pid = child.id();

            // Give WirePlumber a moment to create the node
            std::thread::sleep(std::time::Duration::from_millis(500));

            let result = create_pipewire_process_loopback(pid);

            // The capture should succeed
            assert!(
                result.is_ok(),
                "Expected success capturing dummy process, got {:?}",
                result.err()
            );

            if let Ok(handle) = result {
                // Ensure Drop handles cleanup
                drop(handle);
            }

            // Cleanup the dummy process
            let _ = child.kill();
            let _ = child.wait();
        } else {
            println!("PipeWire is not available, skipping process capture end-to-end test.");
        }
    }
}
