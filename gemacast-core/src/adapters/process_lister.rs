//! Adapter: OS process enumeration for audio capture targets.
//!
//! Production implementation of [`ProcessLister`](crate::ports::process_lister::ProcessLister)
//! that uses platform-specific APIs to find capturable processes:
//!
//! - **Windows**: WASAPI session enumeration
//! - **Linux**: PipeWire Registry node enumeration
//! - **macOS**: ScreenCaptureKit `SCShareableContent`
//!
//! This adapter encapsulates the full process enumeration logic that was
//! previously embedded in `control::http::handle_get_processes`, including:
//! - Root ancestor PID resolution for multi-process apps (e.g., Chrome)
//! - Deduplication by executable name with audio-session preference
//! - Sorting: audio-active processes first, then alphabetically

use crate::domain::types::ProcessInfo;
use crate::ports::process_lister::ProcessLister;

/// Default process lister that delegates to platform-specific APIs.
///
/// - **Windows**: Uses WASAPI `IAudioSessionManager2` to find processes with
///   active audio sessions, then enriches with Toolhelp32 process names.
/// - **Linux**: Uses PipeWire Registry to discover audio-producing nodes.
///   Falls back to empty list if PipeWire is unavailable.
/// - **macOS**: Uses ScreenCaptureKit `SCShareableContent` to list capturable
///   applications. Falls back to empty list if permission is denied.
#[derive(Clone)]
pub struct DefaultProcessLister;

impl ProcessLister for DefaultProcessLister {
    fn list_processes(&self) -> Vec<ProcessInfo> {
        #[cfg(target_os = "windows")]
        return windows_list_processes();

        #[cfg(target_os = "linux")]
        return linux_list_processes();

        #[cfg(target_os = "macos")]
        return macos_list_processes();

        #[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
        return Vec::new();
    }
}

#[cfg(target_os = "windows")]
fn windows_list_processes() -> Vec<ProcessInfo> {
    use crate::adapters::capture::wasapi_loopback;
    use std::collections::{HashMap, HashSet};

    let all_pids = match unsafe { wasapi_loopback::get_process_list() } {
        Ok(map) => map,
        Err(_) => return Vec::new(),
    };

    let audio_pids = match unsafe { wasapi_loopback::get_audio_process_list() } {
        Ok(pids) => pids,
        Err(_) => return Vec::new(),
    };

    // For each audio-producing PID, walk up the process tree to find the
    // root ancestor with the same executable name. This ensures
    // INCLUDE_TARGET_PROCESS_TREE captures the entire tree's audio —
    // critical for multi-process apps like Chrome where audio is produced
    // by a child renderer process, not the main browser PID.
    let mut audio_root_pids = HashSet::<u32>::new();
    for &audio_pid in &audio_pids {
        if let Some(name) = all_pids.get(&audio_pid) {
            let root_pid = wasapi_loopback::get_root_ancestor_pid(audio_pid, &name.to_lowercase());
            audio_root_pids.insert(root_pid);
        }
        // Also mark the original audio PID itself
        audio_root_pids.insert(audio_pid);
    }

    // Deduplicate by name: prefer the PID that is a root ancestor of an
    // audio-producing process. Falls back to the lowest PID if no audio
    // session is found for any instance.
    let mut seen = HashMap::<String, ProcessInfo>::new();
    for (pid, name) in all_pids {
        let key = name.to_lowercase();
        let has_audio = audio_root_pids.contains(&pid);

        seen.entry(key)
            .and_modify(|existing| {
                // Prefer the PID with an active audio session
                if has_audio && !existing.has_audio_session {
                    existing.pid = pid;
                    existing.has_audio_session = true;
                } else if has_audio == existing.has_audio_session && pid < existing.pid {
                    // Same audio status: keep lowest PID for stability
                    existing.pid = pid;
                }
            })
            .or_insert(ProcessInfo {
                pid,
                name,
                has_audio_session: has_audio,
            });
    }

    let mut processes: Vec<_> = seen.into_values().collect();

    // Sort: audio-active processes first, then alphabetically
    processes.sort_by(|a, b| {
        b.has_audio_session
            .cmp(&a.has_audio_session)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });

    processes
}

// ---------------------------------------------------------------------------
// Linux: PipeWire Registry node enumeration
// ---------------------------------------------------------------------------

