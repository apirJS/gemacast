mod domains;
mod state;

use state::AppState;

/// Seconds after which a sender with no heartbeat is considered offline.
pub(crate) const SENDER_HEARTBEAT_TIMEOUT_SECS: u64 = 30;

/// Interval between watchdog sweeps that check for stale senders.
pub(crate) const HEARTBEAT_CHECK_INTERVAL_SECS: u64 = 1;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(AppState::new())
        .setup(|app| {
            domains::ipc::server::spawn_service_command_listener(app.handle().clone());
            Ok(())
        })
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_device_info::init())
        .invoke_handler(tauri::generate_handler![
            domains::discovery::commands::get_local_ip,
            domains::discovery::commands::get_network_identifier,
            domains::discovery::commands::get_connection_status,
            domains::discovery::commands::start_listening_for_senders,
            domains::discovery::commands::stop_listening_for_senders,
            domains::audio::commands::connect_to_sender,
            domains::audio::commands::disconnect_from_sender,
            domains::audio::commands::start_audio_playback,
            domains::audio::commands::stop_audio_playback,
            domains::audio::commands::notify_streaming_stopped,
            domains::audio::commands::kill_playback,
            domains::audio::commands::update_jitter_config,
            domains::audio::commands::get_audio_sources,
            domains::audio::commands::change_audio_source,
            domains::audio::commands::get_process_list,
            domains::audio::commands::establish_websocket,
            domains::audio::commands::probe_sender,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|_app_handle, event| {
            if let tauri::RunEvent::Exit = event {
                std::process::exit(0);
            }
        });
}
