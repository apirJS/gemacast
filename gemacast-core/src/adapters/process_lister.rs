//! Adapter: OS process enumeration for audio capture targets.
//!
//! Production implementation of [`ProcessLister`](crate::ports::process_lister::ProcessLister)
//! that uses WASAPI session enumeration (Windows) to find capturable processes.
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
/// - **Other platforms**: Returns an empty list (process capture not supported).
#[derive(Clone)]
pub struct DefaultProcessLister;

impl ProcessLister for DefaultProcessLister {
    fn list_processes(&self) -> Vec<ProcessInfo> {
        #[cfg(target_os = "windows")]
        {
            windows_list_processes()
        }

        #[cfg(not(target_os = "windows"))]
        {
            Vec::new()
        }
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