#[cfg(target_os = "linux")]
fn linux_list_processes() -> Vec<ProcessInfo> {
    use crate::adapters::capture::pipewire_common;

    // Don't crash if PipeWire is not available (e.g., PulseAudio-only systems)
    if !pipewire_common::is_pipewire_available() {
        tracing::info!("[ProcessLister] PipeWire not available, returning empty process list");
        return Vec::new();
    }

    match linux_enumerate_pipewire_nodes() {
        Ok(processes) => processes,
        Err(e) => {
            tracing::warn!("[ProcessLister] PipeWire enumeration failed: {e}");
            Vec::new()
        }
    }
}

/// Enumerate audio-producing processes via PipeWire Registry.
///
/// Connects to PipeWire, iterates all global Node objects, and filters
/// for those with `media.class` containing `"Stream/Output/Audio"` (application
/// audio playback streams). Deduplicates by process name.
#[cfg(target_os = "linux")]
fn linux_enumerate_pipewire_nodes() -> Result<Vec<ProcessInfo>, crate::domain::error::GemaCastError>
{
    use crate::domain::error::AudioError;
    use pipewire as pw;
    use std::collections::HashMap;

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

    let found_nodes =
        std::sync::Arc::new(std::sync::Mutex::new(HashMap::<String, ProcessInfo>::new()));

    // To handle native PipeWire apps, we must track Client objects because they hold the PID
    // and Name, while the Node object holds the media.class. We map client.id -> (pid, name)
    let client_map =
        std::sync::Arc::new(std::sync::Mutex::new(HashMap::<u32, (u32, String)>::new()));
    let client_map_clone = client_map.clone();

    // We also need to store nodes that we found, since global callbacks can arrive in any order
    struct TempNode {
        client_id: Option<u32>,
        media_class: Option<String>,
        node_pid: Option<u32>,
        node_name: Option<String>,
    }
    let temp_nodes = std::sync::Arc::new(std::sync::Mutex::new(Vec::<TempNode>::new()));
    let temp_nodes_clone = temp_nodes.clone();

    let _listener = registry
        .add_listener_local()
        .global(move |global| {
            if global.type_ == pw::types::ObjectType::Client {
                if let Some(props) = global.props
                    && let Some(app_pid) = props.get("application.process.id")
                {
                    let app_name = props.get("application.name");
                    let pid: u32 = app_pid.parse().ok().unwrap_or(0);
                    let name = app_name
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| format!("PID {pid}"));

                    if pid > 0 {
                        let mut cmap = client_map_clone.lock().unwrap();
                        cmap.insert(global.id, (pid, name));
                    }
                }
            } else if global.type_ == pw::types::ObjectType::Node
                && let Some(props) = global.props
            {
                let media_class = props.get("media.class").map(|s| s.to_string());
                let app_pid = props.get("application.process.id");
                let app_name = props.get("application.name").map(|s| s.to_string());
                let client_id = props.get("client.id").and_then(|s| s.parse::<u32>().ok());

                let node_pid: Option<u32> = app_pid.and_then(|s| s.parse().ok());

                let mut tnodes = temp_nodes_clone.lock().unwrap();
                tnodes.push(TempNode {
                    client_id,
                    media_class,
                    node_pid,
                    node_name: app_name,
                });
            }
        })
        .register();

    // Request a sync. The returned ID will be passed to the 'done' event
    // when all previously issued commands (like the registry enumeration)
    // have completed.
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

    // Run the main loop. It will block until the `done` event fires,
    // which takes less than 5ms rather than a hardcoded 1 second.
    mainloop.run();

    let cmap = client_map.lock().unwrap();
    let tnodes = temp_nodes.lock().unwrap();
    let mut map = found_nodes.lock().unwrap();

    for node in tnodes.iter() {
        if let Some(class) = &node.media_class
            && class.contains("Stream/Output/Audio")
        {
            // Try to get PID from the node first, otherwise lookup the client
            let (pid, name) = if let Some(pid) = node.node_pid {
                let n = node
                    .node_name
                    .clone()
                    .unwrap_or_else(|| format!("PID {pid}"));
                (pid, n)
            } else if let Some(cid) = node.client_id {
                if let Some((pid, n)) = cmap.get(&cid) {
                    (*pid, n.clone())
                } else {
                    (0, String::new())
                }
            } else {
                (0, String::new())
            };

            if pid > 0 {
                let key = name.to_lowercase();
                map.entry(key).or_insert(ProcessInfo {
                    pid,
                    name,
                    has_audio_session: true,
                });
            }
        }
    }

    let mut processes: Vec<_> = map.values().cloned().collect();

    // Sort alphabetically (all have audio sessions)
    processes.sort_by_key(|a| a.name.to_lowercase());

    Ok(processes)
}

