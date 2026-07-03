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

    let client_map = std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::<
        u32,
        u32,
    >::new()));
    let client_map_clone = client_map.clone();

    struct TempNode {
        id: u32,
        client_id: Option<u32>,
        media_class: Option<String>,
        node_pid: Option<u32>,
    }
    let temp_nodes = std::sync::Arc::new(std::sync::Mutex::new(Vec::<TempNode>::new()));
    let temp_nodes_clone = temp_nodes.clone();

    let _listener = registry
        .add_listener_local()
        .global(move |global| {
            if global.type_ == pw::types::ObjectType::Client {
                if let Some(props) = global.props
                    && let Some(pid_str) = props.get("application.process.id")
                    && let Ok(app_pid) = pid_str.parse::<u32>()
                {
                    let mut cmap = client_map_clone.lock().unwrap();
                    cmap.insert(global.id, app_pid);
                }
            } else if global.type_ == pw::types::ObjectType::Node
                && let Some(props) = global.props
            {
                let media_class = props.get("media.class").map(|s| s.to_string());
                let app_pid = props
                    .get("application.process.id")
                    .and_then(|s| s.parse::<u32>().ok());
                let client_id = props.get("client.id").and_then(|s| s.parse::<u32>().ok());

                let mut tnodes = temp_nodes_clone.lock().unwrap();
                tnodes.push(TempNode {
                    id: global.id,
                    client_id,
                    media_class,
                    node_pid: app_pid,
                });
            }
        })
        .register();

    let mainloop_weak = mainloop.downgrade();
    let pending_sync = core
        .sync(0)
        .map_err(|e| AudioError::PipeWireError(format!("Sync: {e}")))?;

    let _core_listener = core
        .add_listener_local()
        .done(move |_id, _seq| {
            if _seq == pending_sync
                && let Some(ml) = mainloop_weak.upgrade()
            {
                ml.quit();
            }
        })
        .register();

    // Run the mainloop briefly to enumerate
    // Use a timer as a fallback timeout just in case sync hangs
    let mainloop_weak2 = mainloop.downgrade();
    let timer = mainloop.loop_().add_timer(move |_| {
        if let Some(ml) = mainloop_weak2.upgrade() {
            ml.quit();
        }
    });
    timer.update_timer(
        Some(std::time::Duration::from_secs(2)),
        None, // One-shot
    );
    let _keep_timer = timer;

    mainloop.run();

    let cmap = client_map.lock().unwrap();
    let tnodes = temp_nodes.lock().unwrap();
    let mut found_node_id = None;

    for node in tnodes.iter() {
        if let Some(class) = &node.media_class
            && class.contains("Stream/Output/Audio")
        {
            let node_pid = if let Some(p) = node.node_pid {
                p
            } else if let Some(cid) = node.client_id {
                *cmap.get(&cid).unwrap_or(&0)
            } else {
                0
            };

            if node_pid == pid {
                found_node_id = Some(node.id.to_string());
                break;
            }
        }
    }

    let result = found_node_id.ok_or(GemaCastError::Audio(AudioError::ProcessNotFound(pid)))?;

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
            let wav_path = std::env::temp_dir().join("dummy_process.wav");
            {
                use std::io::Write;
                let mut f = std::fs::File::create(&wav_path).unwrap();
                f.write_all(&[
                    b'R', b'I', b'F', b'F', 0x24, 0x53, 0x07, 0x00, b'W', b'A', b'V', b'E', b'f',
                    b'm', b't', b' ', 16, 0, 0, 0, 1, 0, 1, 0, 0x80, 0xbb, 0x00, 0x00, 0x80, 0xbb,
            // Create a dummy sink in PipeWire so pw-cat doesn't exit instantly in headless CI
            let _ = std::process::Command::new("pw-cli")
                .args([
                    "create-node",
                    "adapter",
                    "{ factory.name=support.null-audio-sink node.name=\"dummy-sink\" media.class=Audio/Sink object.linger=true }",
                ])
                .status();

            std::thread::sleep(std::time::Duration::from_millis(200));

            // To test end-to-end process capture, we spawn a dummy audio process playing infinite silence
            let mut child = match std::process::Command::new("pw-cat")
                .arg("-p")
                .arg("-f")
                .arg("s16")
                .arg("-r")
                .arg("48000")
                .arg("-c")
                .arg("2")
                .arg("/dev/zero")
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
            {
                Ok(child) => child,
                Err(e) => {
                    println!("Failed to spawn pw-cat ({}), skipping end-to-end test.", e);
                    return;
                }
            };

            let pid = child.id();

            // Give WirePlumber a moment to create the node
            std::thread::sleep(std::time::Duration::from_millis(1000));

            // Check if it already exited
            if let Ok(Some(status)) = child.try_wait() {
                let mut stderr_str = String::new();
                if let Some(mut stderr) = child.stderr.take() {
                    use std::io::Read;
                    let _ = stderr.read_to_string(&mut stderr_str);
                }
                panic!("pw-cat exited prematurely with status {:?}. Stderr: {}", status, stderr_str);
            }

            let result = create_pipewire_process_loopback(pid);

            let _ = child.kill();

            if let Err(e) = result {
                panic!("Expected success capturing dummy process, got {:?}", e);
            }

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