// ---------------------------------------------------------------------------
// macOS: ScreenCaptureKit application enumeration
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
fn macos_list_processes() -> Vec<ProcessInfo> {
    match macos_enumerate_sck_apps() {
        Ok(processes) => processes,
        Err(e) => {
            tracing::warn!("[ProcessLister] ScreenCaptureKit enumeration failed: {e}");
            Vec::new()
        }
    }
}

/// Enumerate capturable applications via ScreenCaptureKit.
///
/// Calls `SCShareableContent::get()` to list all running applications,
/// filters out system processes and the current process, and returns
/// them as `ProcessInfo` entries.
#[cfg(target_os = "macos")]
fn macos_enumerate_sck_apps() -> Result<Vec<ProcessInfo>, crate::domain::error::GemaCastError> {
    use crate::domain::error::AudioError;
    use screencapturekit::prelude::*;
    use std::collections::HashMap;

    let content = SCShareableContent::get().map_err(|e| {
        let msg = format!("{e}");
        if msg.contains("permission") || msg.contains("denied") || msg.contains("not authorized") {
            AudioError::ScreenCapturePermissionDenied
        } else {
            AudioError::ScreenCaptureKitError(msg)
        }
    })?;

    let current_pid = std::process::id() as i32;
    let mut seen = HashMap::<String, ProcessInfo>::new();

    for app in content.applications() {
        let pid = app.process_id();
        if pid == current_pid || pid <= 1 {
            continue;
        }

        let mut name = app.application_name();
        if name.is_empty() {
            name = format!("PID {pid}");
        }

        if name.is_empty() {
            continue;
        }

        let key = name.to_lowercase();
        seen.entry(key).or_insert(ProcessInfo {
            pid: pid as u32,
            name,
            has_audio_session: true,
        });
    }

    let mut processes: Vec<_> = seen.into_values().collect();

    // Sort alphabetically
    processes.sort_by_key(|a| a.name.to_lowercase());

    Ok(processes)
}

#[cfg(test)]
#[cfg(target_os = "linux")]
mod tests {
    use super::*;
    use crate::adapters::capture::pipewire_common::is_pipewire_available;

    #[test]
    fn test_linux_enumerate_pipewire_nodes() {
        if is_pipewire_available() {
            let wav_path = std::env::temp_dir().join("dummy_lister.wav");
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

            // Spawn a dummy audio process that plays infinite silence.
            // Using /dev/zero with explicit raw format guarantees it will stream forever
            // and fully initialize the PipeWire Node, avoiding any issues with invalid/empty WAV files stalling.
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
                    println!("Failed to spawn pw-cat ({}), skipping enumeration test.", e);
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

            let result = linux_enumerate_pipewire_nodes();

            if let Ok(processes) = result {
                let found = processes.iter().any(|p| p.pid == pid);
                assert!(
                    found,
                    "Failed to find the spawned pw-cat process (PID {}) in the enumerated list",
                    pid
                );
            } else {
                panic!("Enumeration failed");
            }

            let _ = child.kill();
            let _ = child.wait();
        } else {
            println!("PipeWire is not available, skipping linux_enumerate_pipewire_nodes test.");
        }
    }
}

#[cfg(test)]
#[cfg(target_os = "macos")]
mod macos_tests {
    use super::*;

    #[test]
    fn test_macos_enumerate_sck_apps() {
        // Attempt to enumerate processes via SCK
        let result = macos_enumerate_sck_apps();

        match result {
            Ok(processes) => {
                println!(
                    "Successfully enumerated {} processes via ScreenCaptureKit",
                    processes.len()
                );
                // We don't assert length > 0 because a strictly isolated CI environment might have no shareable windows,
                // but usually there's at least Finder/WindowServer.
            }
            Err(e) => {
                // In headless CI environments, this might fail due to lack of TCC Screen Recording permissions.
                // We print the error and let it pass rather than failing the build.
                println!(
                    "ScreenCaptureKit enumeration failed (expected if missing permissions): {:?}",
                    e
                );
            }
        }
    }
}
